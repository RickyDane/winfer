//! Resolves the environment agent processes should run with.
//!
//! GUI apps on macOS are launched by launchd and don't inherit the user's shell PATH,
//! so `claude`, `codex`, … installed via npm/brew/~/.local/bin would not be found.
//! We run the login shell once and capture its environment.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::OnceLock;

static LOGIN_ENV: OnceLock<HashMap<String, String>> = OnceLock::new();

/// Environment of an interactive login shell, falling back to this process's env.
pub fn login_env() -> &'static HashMap<String, String> {
    LOGIN_ENV.get_or_init(|| resolve().unwrap_or_else(|| std::env::vars().collect()))
}

/// The login PATH. Looked up case-insensitively: Windows spells it `Path`.
pub fn path_var() -> String {
    login_env()
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case("PATH"))
        .map(|(_, value)| value.clone())
        .unwrap_or_default()
}

/// Full path of a program on the login PATH. On Windows this also finds npm shims like
/// `codex.cmd`, which `Command::new("codex")` would miss.
pub fn find_command(command: &str) -> Option<PathBuf> {
    let cwd = dirs::home_dir().unwrap_or_default();
    which::which_in(command, Some(path_var()), cwd).ok()
}

/// How to start a CLI found on PATH.
#[derive(Debug, Clone, PartialEq)]
pub struct Launcher {
    pub program: PathBuf,
    /// Arguments that go before the CLI's own (the script, when a shim was unwrapped).
    pub args: Vec<String>,
    /// Still a `.cmd`/`.bat`: arguments pass through cmd.exe, which mangles newlines, quotes
    /// and `& | % < >`, and Rust refuses ones it can't escape. Pass only simple arguments.
    pub batch: bool,
}

/// Resolves `command` for spawning. On Windows, npm installs CLIs (codex, claude, opencode…)
/// as `.cmd` shims that just run `node <script> %*`; those are unwrapped to run Node directly,
/// so prompts and JSON arguments reach the CLI intact instead of going through cmd.exe.
pub fn launcher(command: &str) -> Option<Launcher> {
    find_command(command).map(launcher_at)
}

fn launcher_at(path: PathBuf) -> Launcher {
    let batch = path
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("cmd") || e.eq_ignore_ascii_case("bat"));
    if batch {
        let dir = path.parent().unwrap_or(std::path::Path::new("."));
        if let Some(target) = std::fs::read_to_string(&path)
            .ok()
            .and_then(|text| node_shim_script(&text))
            .map(|rel| dir.join(rel))
            .filter(|p| p.exists())
        {
            match shim_target(&target) {
                // Native CLIs (opencode ships opencode.exe): run them directly.
                ShimTarget::Executable => {
                    return Launcher {
                        program: target,
                        args: Vec::new(),
                        batch: false,
                    };
                }
                ShimTarget::Node => {
                    let node = Some(dir.join("node.exe"))
                        .filter(|p| p.exists())
                        .or_else(|| find_command("node"));
                    if let Some(node) = node {
                        return Launcher {
                            program: node,
                            args: vec![target.to_string_lossy().into_owned()],
                            batch: false,
                        };
                    }
                }
                // A shell script or something else: only the shim knows how to run it.
                ShimTarget::Other => {}
            }
        }
    }
    Launcher {
        program: path,
        args: Vec::new(),
        batch,
    }
}

#[derive(Debug, PartialEq)]
enum ShimTarget {
    Executable,
    Node,
    Other,
}

/// What an npm shim's target is: a native program, a Node script, or something else.
fn shim_target(target: &std::path::Path) -> ShimTarget {
    let ext = target
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .unwrap_or_default();
    if matches!(ext.as_str(), "exe" | "com") {
        return ShimTarget::Executable;
    }
    if matches!(ext.as_str(), "js" | "cjs" | "mjs") {
        return ShimTarget::Node;
    }
    // Extensionless bins say how they run in their first line ("#!/usr/bin/env node").
    let mut head = [0u8; 128];
    let n = std::fs::File::open(target)
        .and_then(|mut f| std::io::Read::read(&mut f, &mut head))
        .unwrap_or(0);
    let first_line = String::from_utf8_lossy(&head[..n]);
    let first_line = first_line.lines().next().unwrap_or("");
    if first_line.starts_with("#!") && first_line.contains("node") {
        ShimTarget::Node
    } else {
        ShimTarget::Other
    }
}

