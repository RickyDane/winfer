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
