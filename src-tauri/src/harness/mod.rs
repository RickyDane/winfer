//! `winfer code`: winfer's own coding agent, a terminal UI over three interchangeable backends:
//! the ChatGPT account from Settings → AI (winfer's own tools), Claude Code (`claude -p`
//! stream-json) and Codex (`codex app-server`), each with its own tools and login.

pub mod backend;
pub mod claude_skills;
pub mod config;
pub mod diff;
pub mod event;
pub mod mcp_client;
pub mod session;
pub mod tools;
pub mod tui;

use std::io::Write;
use std::sync::mpsc::channel;

use event::{Cmd, Decision, Ev, Mode, UserMsg};

/// Settings shown in the app (Settings → winfer code); history stays out.
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    model: String,
    effort: Option<String>,
    mode: Mode,
    trim_at: u8,
    compact_at: u8,
    allowed_commands: Vec<String>,
    mcp_servers: serde_json::Map<String, serde_json::Value>,
    lean_bridges: bool,
    lean_all_skills: bool,
    lean_skills: Vec<String>,
    plan_sidebar: bool,
}

#[tauri::command]
pub fn harness_settings() -> Settings {
    let c = config::Config::load();
    Settings {
        model: c.model,
        effort: c.effort,
        mode: c.mode,
        trim_at: c.trim_at,
        compact_at: c.compact_at,
        allowed_commands: c.allowed_commands,
        mcp_servers: c.mcp_servers,
        lean_bridges: c.lean_bridges,
        lean_all_skills: c.lean_all_skills,
        lean_skills: c.lean_skills,
        plan_sidebar: c.plan_sidebar,
    }
}

/// The skills Claude Code can load from the user's setup, to pick from for lean mode.
#[tauri::command]
pub fn harness_claude_skills() -> Vec<claude_skills::ClaudeSkill> {
    claude_skills::available()
}

#[tauri::command]
pub fn harness_set_settings(settings: Settings) -> Result<(), String> {
    if settings.trim_at > 99 || settings.compact_at > 99 {
        return Err("Percentages go up to 99.".into());
    }
    config::Config::update(|c| {
        c.model = settings.model;
        c.effort = settings.effort.filter(|e| !e.is_empty());
        c.mode = settings.mode;
        c.trim_at = settings.trim_at;
        c.compact_at = settings.compact_at;
        c.allowed_commands = settings.allowed_commands.into_iter().map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
        c.mcp_servers = settings.mcp_servers;
        c.lean_bridges = settings.lean_bridges;
        c.lean_all_skills = settings.lean_all_skills;
        c.lean_skills = settings.lean_skills;
        c.plan_sidebar = settings.plan_sidebar;
    });
    Ok(())
}

#[derive(serde::Serialize)]
pub struct HarnessModel {
    spec: String,
    label: String,
    detail: String,
}

/// Every model `winfer code` can use (asks Codex and the ChatGPT account: a few seconds).
#[tauri::command]
pub async fn harness_models() -> Vec<HarnessModel> {
    tauri::async_runtime::spawn_blocking(|| {
        backend::all_models().into_iter().map(|m| HarnessModel { spec: m.spec, label: m.label, detail: m.detail }).collect()
    })
    .await
    .unwrap_or_default()
}

/// `--full-prompt` for this process (the TUI reads lean mode from the saved config otherwise).
pub static LEAN_OFF: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

const HELP: &str = "winfer code — a coding agent for ChatGPT, Claude and Codex models

Usage: winfer code [options] [prompt]

Options:
  -m, --model <backend/model>  chatgpt/<model>, claude/<opus|sonnet|haiku|fable>, codex/<model>
  -r, --resume <id>            Resume a saved session
  -c, --continue               Resume the latest session in this directory
  -p, --print                  Run the prompt without the UI and print the answer
      --json                   With --print: one JSON event per line
      --mode <ask|auto-edit|plan|yolo>
      --yolo                   Same as --mode yolo
      --effort <level>         Reasoning effort
      --append-system-prompt <text>
      --list-models            Print available models (id<TAB>label)
      --no-mouse               Don't capture the mouse (keeps terminal text selection)
      --full-prompt            Claude Code with its full default prompt and tools
  -h, --help";

