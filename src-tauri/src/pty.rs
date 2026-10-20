//! Pseudo-terminal sessions, one per pane.
//!
//! Output is streamed to the webview as raw bytes over a `tauri::ipc::Channel` and also kept
//! in a per-pane ring buffer, so a reloaded webview (or a pane re-attached after a layout
//! change) can replay scrollback while the agent keeps running.

use std::collections::{HashMap, VecDeque};
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};

use portable_pty::{ChildKiller, CommandBuilder, MasterPty, PtySize, native_pty_system};
use serde::{Deserialize, Serialize};
use tauri::ipc::{Channel, InvokeResponseBody};
use tauri::{AppHandle, Emitter, Manager, State};

use crate::env::{login_env, user_shell};
use crate::providers;
use crate::{browser, hooks, mcp};

const SCROLLBACK_BYTES: usize = 2 * 1024 * 1024;
const READ_BUF_BYTES: usize = 64 * 1024;

struct Output {
    scrollback: VecDeque<u8>,
    channel: Option<Channel>,
}

impl Output {
    fn push(&mut self, bytes: &[u8]) {
        self.scrollback.extend(bytes);
        let overflow = self.scrollback.len().saturating_sub(SCROLLBACK_BYTES);
        self.scrollback.drain(..overflow);
        if let Some(channel) = &self.channel {
            // Fails only if the webview went away; scrollback still has the data.
            let _ = channel.send(InvokeResponseBody::Raw(bytes.to_vec()));
        }
    }
}

struct Session {
    master: Box<dyn MasterPty + Send>,
    /// Input for the pane's writer thread. Writing to a pty blocks while the program isn't
    /// reading (a big paste into a busy agent; on Windows the ConPTY pipe is small), so it
    /// never happens on the thread that handles the command.
    input: mpsc::Sender<Vec<u8>>,
    killer: Box<dyn ChildKiller + Send + Sync>,
    pid: Option<u32>,
    output: Arc<Mutex<Output>>,
}

/// A pane whose process is being started.
#[derive(Default)]
struct Starting {
    /// Held while starting, so a pane never gets two processes.
    lock: Mutex<()>,
    /// Set when the pane is killed while starting: its process is ended instead of kept.
    killed: AtomicBool,
}

