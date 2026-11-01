//! The LLM behind voice "smart moves": plans long or complex voice commands, turns what the user
//! said into the message an agent receives, estimates progress and summarizes finished work.
//!
//! Three backends, all with the user's own subscription: ChatGPT through winfer's Sign in with
//! ChatGPT (the Responses API, see chatgpt.rs), the Codex CLI run headless with its own login
//! (`codex exec`, read-only sandbox, no user config), or Claude Code run headless (`claude -p`,
//! no tools, no settings or MCP servers).

use std::io::{Read, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde::Deserialize;

use crate::env::login_env;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ComposeRequest {
    /// "chatgpt", "codex" or "claude".
    provider: String,
    /// Model (e.g. "gpt-6-luna", "sonnet"); empty uses the default.
    #[serde(default)]
    model: String,
    system: String,
    user: String,
    /// A tiny job (a session title): the quickest model wins over the picked one.
    #[serde(default)]
    fast: bool,
}

/// Claude Code's quickest model, for tiny jobs like session titles.
const CLAUDE_FAST_MODEL: &str = "haiku";
/// Codex's quickest model, for the same jobs.
const CODEX_FAST_MODEL: &str = "gpt-6-luna";
/// Muse's quickest model, for the same jobs.
const MUSE_FAST_MODEL: &str = "muse-spark-1.3";

/// Runs a prepared CLI command with `input` on stdin, in a scratch directory, and waits for it.
/// Returns stdout. The prompt goes in on stdin: it's multi-line, which a command-line argument
/// can't carry through Windows' .cmd shims.
fn run(mut cmd: Command, input: &str, name: &str) -> Result<String, String> {
    crate::env::no_window(&mut cmd);
    cmd.envs(login_env())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd.spawn().map_err(|e| format!("couldn't run {name}: {e}"))?;
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(input.as_bytes());
        // Dropping stdin closes it, so the CLI knows the prompt is complete.
    }
    // Keep reading both pipes so a full pipe never blocks the CLI.
    let drain = |pipe: Option<Box<dyn Read + Send>>| {
        std::thread::spawn(move || {
            let mut text = String::new();
            if let Some(mut pipe) = pipe {
                let _ = pipe.read_to_string(&mut text);
            }
            text
        })
    };
    let stdout = drain(child.stdout.take().map(|p| Box::new(p) as Box<dyn Read + Send>));
    let stderr = drain(child.stderr.take().map(|p| Box::new(p) as Box<dyn Read + Send>));
    let deadline = Instant::now() + Duration::from_secs(120);
    let status = loop {
        if let Some(status) = child.try_wait().map_err(|e| e.to_string())? {
            break status;
        }
        if Instant::now() > deadline {
            let _ = child.kill();
            return Err(format!("{name} took too long"));
        }
        std::thread::sleep(Duration::from_millis(100));
    };
    let out = stdout.join().unwrap_or_default();
    if !status.success() {
        let err = stderr.join().unwrap_or_default();
        let last = err.lines().chain(out.lines()).rev().find(|l| !l.trim().is_empty()).unwrap_or("no answer");
        return Err(format!("{name} failed: {last}"));
    }
    Ok(out)
}

fn scratch_dir() -> Result<std::path::PathBuf, String> {
    let dir = std::env::temp_dir().join(format!("winfer-llm-{:x}", rand::random::<u64>()));
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(dir)
}