pub fn run(args: &[String]) {
    crate::env::use_process_env();
    let mut launch = tui::app::Launch { model: String::new(), resume: None, cont: false, prompt: None, mode: None, system: None };
    let mut print = false;
    let mut json = false;
    let mut mouse = true;
    let mut full_prompt = false;
    let mut effort = None;
    let mut words = Vec::new();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "-m" | "--model" => launch.model = it.next().cloned().unwrap_or_default(),
            "-r" | "--resume" => launch.resume = it.next().cloned(),
            "-c" | "--continue" => launch.cont = true,
            "-p" | "--print" => print = true,
            "--json" => json = true,
            "--yolo" => launch.mode = Some(Mode::Yolo),
            "--mode" => launch.mode = it.next().and_then(|m| serde_json::from_value(serde_json::json!(m)).ok()),
            "--effort" => effort = it.next().cloned(),
            "--append-system-prompt" | "--system" => launch.system = it.next().cloned(),
            "--no-mouse" => mouse = false,
            "--full-prompt" => {
                full_prompt = true;
                LEAN_OFF.store(true, std::sync::atomic::Ordering::Relaxed);
            }
            "--list-models" => {
                for m in backend::all_models() {
                    println!("{}\t{}", m.spec, m.label);
                }
                return;
            }
            "-h" | "--help" => {
                println!("{HELP}");
                return;
            }
            "-V" | "--version" => {
                println!("winfer code {}", env!("CARGO_PKG_VERSION"));
                return;
            }
            _ => words.push(a.clone()),
        }
    }
    if !words.is_empty() {
        launch.prompt = Some(words.join(" "));
    }
    if let Some(e) = effort {
        let mut c = config::Config::load();
        c.effort = Some(e).filter(|e| e != "default");
        c.save();
    }
    if print {
        std::process::exit(headless(launch, json, full_prompt));
    }
    if let Err(e) = tui::run(launch, mouse) {
        eprintln!("winfer code: {e}");
        std::process::exit(1);
    }
}

