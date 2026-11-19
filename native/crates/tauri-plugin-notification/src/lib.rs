//! The notification fallback `notify.rs` uses when its own banner can't be shown.
use std::process::Command;
use tauri::{AppHandle, Runtime};

pub trait NotificationExt {
    fn notification(&self) -> Notification;
}

impl<R: Runtime> NotificationExt for AppHandle<R> {
    fn notification(&self) -> Notification {
        Notification
    }
}

pub struct Notification;

impl Notification {
    pub fn builder(&self) -> Builder {
        Builder::default()
    }
}

#[derive(Default)]
pub struct Builder {
    title: String,
    body: String,
}

impl Builder {
    pub fn title(mut self, title: impl Into<String>) -> Self {
        self.title = title.into();
        self
    }

    pub fn body(mut self, body: impl Into<String>) -> Self {
        self.body = body.into();
        self
    }

    pub fn show(self) -> Result<(), String> {
        let mut cmd = if cfg!(target_os = "macos") {
            let mut c = Command::new("osascript");
            c.args(["-e", "on run argv", "-e", "display notification (item 2 of argv) with title (item 1 of argv)", "-e", "end run"]);
            c.args([&self.title, &self.body]);
            c
        } else if cfg!(target_os = "linux") {
            let mut c = Command::new("notify-send");
            c.args([&self.title, &self.body]);
            c
        } else {
            return Err("no notification fallback on this platform".into());
        };
        let status = cmd.status().map_err(|e| e.to_string())?;
        status.success().then_some(()).ok_or_else(|| format!("notifier exited with {status}"))
    }
}
