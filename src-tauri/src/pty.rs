//! Pseudo-terminal sessions, one per pane.
//!
//! Output is streamed to the webview as raw bytes over a `tauri::ipc::Channel` and also kept
//! in a per-pane ring buffer, so a reloaded webview (or a pane re-attached after a layout
//! change) can replay scrollback while the agent keeps running.

use std::collections::{HashMap, VecDeque};
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use portable_pty::{ChildKiller, CommandBuilder, MasterPty, PtySize, native_pty_system};
use serde::{Deserialize, Serialize};
use tauri::ipc::{Channel, InvokeResponseBody};
use tauri::{AppHandle, Emitter, Manager, State};

use crate::env::{login_env, user_shell};
use crate::hooks;
use crate::providers;

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
    writer: Box<dyn Write + Send>,
    killer: Box<dyn ChildKiller + Send + Sync>,
    pid: Option<u32>,
    output: Arc<Mutex<Output>>,
}

#[derive(Default)]
pub struct PtyManager {
    sessions: Mutex<HashMap<String, Session>>,
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
fn resolve_cwd(cwd: Option<&str>) -> PathBuf {
    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"));
    let path = match cwd {
        Some("~") => home.clone(),
        Some(p) if p.starts_with("~/") => home.join(&p[2..]),
        Some(p) => PathBuf::from(p),
        None => home.clone(),
    };
    if path.is_dir() { path } else { home }
}

fn build_command(req: &SpawnRequest) -> Result<CommandBuilder, String> {
    let provider = providers::find(&req.provider_id)
        .ok_or_else(|| format!("unknown provider: {}", req.provider_id))?;

    let mut cmd = match &provider.command {
        Some(command) => {
            let mut cmd = CommandBuilder::new(command);
            // Hook args first: Codex's `-c` must precede subcommands like `resume`.
            cmd.args(hooks::provider_args(&provider.id));
            cmd.args(&provider.args);
            if req.resume {
                cmd.args(&provider.resume_args);
            }
            cmd
        }
        None => {
            let mut cmd = CommandBuilder::new(user_shell());
            #[cfg(unix)]
            cmd.arg("-l");
            cmd
        }
    };

    for (key, value) in login_env() {
        cmd.env(key, value);
    }
    cmd.env("TERM", "xterm-256color");
    cmd.env("COLORTERM", "truecolor");
    cmd.env("TERM_PROGRAM", "winfer");
    cmd.env("WINFER_PANE_ID", &req.pane_id);
    if let Some(server) = hooks::server() {
        cmd.env("WINFER_HOOK", &server.addr);
        cmd.env("WINFER_TOKEN", &server.token);
    }
    if let Some(workspace) = &req.workspace_id {
        cmd.env("WINFER_WORKSPACE", workspace);
    }
    cmd.cwd(resolve_cwd(req.cwd.as_deref()));
    Ok(cmd)
}

#[tauri::command]
pub fn pty_spawn(
    app: AppHandle,
    state: State<'_, PtyManager>,
    req: SpawnRequest,
    on_data: Channel,
) -> Result<SpawnResult, String> {
    let mut sessions = state.sessions.lock().unwrap();

    if let Some(session) = sessions.get(&req.pane_id) {
        // Re-attach: replay scrollback, then route live output to the new channel.
        // Holding the output lock keeps replay and live bytes in order.
        let mut output = session.output.lock().unwrap();
        let replay: Vec<u8> = output.scrollback.iter().copied().collect();
        if !replay.is_empty() {
            let _ = on_data.send(InvokeResponseBody::Raw(replay));
        }
        output.channel = Some(on_data);
        let _ = session.master.resize(size(req.cols, req.rows));
        return Ok(SpawnResult {
            attached: true,
            pid: session.pid,
        });
    }

    let cmd = build_command(&req)?;
    let pair = native_pty_system()
        .openpty(size(req.cols, req.rows))
        .map_err(|e| e.to_string())?;
    let mut child = pair.slave.spawn_command(cmd).map_err(|e| e.to_string())?;
    drop(pair.slave);

    let mut reader = pair.master.try_clone_reader().map_err(|e| e.to_string())?;
    let writer = pair.master.take_writer().map_err(|e| e.to_string())?;
    let pid = child.process_id();
    let killer = child.clone_killer();
    let output = Arc::new(Mutex::new(Output {
        scrollback: VecDeque::new(),
        channel: Some(on_data),
    }));

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

    let pane_id = req.pane_id.clone();
    let session_output = output.clone();
    std::thread::spawn(move || {
        let code = child.wait().map(|s| s.exit_code()).unwrap_or(1);
        let manager = app.state::<PtyManager>();
        let mut sessions = manager.sessions.lock().unwrap();
        // Only drop the session if it is still this one (not killed and respawned).
        if sessions
            .get(&pane_id)
            .is_some_and(|s| Arc::ptr_eq(&s.output, &session_output))
        {
            sessions.remove(&pane_id);
        }
        drop(sessions);
        let _ = app.emit("pty://exit", ExitEvent { pane_id, code });
    });

    sessions.insert(
        req.pane_id.clone(),
        Session {
            master: pair.master,
            writer,
            killer,
            pid,
            output,
        },
    );

    Ok(SpawnResult {
        attached: false,
        pid,
    })
}

#[tauri::command]
pub fn pty_write(
    state: State<'_, PtyManager>,
    pane_id: String,
    data: String,
) -> Result<(), String> {
    let mut sessions = state.sessions.lock().unwrap();
    let session = sessions.get_mut(&pane_id).ok_or("no such pane")?;
    session
        .writer
        .write_all(data.as_bytes())
        .map_err(|e| e.to_string())
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
    let Some(mut session) = state.sessions.lock().unwrap().remove(&pane_id) else {
        return;
    };
    // The child is a session leader (setsid), so its pid is also its process group id:
    // hang up the whole group so tools the agent started go away too.
    #[cfg(unix)]
    if let Some(pid) = session.pid {
        unsafe {
            libc::kill(-(pid as i32), libc::SIGHUP);
        }
    }
    let _ = session.killer.kill();
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
            cols: 80,
            rows: 24,
        };
        let mut cmd = build_command(&req).unwrap();
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
            cols: 80,
            rows: 24,
        };
        assert!(build_command(&req).is_err());
    }
}