/// `--print`: one turn without the UI. Tool approvals are declined unless the mode allows them.
fn headless(launch: tui::app::Launch, json: bool, full_prompt: bool) -> i32 {
    let mut prompt = launch.prompt.unwrap_or_default();
    if prompt.is_empty() || prompt == "-" {
        let _ = std::io::Read::read_to_string(&mut std::io::stdin(), &mut prompt);
    }
    if prompt.trim().is_empty() {
        eprintln!("winfer code: no prompt");
        return 2;
    }
    let cfg = config::Config::load();
    let spec = if launch.model.is_empty() { cfg.model.clone() } else { launch.model };
    let (kind, model) = backend::parse_spec(&spec);
    let cwd = std::env::current_dir().unwrap_or_default();
    let mode = launch.mode.unwrap_or(Mode::Ask);
    let (tx, rx) = channel();
    let handle = backend::start(
        kind,
        backend::Opts {
            cwd,
            model,
            effort: cfg.effort.clone(),
            mode,
            resume: None,
            system: launch.system,
            mcp: config::mcp_servers(&cfg),
            allowed_commands: cfg.allowed_commands.clone(),
            trim_at: cfg.trim_at,
            compact_at: cfg.compact_at,
            lean: cfg.lean_bridges && !full_prompt,
            skills: cfg.lean_skills(),
        },
        tx,
    );
    handle.send(Cmd::Send(UserMsg { text: prompt, images: Vec::new() }));
    let mut out = std::io::stdout();
    let mut code = 0;
    let mut wrote = false;
    // Background subagents still running, and whether the turn is over: the run ends once both
    // are done, and after a last report the backend may start a turn on it.
    let mut running: std::collections::HashSet<String> = Default::default();
    let mut idle = false;
    loop {
        let ev = if idle && running.is_empty() {
            match rx.recv_timeout(std::time::Duration::from_secs(3)) {
                Ok(ev) => ev,
                Err(_) => break,
            }
        } else {
            match rx.recv() {
                Ok(ev) => ev,
                Err(_) => break,
            }
        };
        if json {
            let _ = writeln!(out, "{}", event_json(&ev));
        }
        match ev {
            Ev::TextDelta { text, .. } if !json => {
                wrote = true;
                let _ = write!(out, "{text}");
                let _ = out.flush();
            }
            Ev::Text { text, .. } if !json && !wrote => {
                let _ = write!(out, "{text}");
            }
            Ev::Text { .. } if !json => {
                let _ = writeln!(out);
                wrote = false;
            }
            Ev::ToolStart { name, title, .. } if !json => eprintln!("\x1b[2m● {name} {title}\x1b[0m"),
            Ev::Approval(a) => {
                if !json {
                    eprintln!("\x1b[33m✗ declined {}: {} (use --mode auto-edit or --yolo)\x1b[0m", a.tool, a.title);
                }
                handle.send(Cmd::Approve(a.id, Decision::Deny));
            }
            Ev::Error(e) if !json => {
                eprintln!("\x1b[31m{e}\x1b[0m");
                code = 1;
            }
            Ev::Exited(e) => {
                if !json {
                    eprintln!("\x1b[31m{e}\x1b[0m");
                }
                return 1;
            }
            Ev::TurnStart => idle = false,
            Ev::Task(t) => match t.status.as_deref() {
                Some("running" | "pending") => {
                    running.insert(t.id);
                }
                Some(_) => {
                    running.remove(&t.id);
                }
                None => {}
            },
            Ev::TurnDone if running.is_empty() => break,
            Ev::TurnDone => idle = true,
            _ => {}
        }
    }
    if !json {
        let _ = writeln!(out);
    }
    handle.send(Cmd::Shutdown);
    code
}

fn event_json(ev: &Ev) -> serde_json::Value {
    use serde_json::json;
    match ev {
        Ev::Session(s) => json!({ "type": "session", "id": s }),
        Ev::Model(m) => json!({ "type": "model", "model": m }),
        Ev::TextDelta { id, text } => json!({ "type": "text_delta", "id": id, "text": text }),
        Ev::Text { id, text } => json!({ "type": "text", "id": id, "text": text }),
        Ev::ReasoningDelta { id, text } => json!({ "type": "reasoning_delta", "id": id, "text": text }),
        Ev::ToolStart { id, name, title, input } => json!({ "type": "tool_start", "id": id, "name": name, "title": title, "input": input }),
        Ev::ToolOutput { id, chunk } => json!({ "type": "tool_output", "id": id, "chunk": chunk }),
        Ev::ToolEnd { id, ok, output, diff } => json!({ "type": "tool_end", "id": id, "ok": ok, "output": output, "diff": diff }),
        Ev::Approval(a) => json!({ "type": "approval", "id": a.id, "tool": a.tool, "title": a.title }),
        Ev::Task(t) => json!({ "type": "task", "id": t.id, "tool_id": t.tool_id, "title": t.title, "agent": t.agent, "tools": t.tools, "last": t.last, "status": t.status }),
        Ev::Todos(t) => json!({ "type": "todos", "todos": t }),
        Ev::Usage(u) => json!({ "type": "usage", "context": u.context, "window": u.window, "input": u.input, "cached": u.cached, "output": u.output, "cost": u.cost }),
        Ev::Notice(n) => json!({ "type": "notice", "text": n }),
        Ev::Error(e) => json!({ "type": "error", "text": e }),
        Ev::TurnStart => json!({ "type": "turn_start" }),
        Ev::TurnDone => json!({ "type": "turn_done" }),
        Ev::Exited(e) => json!({ "type": "exited", "text": e }),
    }
}