/// The script an npm/pnpm `.cmd` shim hands to Node, relative to the shim's folder:
/// the last `"%dp0%\…"` (or `"%~dp0\…"`) path before `%*` that isn't node.exe itself.
fn node_shim_script(text: &str) -> Option<String> {
    let line = text.lines().rev().find(|l| l.contains("%*"))?;
    line.split('"')
        .filter_map(|part| {
            part.strip_prefix("%dp0%\\")
                .or_else(|| part.strip_prefix("%~dp0\\"))
                .or_else(|| part.strip_prefix("%~dp0"))
        })
        .filter(|rel| !rel.eq_ignore_ascii_case("node.exe") && !rel.is_empty())
        .last()
        .map(|rel| rel.replace('\\', std::path::MAIN_SEPARATOR_STR))
}

/// Console programs started from the GUI app (curl, git, the headless browser…) would each
/// open a console window on Windows; closing one kills the program.
pub fn no_window(cmd: &mut std::process::Command) -> &mut std::process::Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}

/// The user's shell (used for the "shell" provider).
pub fn user_shell() -> String {
    #[cfg(unix)]
    {
        login_env()
            .get("SHELL")
            .cloned()
            .unwrap_or_else(|| "/bin/zsh".into())
    }
    #[cfg(windows)]
    {
        "powershell.exe".into()
    }
}

#[cfg(unix)]
fn resolve() -> Option<HashMap<String, String>> {
    use std::io::Read;
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    const MARKER: &str = "__WINFER_ENV_START__";
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".into());

    let mut child = Command::new(&shell)
        .args(["-ilc", &format!("printf '{MARKER}'; env -0")])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;

    // A broken rc file must not hang app startup.
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match child.try_wait().ok()? {
            Some(_) => break,
            None if Instant::now() > deadline => {
                let _ = child.kill();
                return None;
            }
            None => std::thread::sleep(Duration::from_millis(20)),
        }
    }

    let mut out = Vec::new();
    child.stdout.take()?.read_to_end(&mut out).ok()?;
    let out = String::from_utf8_lossy(&out);
    let (_, env) = out.split_once(MARKER)?;

    let vars: HashMap<String, String> = env
        .split('\0')
        .filter_map(|kv| kv.split_once('='))
        .filter(|(k, _)| !matches!(*k, "PWD" | "OLDPWD" | "SHLVL" | "_"))
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();

    vars.contains_key("PATH").then_some(vars)
}

#[cfg(windows)]
fn resolve() -> Option<HashMap<String, String>> {
    None
}

#[cfg(test)]
mod tests {
    use super::node_shim_script;

    #[test]
    fn reads_the_script_from_npm_and_pnpm_shims() {
        // npm's cmd-shim (e.g. %APPDATA%\\npm\\codex.cmd).
        let npm = "@ECHO off\r\nGOTO start\r\n:find_dp0\r\nSET dp0=%~dp0\r\nEXIT /b\r\n:start\r\nSETLOCAL\r\nCALL :find_dp0\r\n\r\nIF EXIST \"%dp0%\\node.exe\" (\r\n  SET \"_prog=%dp0%\\node.exe\"\r\n) ELSE (\r\n  SET \"_prog=node\"\r\n  SET PATHEXT=%PATHEXT:;.JS;=;%\r\n)\r\n\r\nendLocal & goto #_undefined_# 2>NUL || title %COMSPEC% & \"%_prog%\"  \"%dp0%\\node_modules\\@openai\\codex\\bin\\codex.js\" %*\r\n";
        let sep = std::path::MAIN_SEPARATOR_STR;
        assert_eq!(
            node_shim_script(npm).unwrap(),
            ["node_modules", "@openai", "codex", "bin", "codex.js"].join(sep)
        );
        // pnpm-style shim.
        let pnpm = "@SETLOCAL\r\n@IF EXIST \"%~dp0\\node.exe\" (\r\n  \"%~dp0\\node.exe\"  \"%~dp0\\..\\cli\\bin\\run.js\" %*\r\n) ELSE (\r\n  node  \"%~dp0\\..\\cli\\bin\\run.js\" %*\r\n)\r\n";
        assert_eq!(node_shim_script(pnpm).unwrap(), ["..", "cli", "bin", "run.js"].join(sep));
        // Not a Node shim.
        assert_eq!(node_shim_script("@echo off\r\nsomething.exe %*\r\n"), None);
    }

