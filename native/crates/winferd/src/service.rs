//! `winferd service install|uninstall`: run winferd at login/boot as the user's service
//! (a systemd user unit on Linux, a launchd agent on macOS).

use std::path::PathBuf;
use std::process::Command;

const LABEL: &str = "dev.winfer.server";

fn exe() -> Result<String, String> {
    std::env::current_exe().map(|p| p.display().to_string()).map_err(|e| e.to_string())
}

fn run(cmd: &str, args: &[&str]) -> Result<(), String> {
    let out = Command::new(cmd).args(args).output().map_err(|e| format!("{cmd}: {e}"))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(format!("{cmd} {}: {}", args.join(" "), String::from_utf8_lossy(&out.stderr).trim()))
    }
}

fn home() -> Result<PathBuf, String> {
    dirs::home_dir().ok_or_else(|| "no home folder".to_string())
}

/// The unit for systemd (user): restarts on failure, logs to the journal.
pub fn systemd_unit(exe: &str) -> String {
    format!(
        "[Unit]\nDescription=winferd (winfer for servers)\nAfter=network-online.target\nWants=network-online.target\n\n\
         [Service]\nExecStart={exe} run\nRestart=on-failure\nRestartSec=3\n\
         # Only winferd itself: what its terminals start in the background (screen, tmux, nohup) keeps\n\
         # running, as after an ssh session; its panes end with it (their terminal closes).\n\
         KillMode=process\n\n\
         [Install]\nWantedBy=default.target\n"
    )
}

/// The launchd agent: kept alive, logs into the data folder.
pub fn launchd_plist(exe: &str, log: &str) -> String {
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
         <plist version=\"1.0\">\n<dict>\n  <key>Label</key><string>{LABEL}</string>\n  \
         <key>ProgramArguments</key><array><string>{exe}</string><string>run</string></array>\n  \
         <key>RunAtLoad</key><true/>\n  <key>KeepAlive</key><true/>\n  \
         <key>StandardErrorPath</key><string>{log}</string>\n  <key>StandardOutPath</key><string>{log}</string>\n\
         </dict>\n</plist>\n"
    )
}

pub fn install() -> Result<String, String> {
    let exe = exe()?;
    if cfg!(target_os = "macos") {
        let dir = home()?.join("Library/LaunchAgents");
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let path = dir.join(format!("{LABEL}.plist"));
        let log = winfer_core::backend::data_dir().join("logs").join("winferd.log");
        let _ = std::fs::create_dir_all(log.parent().unwrap());
        std::fs::write(&path, launchd_plist(&exe, &log.display().to_string())).map_err(|e| e.to_string())?;
        let _ = run("launchctl", &["unload", &path.display().to_string()]);
        run("launchctl", &["load", "-w", &path.display().to_string()])?;
        Ok(format!("Installed the launchd agent {} (logs: {}).", path.display(), log.display()))
    } else {
        let dir = home()?.join(".config/systemd/user");
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let path = dir.join("winferd.service");
        std::fs::write(&path, systemd_unit(&exe)).map_err(|e| e.to_string())?;
        run("systemctl", &["--user", "daemon-reload"])?;
        run("systemctl", &["--user", "enable", "--now", "winferd.service"])?;
        let user = std::env::var("USER").unwrap_or_default();
        Ok(format!(
            "Installed and started the systemd user service {} (logs: journalctl --user -u winferd -f).\n\
             To keep it running while you're logged out: sudo loginctl enable-linger {user}",
            path.display()
        ))
    }
}

pub fn uninstall() -> Result<String, String> {
    if cfg!(target_os = "macos") {
        let path = home()?.join(format!("Library/LaunchAgents/{LABEL}.plist"));
        let _ = run("launchctl", &["unload", "-w", &path.display().to_string()]);
        let _ = std::fs::remove_file(&path);
        Ok("Removed the launchd agent.".into())
    } else {
        let _ = run("systemctl", &["--user", "disable", "--now", "winferd.service"]);
        let _ = std::fs::remove_file(home()?.join(".config/systemd/user/winferd.service"));
        let _ = run("systemctl", &["--user", "daemon-reload"]);
        Ok("Removed the systemd user service.".into())
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn units_run_the_daemon() {
        assert!(super::systemd_unit("/home/u/.local/bin/winferd").contains("ExecStart=/home/u/.local/bin/winferd run"));
        let plist = super::launchd_plist("/usr/local/bin/winferd", "/tmp/w.log");
        assert!(plist.contains("<string>/usr/local/bin/winferd</string><string>run</string>"));
        assert!(plist.contains("<key>KeepAlive</key><true/>"));
    }
}
