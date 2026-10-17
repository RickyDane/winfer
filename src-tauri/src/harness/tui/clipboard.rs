//! Images on the system clipboard (a screenshot, a copied picture), saved as PNG files to attach.
//! Uses the OS tools rather than a clipboard crate: osascript on macOS, wl-paste/xclip on Linux,
//! PowerShell on Windows.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

/// Where pasted images go: `<temp>/winfer-code/paste-<millis>.png`.
fn target() -> Option<PathBuf> {
    let dir = std::env::temp_dir().join("winfer-code");
    std::fs::create_dir_all(&dir).ok()?;
    let ms = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0);
    Some(dir.join(format!("paste-{ms}.png")))
}

/// Saves the clipboard's image to a PNG file; None when it holds no image.
pub fn save_image() -> Option<PathBuf> {
    let path = target()?;
    if write_image(&path) && is_png(&path) {
        return Some(path);
    }
    let _ = std::fs::remove_file(&path);
    None
}

fn is_png(path: &Path) -> bool {
    std::fs::read(path).is_ok_and(|b| b.len() > 8 && b.starts_with(b"\x89PNG"))
}

#[cfg_attr(all(unix, not(target_os = "macos")), allow(dead_code))]
fn quiet(cmd: &mut Command) -> &mut Command {
    cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null())
}

#[cfg(target_os = "macos")]
fn write_image(path: &Path) -> bool {
    let p = path.to_string_lossy().replace('\\', "\\\\").replace('"', "\\\"");
    // «class PNGf» covers screenshots and copied images; AppleScript converts TIFF to PNG.
    let script = format!(
        "set f to POSIX file \"{p}\"\nset png to (the clipboard as «class PNGf»)\nset h to open for access f with write permission\nset eof h to 0\nwrite png to h\nclose access h"
    );
    quiet(Command::new("osascript").arg("-e").arg(script)).status().is_ok_and(|s| s.success())
}

#[cfg(all(unix, not(target_os = "macos")))]
fn write_image(path: &Path) -> bool {
    let tries: [(&str, &[&str]); 2] =
        [("wl-paste", &["--no-newline", "--type", "image/png"]), ("xclip", &["-selection", "clipboard", "-t", "image/png", "-o"])];
    tries.iter().any(|(bin, args)| {
        let Ok(file) = std::fs::File::create(path) else { return false };
        Command::new(bin)
            .args(*args)
            .stdin(Stdio::null())
            .stdout(file)
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
            && is_png(path)
    })
}

#[cfg(windows)]
fn write_image(path: &Path) -> bool {
    let p = path.to_string_lossy().replace('\'', "''");
    let script = format!(
        "Add-Type -AssemblyName System.Windows.Forms; $i = [System.Windows.Forms.Clipboard]::GetImage(); if ($i) {{ $i.Save('{p}', [System.Drawing.Imaging.ImageFormat]::Png) }} else {{ exit 1 }}"
    );
    quiet(Command::new("powershell").args(["-NoProfile", "-STA", "-Command", &script])).status().is_ok_and(|s| s.success())
}