#[derive(Default)]
pub struct PtyManager {
    sessions: Mutex<HashMap<String, Session>>,
    starting: Mutex<HashMap<String, Arc<Starting>>>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SpawnRequest {
    pane_id: String,
    provider_id: String,
    cwd: Option<String>,
    workspace_id: Option<String>,
    /// Append the provider's resume arguments (used when restoring panes after a restart).
    #[serde(default)]
    resume: bool,
    /// The agent session to resume (captured from its hooks); without one, resuming picks the
    /// most recent session in the folder.
    #[serde(default)]
    session: Option<String>,
    /// Initial prompt for a fresh agent (race mode, voice "open claude and tell it to …").
    #[serde(default)]
    prompt: Option<String>,
    /// Model to launch with (the provider's `modelArg`); `None` uses the CLI's default.
    #[serde(default)]
    model: Option<String>,
    /// Skip the agent's permission prompts (its `autoApproveArgs`), from settings.
    #[serde(default)]
    auto_approve: bool,
    /// Added to the agent's system prompt (from settings); empty or `None` adds nothing.
    #[serde(default)]
    system_prompt: Option<String>,
    /// A/B test baseline: start the agent without winfer's MCP tools, the browser, or the
    /// user's own MCP servers and skills (see [`plain_args`]). Status hooks stay on.
    #[serde(default)]
    plain: bool,
    /// Codex CLI panes: start Codex lean and with its output cap (see [`lean_codex_args`]), from
    /// settings (off by default; winfer code's Codex has its own switch).
    #[serde(default)]
    lean: bool,
    /// MCP tools to leave out of the agent's tool list (Settings → Agent tools).
    #[serde(default)]
    hidden_tools: Vec<String>,
    cols: u16,
    rows: u16,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SpawnResult {
    /// True if the pane already had a running session and was re-attached.
    attached: bool,
    pid: Option<u32>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ExitEvent {
    pane_id: String,
    code: u32,
    /// Lets the UI ignore a killed process's exit once the pane has been respawned.
    pid: Option<u32>,
}

fn size(cols: u16, rows: u16) -> PtySize {
    PtySize {
        rows: rows.max(2),
        cols: cols.max(2),
        pixel_width: 0,
        pixel_height: 0,
    }
}

/// Expands `~` and falls back to the home directory if the path doesn't exist.
pub(crate) fn resolve_cwd(cwd: Option<&str>) -> PathBuf {
    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"));
    let path = match cwd {
        Some("~") => home.clone(),
        Some(p) if p.starts_with("~/") => home.join(&p[2..]),
        Some(p) => PathBuf::from(p),
        None => home.clone(),
    };
    if path.is_dir() { path } else { home }
}

/// Orders an agent's CLI arguments. MCP args come first: Claude's `--mcp-config` is variadic and
/// would swallow a following positional prompt, and the single-value `--settings` after it ends
/// the list. Codex's `-c` overrides must precede subcommands like `resume`.
fn agent_args(
    provider: &providers::Provider,
    mcp_args: Vec<String>,
    hook_args: Vec<String>,
    model: Option<&str>,
    resume: bool,
    session: Option<&str>,
    prompt: Option<&str>,
) -> Vec<String> {
    let mut args = mcp_args;
    args.extend(hook_args);
    args.extend(provider.args.iter().cloned());
    // Global option, so it goes before subcommands like Codex's `resume`.
    if let (Some(flag), Some(model)) = (&provider.model_arg, model.filter(|m| !m.is_empty())) {
        args.extend([flag.clone(), model.to_string()]);
    }
    let session = session.filter(|s| !s.is_empty() && !provider.resume_session_args.is_empty());
    if let (true, Some(session)) = (resume, session) {
        args.extend(provider.resume_session_args.iter().map(|a| a.replace("{session}", session)));
    } else if resume {
        args.extend(provider.resume_args.iter().cloned());
    } else if let Some(prompt) = prompt {
        if let Some(flag) = &provider.prompt_flag {
            args.extend([flag.clone(), prompt.to_string()]);
        } else if provider.prompt_arg {
            args.push(prompt.to_string());
        }
    }
    args
}

/// Moves the system prompt into the start prompt for agents with no system prompt flag (only for
/// a fresh session, which is the only one that gets a start prompt). Plain A/B runs get none.
fn with_system_prompt(mut req: SpawnRequest) -> SpawnRequest {
    let Some(system) = req.system_prompt.take().filter(|s| !s.trim().is_empty() && !req.plain) else {
        return req;
    };
    match providers::find(&req.provider_id) {
        Some(p) if p.command.is_some() && !p.system_prompt_args.is_empty() => req.system_prompt = Some(system),
        Some(p) if p.command.is_some() && !req.resume => {
            req.prompt = Some(match req.prompt.take().filter(|p| !p.trim().is_empty()) {
                Some(prompt) => format!("{system}\n\n{prompt}"),
                None => system,
            });
        }
        _ => {}
    }
    req
}

/// The provider's `system_prompt_args` with the text filled in.
fn system_prompt_args(provider: &providers::Provider, prompt: &str) -> Vec<String> {
    let toml = serde_json::to_string(prompt).unwrap_or_default();
    provider
        .system_prompt_args
        .iter()
        .map(|a| a.replace("{prompt_toml}", &toml).replace("{prompt}", prompt))
        .collect()
}

/// Arguments that keep an agent from loading the user's own MCP servers and skills, for a
/// plain A/B test run. Simple flags, so they're safe in the MCP position of [`agent_args`].
fn plain_args(provider_id: &str) -> Vec<String> {
    match provider_id {
        "claude" => vec!["--strict-mcp-config".into(), "--disable-slash-commands".into()],
        "codex" => vec!["-c".into(), "mcp_servers={}".into()],
        "pi" => vec!["--no-mcp".into(), "--no-skills".into()],
        _ => Vec::new(),
    }
}

/// Arguments that turn off an agent's bundled extras winfer's panes don't need, so its tool list
/// and skill catalog stay small. Codex: ChatGPT apps (`mcp__codex_apps__*`), plugins and their
/// skills, the bundled system skills, image generation, computer use, its own browser (winfer's
/// stays), goals, plugin-install suggestions and the sleep tool. `code_mode_host` stays on:
/// without it Codex's `exec` tool fails closed.
fn lean_args(provider_id: &str) -> Vec<String> {
    const CODEX: &[&str] = &[
        "features.apps=false",
        "features.plugins=false",
        "features.remote_plugin=false",
        "features.image_generation=false",
        "features.computer_use=false",
        "features.browser_use=false",
        "features.goals=false",
        "features.tool_suggest=false",
        "features.sleep_tool=false",
        "skills.bundled.enabled=false",
    ];
    match provider_id {
        "codex" => CODEX.iter().flat_map(|o| ["-c".to_string(), o.to_string()]).collect(),
        _ => Vec::new(),
    }
}

/// Claude Code's limits on a single tool result, lowered from its defaults (30k characters of
/// shell output, 25k tokens per file read or MCP result): tool output is most of an agent's
/// context, and the biggest results are a few percent of calls but almost a third of the tokens.
/// Over the limit, shell output is cut and an oversized read fails asking for an offset and
/// limit. `winfer code` passes them on to the Claude Code it runs.
const OUTPUT_CAPS: [(&str, &str); 3] = [
    ("BASH_MAX_OUTPUT_LENGTH", "15000"),
    ("CLAUDE_CODE_FILE_READ_MAX_OUTPUT_TOKENS", "12000"),
    ("MAX_MCP_OUTPUT_TOKENS", "15000"),
];

/// Codex's limit on a single tool result, in tokens: about the shell cap Claude Code gets (its
/// own default for Codex models is 10k).
const CODEX_OUTPUT_TOKENS: u32 = 4000;

/// Codex's output cap as config overrides, unless the user's Codex config sets one itself (an
/// override would win over it). For `winfer code`'s Codex backend, and Codex CLI panes with Lean
/// Codex on.
pub(crate) fn codex_output_cap() -> Vec<String> {
    let home = std::env::var_os("CODEX_HOME").map(PathBuf::from).or_else(|| dirs::home_dir().map(|h| h.join(".codex")));
    let config = home.and_then(|h| std::fs::read_to_string(h.join("config.toml")).ok()).unwrap_or_default();
    if sets_output_limit(&config) {
        return Vec::new();
    }
    vec!["-c".into(), format!("tool_output_token_limit={CODEX_OUTPUT_TOKENS}")]
}

/// Whether a Codex config sets `tool_output_token_limit` (at the top level or in a profile).
fn sets_output_limit(config: &str) -> bool {
    config.lines().any(|l| l.trim_start().strip_prefix("tool_output_token_limit").is_some_and(|rest| rest.trim_start().starts_with('=')))
}

/// The [`OUTPUT_CAPS`] a provider gets: those the user hasn't set themselves (compared without
/// case, as Windows does).
fn output_caps(provider_id: &str, env: &HashMap<String, String>) -> Vec<(String, String)> {
    if !matches!(provider_id, "claude" | "winfer") {
        return Vec::new();
    }
    OUTPUT_CAPS
        .iter()
        .filter(|(key, _)| !env.keys().any(|k| k.eq_ignore_ascii_case(key)))
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

/// Instructions Codex adds to every request that winfer's panes don't need: its permissions
/// (approvals come from winfer's settings), ChatGPT apps and collaboration modes.
const CODEX_LEAN_INSTRUCTIONS: &[&str] = &[
    "include_permissions_instructions=false",
    "include_apps_instructions=false",
    "include_collaboration_mode_instructions=false",
];

/// Everything Lean Codex turns off, as config overrides: the bundled extras ([`lean_args`]), the
/// instructions above and the skills not picked (Settings → winfer code → Skills); with
/// `short_prompt`, Codex's built-in system prompt is swapped for a short one too. In the bench
/// (12 tasks, same pass rate): 37% fewer input tokens, 52% with the short prompt.
pub(crate) fn lean_codex_args(short_prompt: bool) -> Vec<String> {
    let mut args = lean_args("codex");
    args.extend(CODEX_LEAN_INSTRUCTIONS.iter().flat_map(|o| ["-c".to_string(), o.to_string()]));
    args.extend(crate::harness::claude_skills::codex_args(&crate::harness::config::Config::load().lean_skills()));
    if let Some(file) = short_prompt.then(codex_prompt_file).flatten() {
        // A JSON string is a valid TOML string, backslashes of Windows paths included.
        args.extend(["-c".into(), format!("model_instructions_file={}", serde_json::to_string(&file.to_string_lossy()).unwrap_or_default())]);
    }
    args
}

/// The short prompt for Lean Codex as a file (Codex reads its system prompt from one), written
/// when missing or out of date.
fn codex_prompt_file() -> Option<PathBuf> {
    let dir = crate::harness::config::dir();
    std::fs::create_dir_all(&dir).ok()?;
    let file = dir.join("codex-lean-prompt.md");
    if std::fs::read_to_string(&file).ok().as_deref() != Some(crate::harness::backend::CODEX_LEAN_PROMPT) {
        std::fs::write(&file, crate::harness::backend::CODEX_LEAN_PROMPT).ok()?;
    }
    Some(file)
}

/// `winfer code` runs this GUI-subsystem binary, which Windows doesn't attach to the pane's
/// pseudoconsole (its TUI aborted with exit code 0xC0000409). cmd.exe, a console program, holds
/// the console and waits for it (`start /b /wait`, which also passes on its exit code); winfer
/// code attaches to cmd.exe's console (`harness::attach_console`). The arguments after `code`
/// travel as JSON in the returned environment value, out of cmd.exe's reach.
#[cfg(windows)]
fn console_host(cmd: &CommandBuilder) -> (CommandBuilder, String) {
    let argv = cmd.get_argv();
    let args: Vec<String> = argv.iter().skip(2).map(|a| a.to_string_lossy().into_owned()).collect();
    let shell = login_env()
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("ComSpec"))
        .map(|(_, v)| v.clone())
        .unwrap_or_else(|| "cmd.exe".into());
    let mut host = CommandBuilder::new(shell);
    host.args(["/d", "/c", "start", "", "/b", "/wait"]);
    host.args(&argv[..2.min(argv.len())]);
    (host, serde_json::to_string(&args).unwrap_or_default())
}

/// The command for a pane, and whether its start prompt has to be typed in rather than passed
/// as an argument.
fn build_command(req: &SpawnRequest, cdp_url: Option<&str>) -> Result<(CommandBuilder, bool), String> {
    let mut provider = providers::find(&req.provider_id)
        .ok_or_else(|| format!("unknown provider: {}", req.provider_id))?;

    let mut type_prompt = false;
    // Set after the login environment, which they'd otherwise be overwritten by.
    let mut agent_env = Vec::new();
    let mut cmd = match &provider.command {
        Some(command) => {
            let servers: Vec<_> = if req.plain {
                Vec::new()
            } else {
                // In a project indexed with codegraph, its own server replaces the kb_* tools it covers.
                let codegraph = mcp::codegraph_server(&resolve_cwd(req.cwd.as_deref()));
                let mut hidden = req.hidden_tools.clone();
                if codegraph.is_some() {
                    hidden.extend(mcp::CODEGRAPH_COVERS.map(String::from));
                }
                mcp::winfer_server(&req.pane_id, &hidden)
                    .into_iter()
                    .chain(codegraph)
                    .chain(cdp_url.map(|url| mcp::filtered(browser::playwright_server(url), &req.hidden_tools)))
                    .collect()
            };
            let launcher = crate::env::launcher(command)
                .ok_or_else(|| format!("{} isn't installed (no `{command}` on PATH)", provider.name))?;
            let mut cmd = CommandBuilder::new(&launcher.program);
            cmd.args(&launcher.args);
            let opencode_v2 = provider.id == "opencode" && providers::opencode_v2(&launcher);
            // OpenCode 1 has no `--auto`; its config allows everything instead (see agent_env).
            let config_approve = req.auto_approve && provider.id == "opencode" && !opencode_v2;
            if req.auto_approve && !config_approve {
                let extra = std::mem::take(&mut provider.auto_approve_args);
                provider.args.extend(extra);
            }
            // Multi-line text doesn't survive cmd.exe, so batch launchers go without.
            if let (Some(system), false) = (&req.system_prompt, launcher.batch) {
                let extra = system_prompt_args(&provider, system);
                provider.args.extend(extra);
            }
            // Only OpenCode 2 plugins have the API ours uses (and OpenCode 1 reads `plugin`).
            let plugin = hooks::server().filter(|_| opencode_v2).map(|s| s.opencode_plugin.to_string_lossy().into_owned());
            // Environment variables don't pass through cmd.exe, so these work even for a batch file.
            agent_env = mcp::agent_env(
                &provider.id,
                &servers,
                plugin.as_deref(),
                config_approve,
                login_env().get("OPENCODE_CONFIG_CONTENT").map(String::as_str),
            );
            if !req.plain {
                agent_env.extend(output_caps(&provider.id, login_env()));
            }
            // OpenCode 2 runs sessions in a shared background server that never sees this
            // process's environment (so our MCP servers and plugin); its own private server does.
            if opencode_v2 && !agent_env.is_empty() {
                cmd.arg("--standalone");
            }
            // A batch file we couldn't unwrap passes arguments through cmd.exe: only simple ones
            // survive, so JSON configs are left out and the prompt is typed in instead.
            type_prompt = launcher.batch;
            let (mcp, hooks) = if launcher.batch {
                (Vec::new(), Vec::new())
            } else if req.plain {
                (plain_args(&provider.id), hooks::provider_args(&provider.id))
            } else {
                let mut mcp = mcp::agent_args(&provider.id, &servers);
                if req.lean && provider.id == "codex" {
                    mcp.extend(codex_output_cap());
                    mcp.extend(lean_codex_args(false));
                }
                (mcp, hooks::provider_args(&provider.id))
            };
            cmd.args(agent_args(
                &provider,
                mcp,
                hooks,
                req.model.as_deref(),
                req.resume,
                req.session.as_deref(),
                req.prompt.as_deref().filter(|_| !launcher.batch),
            ));
            #[cfg(windows)]
            if provider.id == "winfer" {
                let (host, args) = console_host(&cmd);
                agent_env.push((crate::harness::CODE_ARGS_ENV.to_string(), args));
                cmd = host;
            }
            cmd
        }
        None => {
            let mut cmd = CommandBuilder::new(user_shell());
            #[cfg(unix)]
            cmd.arg("-l");
            // pwsh's "new version available" banner pads its box to the console width and
            // crashes the shell (PadRight(-1)) when the pane is narrower than the box.
            #[cfg(windows)]
            agent_env.push(("POWERSHELL_UPDATECHECK".to_string(), "Off".to_string()));
            cmd
        }
    };

    for (key, value) in login_env() {
        cmd.env(key, value);
    }
    for (key, value) in agent_env {
        cmd.env(key, value);
    }
    cmd.env("TERM", "xterm-256color");
    cmd.env("COLORTERM", "truecolor");
    cmd.env("TERM_PROGRAM", "winfer");
    // xterm.js renders OSC 8 hyperlinks, but CLIs only emit them for terminals they know: this makes
    // Claude Code link the files it reads and edits, and OpenCode its markdown links (opened in the viewer).
    cmd.env("FORCE_HYPERLINK", "1");
    cmd.env("WINFER_PANE_ID", &req.pane_id);
    if let Some(server) = hooks::server() {
        cmd.env("WINFER_HOOK", &server.addr);
        cmd.env("WINFER_TOKEN", &server.token);
    }
    if let Some(url) = cdp_url {
        cmd.env("WINFER_CDP_URL", url);
    }
    if let Some(workspace) = &req.workspace_id {
        cmd.env("WINFER_WORKSPACE", workspace);
    }
    cmd.cwd(resolve_cwd(req.cwd.as_deref()));
    Ok((cmd, type_prompt))
}

/// Starts (or re-attaches to) a pane's process. On a blocking thread: starting an agent can take
/// seconds (the workspace browser launches on demand, Windows process start-up), which on the
/// main thread would freeze the whole window.
#[tauri::command]
pub async fn pty_spawn(app: AppHandle, req: SpawnRequest, on_data: Channel) -> Result<SpawnResult, String> {
    tauri::async_runtime::spawn_blocking(move || spawn_session(&app, req, on_data))
        .await
        .map_err(|e| e.to_string())?
}

/// Re-attaches a running pane to a new channel: replays scrollback, then routes live output there.
fn reattach(session: &Session, req: &SpawnRequest, on_data: Channel) -> SpawnResult {
    // Holding the output lock keeps replay and live bytes in order.
    let mut output = session.output.lock().unwrap();
    let replay: Vec<u8> = output.scrollback.iter().copied().collect();
    if !replay.is_empty() {
        let _ = on_data.send(InvokeResponseBody::Raw(replay));
    }
    output.channel = Some(on_data);
    let _ = session.master.resize(size(req.cols, req.rows));
    SpawnResult { attached: true, pid: session.pid }
}

fn spawn_session(app: &AppHandle, req: SpawnRequest, on_data: Channel) -> Result<SpawnResult, String> {
    let manager = app.state::<PtyManager>();
    let starting = manager.starting.lock().unwrap().entry(req.pane_id.clone()).or_default().clone();
    let _starting = starting.lock.lock().unwrap_or_else(|e| e.into_inner());
    // Only a kill during this start counts, not one of an earlier process.
    starting.killed.store(false, Ordering::SeqCst);

    // The sessions lock is only held briefly: pty_write and pty_resize need it on the main thread.
    if let Some(session) = manager.sessions.lock().unwrap().get(&req.pane_id) {
        return Ok(reattach(session, &req, on_data));
    }

    // Agents share the workspace browser (started on demand if one is installed).
    let cdp_url = match (&req.workspace_id, req.provider_id.as_str()) {
        (Some(ws), provider) if provider != "shell" && !req.plain => {
            app.state::<browser::BrowserManager>().cdp_url(app, ws)
        }
        _ => None,
    };
    let req = with_system_prompt(req);
    let (cmd, type_prompt) = build_command(&req, cdp_url.as_deref())?;
    let pair = native_pty_system()
        .openpty(size(req.cols, req.rows))
        .map_err(|e| e.to_string())?;
    let mut child = pair.slave.spawn_command(cmd).map_err(|e| e.to_string())?;
    drop(pair.slave);

    let mut reader = pair.master.try_clone_reader().map_err(|e| e.to_string())?;
    let mut writer = pair.master.take_writer().map_err(|e| e.to_string())?;
    let pid = child.process_id();
    let mut killer = child.clone_killer();
    let output = Arc::new(Mutex::new(Output {
        scrollback: VecDeque::new(),
        channel: Some(on_data),
    }));
    let (input, inputs) = mpsc::channel::<Vec<u8>>();

    {
        let mut sessions = manager.sessions.lock().unwrap();
        if starting.killed.load(Ordering::SeqCst) {
            drop(sessions);
            terminate(pid, killer.as_mut());
            std::thread::spawn(move || {
                let _ = child.wait();
            });
            return Err("the pane was closed while it started".into());
        }
        sessions.insert(
            req.pane_id.clone(),
            Session {
                master: pair.master,
                input: input.clone(),
                killer,
                pid,
                output: output.clone(),
            },
        );
    }

    // Ends when the session is dropped (its sender goes) or the program is gone.
    std::thread::spawn(move || {
        for data in inputs {
            if writer.write_all(&data).is_err() {
                break;
            }
        }
    });

    let reader_output = output.clone();
    std::thread::spawn(move || {
        let mut buf = vec![0u8; READ_BUF_BYTES];
        loop {
            match reader.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => reader_output.lock().unwrap().push(&buf[..n]),
            }
        }
    });

    let exit_app = app.clone();
    let pane_id = req.pane_id.clone();
    let session_output = output;
    std::thread::spawn(move || {
        let code = child.wait().map(|s| s.exit_code()).unwrap_or(1);
        let manager = exit_app.state::<PtyManager>();
        let mut sessions = manager.sessions.lock().unwrap();
        // Only drop the session if it is still this one (not killed and respawned).
        if sessions
            .get(&pane_id)
            .is_some_and(|s| Arc::ptr_eq(&s.output, &session_output))
        {
            sessions.remove(&pane_id);
        }
        drop(sessions);
        let _ = exit_app.emit("pty://exit", ExitEvent { pane_id, code, pid });
    });

    // CLIs without a prompt argument get the prompt typed in once they've had time to start.
    let typed_prompt = req.prompt.clone().filter(|_| {
        !req.resume
            && (type_prompt || !providers::find(&req.provider_id).is_some_and(|p| p.prompt_arg || p.prompt_flag.is_some()))
    });
    if let Some(prompt) = typed_prompt {
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(2500));
            // Multi-line prompts go in as one bracketed paste so the agent doesn't submit
            // at the first line break.
            let text = if prompt.contains('\n') {
                format!("\x1b[200~{prompt}\x1b[201~\r")
            } else {
                format!("{prompt}\r")
            };
            // Fails only if the pane is gone by now.
            let _ = input.send(text.into_bytes());
        });
    }

    Ok(SpawnResult {
        attached: false,
        pid,
    })
}

