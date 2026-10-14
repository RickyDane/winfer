//! The LLM behind voice "smart moves": plans long or complex voice commands, turns what the user
//! said into the message an agent receives, estimates progress and summarizes finished work.
//!
//! It runs the user's own coding-agent CLI headless, with its own login: Claude Code
//! (`claude -p`, no tools, no settings or MCP servers) or Codex (`codex exec`, read-only and
//! ephemeral). Nothing else is called.

use std::io::{Read, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde::Deserialize;

use crate::env::login_env;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ComposeRequest {
    /// "claude" or "codex".
    provider: String,
    /// Model for the CLI (e.g. "sonnet", "gpt-5"); empty uses its default.
    #[serde(default)]
    model: String,
    system: String,
    user: String,
}

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

/// Codex with the user's ChatGPT login, read-only and ephemeral.
fn codex(model: &str, system: &str, user: &str) -> Result<String, String> {
    let launcher = crate::env::launcher("codex").ok_or("Codex CLI isn't installed.")?;
    let dir = scratch_dir()?;
    let out = dir.join("answer.txt");
    let mut cmd = Command::new(&launcher.program);
    cmd.args(&launcher.args)
        .current_dir(&dir)
        .args(["exec", "--skip-git-repo-check", "--ephemeral", "-s", "read-only", "-o"])
        .arg(&out);
    if !model.is_empty() {
        cmd.args(["-m", model]);
    }
    cmd.arg("-");
    let result = run(cmd, &format!("{system}\n\n{user}"), "Codex");
    let answer = std::fs::read_to_string(&out).unwrap_or_default();
    let _ = std::fs::remove_dir_all(&dir);
    result?;
    if answer.trim().is_empty() {
        return Err("Codex gave no answer".into());
    }
    Ok(answer.trim().to_string())
}

#[tauri::command]
pub async fn llm_compose(req: ComposeRequest) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || match req.provider.as_str() {
        "claude" => claude(&req.model, &req.system, &req.user),
        "codex" => codex(&req.model, &req.system, &req.user),
        other => Err(format!("unknown LLM provider {other}")),
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

    /// Live: a logged-in Claude Code and Codex.
    /// cargo test --lib -- --ignored --nocapture live_compose
    #[test]
    #[ignore]
    fn live_compose() {
        let system = "Rewrite as a one-sentence instruction for a coding agent. Output only it.";
        for (provider, run) in [("claude", claude as fn(&str, &str, &str) -> Result<String, String>), ("codex", codex)] {
            let started = Instant::now();
            let result = run("", system, "Spoken: make the button blue");
            eprintln!("{provider}: {result:?} in {:?}", started.elapsed());
            assert!(result.is_ok());
        }
    }
}