    #[test]
    fn tells_native_node_and_other_shim_targets_apart() {
        use super::{shim_target, ShimTarget};
        let dir = std::env::temp_dir().join(format!("winfer-target-{:x}", rand::random::<u32>()));
        std::fs::create_dir_all(&dir).unwrap();
        let write = |name: &str, body: &[u8]| {
            let p = dir.join(name);
            std::fs::write(&p, body).unwrap();
            p
        };
        // opencode-ai's bin is its native binary.
        assert_eq!(shim_target(&write("opencode.exe", b"MZ\x90\x00")), ShimTarget::Executable);
        assert_eq!(shim_target(&write("codex.js", b"import x from 'y'")), ShimTarget::Node);
        assert_eq!(shim_target(&write("claude", b"#!/usr/bin/env node\nrequire('x')")), ShimTarget::Node);
        assert_eq!(shim_target(&write("tool", b"#!/bin/sh\nexec foo")), ShimTarget::Other);
        assert_eq!(shim_target(&write("blob", b"\x7fELF")), ShimTarget::Other);
    }

    /// An npm shim for a native program (like opencode.exe) runs the program itself.
    #[cfg(windows)]
    #[test]
    fn exe_shims_run_the_program_directly() {
        let dir = std::env::temp_dir().join(format!("winfer-exe-shim-{:x}", rand::random::<u32>()));
        let bin = dir.join("node_modules").join("fake").join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        let exe = bin.join("fake.exe");
        std::fs::copy(r"C:\Windows\System32\whoami.exe", &exe).unwrap();
        let shim = dir.join("fake.cmd");
        std::fs::write(&shim, "@ECHO off\r\nGOTO start\r\n:find_dp0\r\nSET dp0=%~dp0\r\nEXIT /b\r\n:start\r\nSETLOCAL\r\nCALL :find_dp0\r\n\"%dp0%\\node_modules\\fake\\bin\\fake.exe\"   %*\r\n").unwrap();
        let launcher = super::launcher_at(shim);
        assert_eq!(launcher.program, exe);
        assert!(launcher.args.is_empty() && !launcher.batch, "{launcher:?}");
        assert!(std::process::Command::new(&launcher.program).output().unwrap().status.success());
    }

    /// A real npm-style shim: unwrapped to Node, a multi-line argument full of cmd.exe
    /// metacharacters arrives intact (through the .cmd it would be refused or mangled).
    #[cfg(windows)]
    #[test]
    fn unwrapped_shim_passes_arguments_intact() {
        let dir = std::env::temp_dir().join(format!("winfer-shim-{:x}", rand::random::<u32>()));
        let bin = dir.join("node_modules").join("fake").join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::write(bin.join("fake.js"), "process.stdout.write(JSON.stringify(process.argv.slice(2)))").unwrap();
        let shim = dir.join("fake.cmd");
        std::fs::write(
            &shim,
            "@ECHO off\r\nGOTO start\r\n:find_dp0\r\nSET dp0=%~dp0\r\nEXIT /b\r\n:start\r\nSETLOCAL\r\nCALL :find_dp0\r\nIF EXIST \"%dp0%\\node.exe\" (\r\n  SET \"_prog=%dp0%\\node.exe\"\r\n) ELSE (\r\n  SET \"_prog=node\"\r\n)\r\nendLocal & goto #_undefined_# 2>NUL || title %COMSPEC% & \"%_prog%\"  \"%dp0%\\node_modules\\fake\\bin\\fake.js\" %*\r\n",
        )
        .unwrap();

        let launcher = super::launcher_at(shim);
        assert!(!launcher.batch, "{launcher:?}");
        let tricky = "line one\nline \"two\" & echo pwned | more %PATH% <x>";
        let out = std::process::Command::new(&launcher.program)
            .args(&launcher.args)
            .arg(tricky)
            .output()
            .unwrap();
        let args: Vec<String> = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(args, vec![tricky.to_string()]);
    }
}
