//! Opens URLs in the user's default browser.
use std::process::Command;
use tauri::{AppHandle, Runtime};

pub trait OpenerExt {
    fn opener(&self) -> Opener;
}

impl<R: Runtime> OpenerExt for AppHandle<R> {
    fn opener(&self) -> Opener {
        Opener
    }
}

pub struct Opener;

impl Opener {
    pub fn open_url(&self, url: impl AsRef<str>, _with: Option<impl AsRef<str>>) -> Result<(), String> {
        let url = url.as_ref();
        let mut cmd = if cfg!(target_os = "macos") {
            Command::new("open")
        } else if cfg!(windows) {
            let mut c = Command::new("rundll32");
            c.arg("url.dll,FileProtocolHandler");
            c
        } else {
            Command::new("xdg-open")
        };
        cmd.arg(url).spawn().map(|_| ()).map_err(|e| e.to_string())
    }
}