/// Queues input for the pane's writer thread, so a program that isn't reading can't block the
/// main thread. Stays a sync command: those run in the order they're invoked, which keeps
/// keystrokes in order (async ones may run in parallel).
#[tauri::command]
pub fn pty_write(
    state: State<'_, PtyManager>,
    pane_id: String,
    data: String,
) -> Result<(), String> {
    let sessions = state.sessions.lock().unwrap();
    let session = sessions.get(&pane_id).ok_or("no such pane")?;
    session
        .input
        .send(data.into_bytes())
        .map_err(|_| "the pane's program has exited".to_string())
}

#[tauri::command]
pub fn pty_resize(
    state: State<'_, PtyManager>,
    pane_id: String,
    cols: u16,
    rows: u16,
) -> Result<(), String> {
    let sessions = state.sessions.lock().unwrap();
    let session = sessions.get(&pane_id).ok_or("no such pane")?;
    session
        .master
        .resize(size(cols, rows))
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn pty_kill(state: State<'_, PtyManager>, pane_id: String) {
    // Before looking for the session: a start still under way then ends its process itself.
    if let Some(starting) = state.starting.lock().unwrap().get(&pane_id) {
        starting.killed.store(true, Ordering::SeqCst);
    }
    let Some(mut session) = state.sessions.lock().unwrap().remove(&pane_id) else {
        return;
    };
    terminate(session.pid, session.killer.as_mut());
}

/// Ends a pane's program and whatever it started.
fn terminate(pid: Option<u32>, killer: &mut (dyn ChildKiller + Send + Sync)) {
    // The child is a session leader (setsid), so its pid is also its process group id:
    // hang up the whole group so tools the agent started go away too, then force-kill
    // whatever ignored the hangup (a stuck command, a shell trapping SIGHUP).
    #[cfg(unix)]
    if let Some(pid) = pid {
        unsafe {
            libc::kill(-(pid as i32), libc::SIGHUP);
        }
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(500));
            unsafe {
                libc::kill(-(pid as i32), libc::SIGKILL);
            }
        });
    }
    #[cfg(not(unix))]
    let _ = pid;
    let _ = killer.kill();
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn spawned_shell_gets_pane_env_login_path_and_cwd() {
        let req = SpawnRequest {
            pane_id: "p-test".into(),
            provider_id: "shell".into(),
            cwd: Some("~".into()),
            workspace_id: Some("ws-test".into()),
            resume: false,
            session: None,
            prompt: None,
            model: None,
            auto_approve: false,
            system_prompt: None,
            plain: false,
            lean: false,
            hidden_tools: Vec::new(),
            cols: 80,
            rows: 24,
        };
        let (mut cmd, _) = build_command(&req, None).unwrap();
        cmd.args([
            "-c",
            "echo \"[$WINFER_PANE_ID $WINFER_WORKSPACE $TERM $PWD]\"",
        ]);

        let pair = native_pty_system().openpty(size(80, 24)).unwrap();
        let _child = pair.slave.spawn_command(cmd).unwrap();
        drop(pair.slave);
        let mut reader = pair.master.try_clone_reader().unwrap();

        // Read on a thread: EOF on a PTY master isn't reliable across platforms.
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let mut buf = [0u8; 4096];
            while let Ok(n) = reader.read(&mut buf) {
                if n == 0 || tx.send(buf[..n].to_vec()).is_err() {
                    break;
                }
            }
        });
        let mut out = String::new();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while !out.contains(']') && std::time::Instant::now() < deadline {
            if let Ok(chunk) = rx.recv_timeout(std::time::Duration::from_millis(100)) {
                out.push_str(&String::from_utf8_lossy(&chunk));
            }
        }
        let home = dirs::home_dir().unwrap();
        let expected = format!("[p-test ws-test xterm-256color {}]", home.display());
        assert!(out.contains(&expected), "output was: {out:?}");
    }

    #[test]
    fn mcp_config_never_swallows_the_prompt() {
        let claude = &providers::find("claude").unwrap();
        let args = agent_args(
            claude,
            vec!["--mcp-config".into(), "{}".into()],
            vec!["--settings".into(), "/hooks.json".into()],
            None,
            false,
            None,
            Some("fix the login bug"),
        );
        assert_eq!(
            args,
            [
                "--mcp-config",
                "{}",
                "--settings",
                "/hooks.json",
                "fix the login bug"
            ]
        );
        let resumed = agent_args(claude, vec![], vec![], Some("opus"), true, None, Some("ignored"));
        assert_eq!(resumed, ["--model", "opus", "--continue"]);
        let by_id = agent_args(claude, vec![], vec![], None, true, Some("abc-123"), None);
        assert_eq!(by_id, ["--resume", "abc-123"]);
        // A known session only matters when resuming.
        assert!(agent_args(claude, vec![], vec![], None, false, Some("abc-123"), None).is_empty());
        // OpenCode has no launch-time model flag and takes the prompt as an option.
        let opencode = &providers::find("opencode").unwrap();
        assert_eq!(agent_args(opencode, vec![], vec![], Some("opencode/big-pickle"), false, None, Some("fix it")), ["--prompt", "fix it"]);
        assert_eq!(agent_args(opencode, vec![], vec![], None, true, Some("ses_1"), None), ["--session", "ses_1"]);
        let codex = &providers::find("codex").unwrap();
        assert_eq!(
            agent_args(
                codex,
                vec!["-c".into(), "a=1".into()],
                vec![],
                Some("gpt-5.5"),
                true,
                None,
                None
            ),
            ["-c", "a=1", "--model", "gpt-5.5", "resume", "--last"]
        );
        assert_eq!(
            agent_args(codex, vec!["-c".into(), "a=1".into()], vec![], None, true, Some("0199-uuid"), None),
            ["-c", "a=1", "resume", "0199-uuid"]
        );
    }

    #[test]
    fn system_prompt_goes_in_a_flag_or_the_start_prompt() {
        let req = |provider: &str, prompt: Option<&str>, resume: bool| SpawnRequest {
            pane_id: "p".into(),
            provider_id: provider.into(),
            cwd: None,
            workspace_id: None,
            resume,
            session: None,
            prompt: prompt.map(Into::into),
            model: None,
            auto_approve: false,
            system_prompt: Some("Use your tools.".into()),
            plain: false,
            lean: false,
            hidden_tools: Vec::new(),
            cols: 80,
            rows: 24,
        };
        let claude = with_system_prompt(req("claude", Some("fix it"), false));
        assert_eq!(claude.system_prompt.as_deref(), Some("Use your tools."));
        assert_eq!(claude.prompt.as_deref(), Some("fix it"));
        assert_eq!(
            system_prompt_args(&providers::find("claude").unwrap(), "Use your tools."),
            ["--append-system-prompt", "Use your tools."]
        );
        assert_eq!(
            system_prompt_args(&providers::find("codex").unwrap(), "say \"hi\"\nnow"),
            ["-c", r#"developer_instructions="say \"hi\"\nnow""#]
        );
        // No flag: it leads the start prompt, or is the start prompt.
        let opencode = with_system_prompt(req("opencode", Some("fix it"), false));
        assert_eq!((opencode.system_prompt, opencode.prompt.as_deref()), (None, Some("Use your tools.\n\nfix it")));
        assert_eq!(with_system_prompt(req("opencode", None, false)).prompt.as_deref(), Some("Use your tools."));
        // A resumed session already had it; shells and plain runs never get it.
        assert_eq!(with_system_prompt(req("opencode", None, true)).prompt, None);
        assert_eq!(with_system_prompt(req("shell", None, false)).prompt, None);
        let mut plain = req("claude", None, false);
        plain.plain = true;
        assert_eq!(with_system_prompt(plain).system_prompt, None);
    }

    #[test]
    fn plain_runs_skip_the_users_mcp_servers() {
        assert_eq!(plain_args("claude"), ["--strict-mcp-config", "--disable-slash-commands"]);
        assert_eq!(plain_args("codex"), ["-c", "mcp_servers={}"]);
        assert!(plain_args("opencode").is_empty());
    }

    #[test]
    fn codex_output_cap_yields_to_the_users_config() {
        assert!(!sets_output_limit("model = \"gpt-5\"\n# tool_output_token_limit = 1\n"));
        assert!(sets_output_limit("tool_output_token_limit = 20000\n"));
        assert!(sets_output_limit("[profiles.big]\n  tool_output_token_limit=50000\n"));
        assert!(!sets_output_limit("tool_output_token_limit_extra = 1\n"));
    }

    #[test]
    fn output_caps_leave_the_users_own_alone() {
        let none = HashMap::new();
        assert_eq!(output_caps("claude", &none).len(), 3);
        assert_eq!(output_caps("winfer", &none).len(), 3);
        assert!(output_caps("codex", &none).is_empty());
        // Windows environment names ignore case.
        let own = HashMap::from([("Bash_Max_Output_Length".to_string(), "50000".to_string())]);
        let caps = output_caps("claude", &own);
        assert_eq!(caps.len(), 2);
        assert!(caps.iter().all(|(k, _)| k != "BASH_MAX_OUTPUT_LENGTH"));
    }

    #[test]
    fn lean_codex_drops_instructions_and_can_swap_the_prompt() {
        let args = lean_codex_args(false);
        assert!(args.windows(2).any(|w| w == ["-c", "include_permissions_instructions=false"]));
        assert!(args.windows(2).any(|w| w == ["-c", "features.apps=false"]));
        assert!(!args.iter().any(|a| a.starts_with("model_instructions_file")));
        let short = lean_codex_args(true);
        let file = short.iter().find_map(|a| a.strip_prefix("model_instructions_file=")).expect("short prompt");
        let path: String = serde_json::from_str(file).unwrap();
        assert_eq!(std::fs::read_to_string(path).unwrap(), crate::harness::backend::CODEX_LEAN_PROMPT);
    }

    #[test]
    fn lean_codex_turns_off_bundled_extras() {
        let args = lean_args("codex");
        assert!(args.chunks(2).all(|p| p[0] == "-c"));
        assert!(args.contains(&"features.apps=false".to_string()));
        assert!(args.contains(&"skills.bundled.enabled=false".to_string()));
        assert!(!args.iter().any(|a| a.contains("code_mode_host")));
        assert!(lean_args("claude").is_empty());
    }

    #[test]
    fn login_env_has_path() {
        assert!(login_env().contains_key("PATH"));
    }

    #[test]
    fn unknown_provider_is_rejected() {
        let req = SpawnRequest {
            pane_id: "p".into(),
            provider_id: "nope".into(),
            cwd: None,
            workspace_id: None,
            resume: false,
            session: None,
            prompt: None,
            model: None,
            auto_approve: false,
            system_prompt: None,
            plain: false,
            lean: false,
            hidden_tools: Vec::new(),
            cols: 80,
            rows: 24,
        };
        assert!(build_command(&req, None).is_err());
    }
}
