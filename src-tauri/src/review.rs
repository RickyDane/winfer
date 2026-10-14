//! Checks for the review inbox: finds a project's test command and runs it in an agent's
//! worktree, so a finished agent's card can say whether its change passes.

use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde::Serialize;

use crate::env::login_env;

/// Long enough for real suites; a hung watcher must not block the card forever.
const TIMEOUT: Duration = Duration::from_secs(600);
/// Only the end of the output matters for a failing run.
const OUTPUT_TAIL: usize = 6000;

/// The test command a developer would run in `dir`, if the project declares one.
pub fn test_command(dir: &Path) -> Option<String> {
    let has = |name: &str| dir.join(name).exists();
    if let Ok(text) = std::fs::read_to_string(dir.join("package.json")) {
        let json: serde_json::Value = serde_json::from_str(&text).unwrap_or_default();
        let script = json["scripts"]["test"].as_str().unwrap_or_default();
        // `npm init` writes a placeholder test script that always fails.
        if !script.is_empty() && !script.contains("no test specified") {
            let runner = if has("pnpm-lock.yaml") {
                "pnpm"
            } else if has("yarn.lock") {
                "yarn"
            } else if has("bun.lockb") || has("bun.lock") {
                "bun run"
            } else {
                "npm"
            };
            return Some(format!("{runner} test"));
        }
    }
    if has("Cargo.toml") {
        return Some("cargo test".into());
    }
    if has("go.mod") {
        return Some("go test ./...".into());
    }
    let python = ["pyproject.toml", "pytest.ini", "setup.cfg", "tox.ini"]
        .iter()
        .any(|f| std::fs::read_to_string(dir.join(f)).is_ok_and(|t| t.contains("pytest")));
    if python || dir.join("tests").join("conftest.py").exists() {
        return Some("pytest -q".into());
    }
    if std::fs::read_to_string(dir.join("Makefile"))
        .is_ok_and(|t| t.lines().any(|l| l.starts_with("test:")))
    {
        return Some("make test".into());
    }
    None
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TestRun {
    command: String,
    ok: bool,
    /// Last part of stdout + stderr.
    output: String,
    ms: u64,
}

fn run(dir: &Path, command: &str) -> Result<TestRun, String> {
    let mut cmd = if cfg!(windows) {
        let mut c = Command::new("cmd");
        c.args(["/C", &format!("{command} 2>&1")]);
        c
    } else {
        let mut c = Command::new("sh");
        c.args(["-c", &format!("{command} 2>&1")]);
        c
    };
    cmd.current_dir(dir)
        .envs(login_env())
        // Keeps vitest/jest out of watch mode and tools out of interactive prompts.
        .env("CI", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    // Its own process group, so a timeout also stops the test runner's children.
    #[cfg(unix)]
    std::os::unix::process::CommandExt::process_group(&mut cmd, 0);

    let started = Instant::now();
    let mut child = cmd.spawn().map_err(|e| format!("couldn't run {command}: {e}"))?;
    let mut stdout = child.stdout.take().ok_or("no output pipe")?;
    let reader = std::thread::spawn(move || {
        let mut out = Vec::new();
        let _ = stdout.read_to_end(&mut out);
        String::from_utf8_lossy(&out).into_owned()
    });
    let status = loop {
        if let Some(status) = child.try_wait().map_err(|e| e.to_string())? {
            break Some(status);
        }
        if started.elapsed() > TIMEOUT {
            #[cfg(unix)]
            unsafe {
                libc::kill(-(child.id() as i32), libc::SIGKILL);
            }
            let _ = child.kill();
            let _ = child.wait();
            break None;
        }
        std::thread::sleep(Duration::from_millis(100));
    };
    let output = reader.join().unwrap_or_default();
    let tail = match output.char_indices().nth_back(OUTPUT_TAIL) {
        Some((i, _)) => format!("…{}", &output[i..]),
        None => output,
    };
    Ok(TestRun {
        command: command.to_string(),
        ok: status.is_some_and(|s| s.success()),
        output: match status {
            Some(_) => tail,
            None => format!("{tail}\n[stopped after {} minutes]", TIMEOUT.as_secs() / 60),
        },
        ms: started.elapsed().as_millis() as u64,
    })
}

#[tauri::command]
pub fn review_test_command(dir: String) -> Option<String> {
    test_command(Path::new(&dir))
}

/// Runs the project's tests in `dir`; `Ok(None)` when there's no test command.
#[tauri::command]
pub async fn review_run_tests(dir: String) -> Result<Option<TestRun>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let dir = Path::new(&dir);
        match test_command(dir) {
            Some(command) => run(dir, &command).map(Some),
            None => Ok(None),
        }
    })
    .await
    .map_err(|e| e.to_string())?
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project(files: &[(&str, &str)]) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("winfer-review-{:x}", rand::random::<u64>()));
        for (name, text) in files {
            let path = dir.join(name);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn detects_test_commands() {
        let pnpm = project(&[
            ("package.json", r#"{"scripts":{"test":"vitest run"}}"#),
            ("pnpm-lock.yaml", ""),
        ]);
        assert_eq!(test_command(&pnpm).as_deref(), Some("pnpm test"));
        let placeholder = project(&[(
            "package.json",
            r#"{"scripts":{"test":"echo \"Error: no test specified\" && exit 1"}}"#,
        )]);
        assert_eq!(test_command(&placeholder), None);
        let rust = project(&[("Cargo.toml", "[package]")]);
        assert_eq!(test_command(&rust).as_deref(), Some("cargo test"));
        let python = project(&[("pyproject.toml", "[tool.pytest.ini_options]")]);
        assert_eq!(test_command(&python).as_deref(), Some("pytest -q"));
        let make = project(&[("Makefile", "build:\n\ttrue\ntest:\n\ttrue\n")]);
        assert_eq!(test_command(&make).as_deref(), Some("make test"));
        assert_eq!(test_command(&project(&[])), None);
    }

    #[cfg(unix)]
    #[test]
    fn runs_tests_and_reports_failures() {
        let passing = project(&[("Makefile", "test:\n\t@echo all good\n")]);
        let run_ok = run(&passing, "make test").unwrap();
        assert!(run_ok.ok, "{}", run_ok.output);
        assert!(run_ok.output.contains("all good"));

        let failing = project(&[("Makefile", "test:\n\t@echo boom >&2; exit 3\n")]);
        let run_err = run(&failing, "make test").unwrap();
        assert!(!run_err.ok);
        assert!(run_err.output.contains("boom"), "stderr is captured: {}", run_err.output);
    }
}
