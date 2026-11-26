//! The server's health for its row in the desktop's sidebar (`server.stats` in the mirror). Its
//! folders (`fs.list`, `git.clone`, the deploy key) are in `winfer_core::folders`.

use serde_json::{Value, json};
pub use winfer_core::folders::*;
use std::process::Command;

fn read(path: &str) -> Option<String> {
    std::fs::read_to_string(path).ok()
}

/// Load, memory, disk and uptime, where the OS tells (Linux: /proc; elsewhere: what `df` and
/// `sysctl` give).
pub fn stats() -> Value {
    let cpus = std::thread::available_parallelism().map_or(1, |n| n.get());
    let load = read("/proc/loadavg").and_then(|t| t.split_whitespace().next()?.parse::<f64>().ok()).or_else(|| {
        let out = Command::new("sysctl").args(["-n", "vm.loadavg"]).output().ok()?;
        String::from_utf8_lossy(&out.stdout).trim().trim_start_matches('{').split_whitespace().next()?.parse().ok()
    });
    let mem = read("/proc/meminfo").map(|t| {
        let field = |name: &str| t.lines().find(|l| l.starts_with(name)).and_then(|l| l.split_whitespace().nth(1)?.parse::<u64>().ok()).unwrap_or(0) * 1024;
        (field("MemTotal:"), field("MemTotal:").saturating_sub(field("MemAvailable:")))
    });
    let uptime = read("/proc/uptime").and_then(|t| t.split_whitespace().next()?.parse::<f64>().ok()).map(|s| s as u64);
    let disk = Command::new("df").args(["-kP"]).arg(expand("~")).output().ok().and_then(|o| {
        let text = String::from_utf8_lossy(&o.stdout).to_string();
        let cols: Vec<u64> = text.lines().nth(1)?.split_whitespace().skip(1).take(3).filter_map(|c| c.parse().ok()).collect();
        (cols.len() == 3).then(|| (cols[0] * 1024, cols[1] * 1024))
    });
    json!({
        "cpus": cpus, "load": load,
        "memTotal": mem.map(|m| m.0), "memUsed": mem.map(|m| m.1),
        "diskTotal": disk.map(|d| d.0), "diskUsed": disk.map(|d| d.1),
        "uptime": uptime,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_clones_after_the_repo() {
        assert_eq!(repo_name("https://github.com/RickyDane/winfer.git").as_deref(), Some("winfer"));
        assert_eq!(repo_name("git@github.com:org/tool.git").as_deref(), Some("tool"));
        assert_eq!(repo_name("https://example.com/x/y/").as_deref(), Some("y"));
        assert_eq!(repo_name("https://example.com/.."), None);
        assert!(looks_like_git_url("git@github.com:org/tool.git"));
        assert!(!looks_like_git_url("~/code"));
    }

    #[test]
    fn lists_folders_with_home_as_tilde() {
        let v = list("~");
        assert_eq!(v["path"], "~");
        assert!(v["dirs"].is_array());
        assert!(stats()["cpus"].as_u64().unwrap() >= 1);
    }
}