/// Claude Code in print mode with the user's Claude login: no tools, no MCP servers, no
/// user/project settings (so their hooks and output styles don't shape the answer).
fn claude_args(dir: &Path, model: &str) -> Vec<String> {
    let mut args: Vec<String> = [
        "-p",
        "--output-format",
        "text",
        "--no-session-persistence",
        "--tools",
        "",
        "--strict-mcp-config",
        "--setting-sources",
        "",
        "--system-prompt-file",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    args.push(dir.join("system.txt").to_string_lossy().into_owned());
    if !model.is_empty() {
        args.extend(["--model".into(), model.into()]);
    }
    args
}

fn claude(model: &str, system: &str, user: &str) -> Result<String, String> {
    let launcher = crate::env::launcher("claude").ok_or("Claude Code isn't installed.")?;
    let dir = scratch_dir()?;
    let result = std::fs::write(dir.join("system.txt"), system).map_err(|e| e.to_string()).and_then(|_| {
        let mut cmd = Command::new(&launcher.program);
        cmd.args(&launcher.args).args(claude_args(&dir, model)).current_dir(&dir);
        run(cmd, user, "Claude Code")
    });
    let _ = std::fs::remove_dir_all(&dir);
    let answer = result?;
    let answer = answer.trim();
    if answer.is_empty() {
        return Err("Claude Code gave no answer".into());
    }
    Ok(answer.to_string())
}

/// Codex in exec mode with the user's Codex login: read-only sandbox in an empty scratch
/// directory, no session files, no user config or rules. The system prompt replaces Codex's own
/// instructions; the last message is read from a file, so progress output can't leak in.
fn codex_args(dir: &Path, model: &str) -> Vec<String> {
    let mut args: Vec<String> = [
        "exec",
        "--skip-git-repo-check",
        "--ephemeral",
        "--ignore-user-config",
        "--ignore-rules",
        "--sandbox",
        "read-only",
        "--color",
        "never",
        "-c",
        "model_reasoning_effort=\"low\"",
        "-c",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    // A TOML literal string: Windows paths keep their backslashes.
    args.push(format!("model_instructions_file='{}'", dir.join("system.txt").to_string_lossy()));
    args.extend(["--output-last-message".into(), dir.join("answer.txt").to_string_lossy().into_owned()]);
    // Like the interactive Codex backend, `-fast` selects a service tier, not a model ID.
    let model = match model.strip_suffix("-fast") {
        Some(base) if !base.is_empty() => {
            args.extend(["-c".into(), "service_tier=\"priority\"".into()]);
            base
        }
        _ => model,
    };
    if !model.is_empty() {
        args.extend(["--model".into(), model.into()]);
    }
    // The prompt comes in on stdin.
    args.push("-".into());
    args
}

fn codex(model: &str, system: &str, user: &str) -> Result<String, String> {
    let launcher = crate::env::launcher("codex").ok_or("Codex isn't installed.")?;
    let dir = scratch_dir()?;
    let result = std::fs::write(dir.join("system.txt"), system).map_err(|e| e.to_string()).and_then(|_| {
        let mut cmd = Command::new(&launcher.program);
        cmd.args(&launcher.args).args(codex_args(&dir, model)).current_dir(&dir);
        run(cmd, user, "Codex")?;
        std::fs::read_to_string(dir.join("answer.txt")).map_err(|_| "Codex gave no answer".to_string())
    });
    let _ = std::fs::remove_dir_all(&dir);
    let answer = result?;
    let answer = answer.trim();
    if answer.is_empty() {
        return Err("Codex gave no answer".into());
    }
    Ok(answer.to_string())
}

/// Muse in exec mode with the user's Muse login: no workspace writes or shell, and the
/// answer read from its JSONL stream's output deltas (falling back to the terminal text).
fn muse_args(dir: &Path, model: &str) -> Vec<String> {
    let mut args: Vec<String> = [
        "exec",
        "--json",
        "--disable-write",
        "--disable-shell",
        "--disable-web-tools",
        "--approval-mode",
        "never",
        "--prompt-file",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    args.push(dir.join("prompt.txt").to_string_lossy().into_owned());
    if !model.is_empty() {
        args.extend(["--model".into(), model.into()]);
    }
    args
}

fn muse(model: &str, system: &str, user: &str) -> Result<String, String> {
    let launcher = crate::env::launcher("muse").ok_or("Muse isn't installed.")?;
    let dir = scratch_dir()?;
    let prompt = format!("{system}\n\n{user}");
    let result = std::fs::write(dir.join("prompt.txt"), &prompt).map_err(|e| e.to_string()).and_then(|_| {
        let mut cmd = Command::new(&launcher.program);
        cmd.args(&launcher.args).args(muse_args(&dir, model)).current_dir(&dir);
        run(cmd, "", "Muse")
    });
    let _ = std::fs::remove_dir_all(&dir);
    let out = result?;
    let mut answer = String::new();
    let mut terminal = String::new();
    for line in out.lines() {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else { continue };
        match v["payload_type"].as_str() {
            Some("run.output.delta") => {
                if let Some(t) = v["payload"]["text"].as_str() {
                    answer.push_str(t);
                }
            }
            Some("run.terminal.completed") => {
                if let Some(t) = v["payload"]["text"].as_str() {
                    terminal = t.to_string();
                }
            }
            _ => {}
        }
    }
    let answer = if answer.trim().is_empty() { terminal } else { answer };
    let answer = answer.trim();
    if answer.is_empty() {
        return Err("Muse gave no answer".into());
    }
    Ok(answer.to_string())
}

/// One blocking, tool-less answer from `provider` ("claude", "codex", "chatgpt" or "muse")
/// with the user's own login. An empty model uses the provider's default.
pub fn respond(provider: &str, model: &str, system: &str, user: &str) -> Result<String, String> {
    match provider {
        "claude" => claude(model, system, user),
        "codex" => codex(model, system, user),
        "chatgpt" => crate::chatgpt::respond(model, system, user),
        "muse" => muse(model, system, user),
        other => Err(format!("unknown LLM provider {other}")),
    }
}

#[tauri::command]
pub async fn llm_compose(req: ComposeRequest) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let model = match (req.fast, req.provider.as_str()) {
            (true, "claude") => CLAUDE_FAST_MODEL,
            (true, "codex") => CODEX_FAST_MODEL,
            (true, "muse") => MUSE_FAST_MODEL,
            _ => &req.model,
        };
        respond(&req.provider, model, &req.system, &req.user)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claude_runs_without_tools_settings_or_mcp() {
        let args = claude_args(Path::new("/tmp/x"), "sonnet");
        let after = |flag: &str| args.iter().position(|a| a == flag).map(|i| args[i + 1].as_str());
        assert_eq!(args[0], "-p");
        assert_eq!(after("--tools"), Some(""));
        assert_eq!(after("--setting-sources"), Some(""));
        assert_eq!(after("--model"), Some("sonnet"));
        assert!(args.contains(&"--strict-mcp-config".to_string()));
        assert!(after("--system-prompt-file").unwrap().ends_with("system.txt"));
        assert!(!claude_args(Path::new("/tmp/x"), "").contains(&"--model".to_string()));
    }

    #[test]
    fn muse_runs_tool_less_and_reads_its_prompt_from_a_file() {
        let args = muse_args(Path::new("/tmp/x"), "muse-spark-1.3");
        let after = |flag: &str| args.iter().position(|a| a == flag).map(|i| args[i + 1].as_str());
        assert_eq!(args[0], "exec");
        assert!(args.contains(&"--json".to_string()));
        assert!(args.contains(&"--disable-write".to_string()));
        assert!(args.contains(&"--disable-shell".to_string()));
        assert_eq!(after("--approval-mode"), Some("never"));
        assert_eq!(after("--model"), Some("muse-spark-1.3"));
        assert!(after("--prompt-file").unwrap().ends_with("prompt.txt"));
        assert!(!muse_args(Path::new("/tmp/x"), "").contains(&"--model".to_string()));
    }

    #[test]
    fn codex_runs_read_only_without_user_config() {
        let args = codex_args(Path::new("/tmp/x"), "gpt-6.1-sol");
        let after = |flag: &str| args.iter().position(|a| a == flag).map(|i| args[i + 1].as_str());
        assert_eq!(args[0], "exec");
        assert_eq!(after("--sandbox"), Some("read-only"));
        assert_eq!(after("--model"), Some("gpt-6.1-sol"));
        assert!(args.contains(&"--ignore-user-config".to_string()));
        assert!(args.contains(&"--ephemeral".to_string()));
        assert!(args.iter().any(|a| a.starts_with("model_instructions_file='") && a.ends_with("system.txt'")));
        assert!(after("--output-last-message").unwrap().ends_with("answer.txt"));
        assert_eq!(args.last().map(String::as_str), Some("-"));
        assert!(!codex_args(Path::new("/tmp/x"), "").contains(&"--model".to_string()));
    }

    #[test]
    fn codex_fast_uses_the_base_model_and_priority_tier() {
        let args = codex_args(Path::new("/tmp/x"), "gpt-6.1-sol-fast");
        let model = args.iter().position(|a| a == "--model").unwrap();
        assert_eq!(args[model + 1], "gpt-6.1-sol");
        assert!(args.windows(2).any(|a| a[0] == "-c" && a[1] == "service_tier=\"priority\""));
        assert_eq!(args.last().map(String::as_str), Some("-"));
        for model in ["gpt-6.1-sol", "", "-fast"] {
            let args = codex_args(Path::new("/tmp/x"), model);
            assert!(!args.iter().any(|a| a.starts_with("service_tier=")));
        }
    }

    /// Live: a logged-in Claude Code and Codex, and a ChatGPT sign-in made in winfer.
    /// cargo test --lib -- --ignored --nocapture live_compose
    #[test]
    #[ignore]
    fn live_compose() {
        let system = "Rewrite as a one-sentence instruction for a coding agent. Output only it.";
        for (provider, run) in [("claude", claude as fn(&str, &str, &str) -> Result<String, String>), ("codex", codex), ("chatgpt", crate::chatgpt::respond)] {
            let started = Instant::now();
            let result = run("", system, "Spoken: make the button blue");
            eprintln!("{provider}: {result:?} in {:?}", started.elapsed());
            assert!(result.is_ok());
        }
    }
}
