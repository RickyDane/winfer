//! Newer versions of the agent CLIs, and installing them.
//!
//! How a CLI updates depends on how it was installed: Homebrew installs (the binary lives under
//! `Cellar/` or `Caskroom/`) go through `brew upgrade`, npm installs through the package manager
//! that owns the `node_modules` folder, everything else through the CLI's own update subcommand.
//! The newest version comes from Homebrew for brew installs, otherwise from the provider's
//! `update` source in providers.json (an npm package or a page that names it).

use serde::Serialize;
use std::path::{Component, Path, PathBuf};
use std::time::Duration;

use crate::env::login_env;
use crate::providers::{self, Provider};

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum Install {
    Brew { name: String, cask: bool },
    /// Installed globally by npm, pnpm or bun (the `manager`).
    Package { manager: String },
    /// Anything else: native installers that keep their own versions folder, manual copies.
    Own,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VersionStatus {
    pub id: String,
    pub current: Option<String>,
    /// None when there's no way to tell (offline, or the CLI names no source).
    pub latest: Option<String>,
    pub outdated: bool,
    pub install: Install,
    /// Whether winfer knows a command that updates it.
    pub updatable: bool,
}

/// How the CLI at `path` (symlinks resolved) was installed.
pub fn install_kind(path: &Path) -> Install {
    let parts: Vec<String> = path
        .components()
        .filter_map(|c| match c {
            Component::Normal(s) => Some(s.to_string_lossy().into_owned()),
            _ => None,
        })
        .collect();
    for (i, part) in parts.iter().enumerate() {
        if let (true, Some(name)) = (part == "Cellar" || part == "Caskroom", parts.get(i + 1)) {
            return Install::Brew {
                name: name.clone(),
                cask: part == "Caskroom",
            };
        }
    }
    if parts.iter().any(|p| p == "node_modules") {
        let manager = if parts.iter().any(|p| p == ".bun") {
            "bun"
        } else if parts.iter().any(|p| p.to_ascii_lowercase().contains("pnpm")) {
            "pnpm"
        } else {
            "npm"
        };
        return Install::Package {
            manager: manager.into(),
        };
    }
    Install::Own
}

/// The version in a `--version` line: "codex-cli 0.162.1" → "0.162.1", "opencode v2.0.28" → "2.0.28".
pub fn parse_version(text: &str) -> Option<String> {
    let start = text.find(|c: char| c.is_ascii_digit())?;
    let version: String = text[start..]
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || ".-+".contains(*c))
        .collect();
    let version = version.trim_end_matches(['.', '-', '+']);
    (!version.is_empty()).then(|| version.to_string())
}

/// The numeric release part: "2026.08.11-e8db854" → [2026, 8, 11]; build hashes and
/// pre-release tags don't count.
fn release(version: &str) -> Vec<u64> {
    version
        .trim_start_matches('v')
        .split(['.', '-', '+'])
        .map_while(|p| p.parse().ok())
        .collect()
}

/// Whether `latest` is a newer release than `current`.
pub fn is_newer(latest: &str, current: &str) -> bool {
    let (a, b) = (release(latest), release(current));
    if a.is_empty() || b.is_empty() {
        return false;
    }
    for i in 0..a.len().max(b.len()) {
        let (x, y) = (a.get(i).copied().unwrap_or(0), b.get(i).copied().unwrap_or(0));
        if x != y {
            return x > y;
        }
    }
    false
}

/// The version that follows `marker` in `page`: "…/lab/2026.10.14-941492e/…" → "2026.10.14-941492e".
pub fn version_after(page: &str, marker: &str) -> Option<String> {
    let rest = &page[page.find(marker)? + marker.len()..];
    let version: String = rest
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || "._-".contains(*c))
        .collect();
    (!release(&version).is_empty()).then_some(version)
}

/// Homebrew's newest version of a formula or cask (`brew info` reads its fresh API cache).
pub fn brew_latest(json: &str, cask: bool) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(json).ok()?;
    let version = if cask {
        value["casks"][0]["version"].as_str()?
    } else {
        value["formulae"][0]["versions"]["stable"].as_str()?
    };
    // Casks may append a build after a comma ("1.2.3,abc").
    Some(version.split(',').next()?.to_string())
}

/// The same from formulae.brew.sh, which describes one formula or cask per document.
pub fn brew_api_latest(json: &str, cask: bool) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(json).ok()?;
    let wrapped = if cask {
        serde_json::json!({ "casks": [value] })
    } else {
        serde_json::json!({ "formulae": [value] })
    };
    brew_latest(&wrapped.to_string(), cask)
}

fn http_get(url: &str) -> Option<String> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(10)))
        .build()
        .into();
    agent.get(url).call().ok()?.body_mut().read_to_string().ok()
}

/// Runs `program args…` with the login environment; stdout and stderr together, or None on
/// timeout or if it couldn't start.
fn run(program: &Path, args: &[String], timeout: Duration) -> Option<(bool, String)> {
    use std::io::Read;
    let mut child = crate::env::no_window(&mut std::process::Command::new(program))
        .args(args)
        .envs(login_env())
        .env("NO_COLOR", "1")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .ok()?;
    let mut stdout = child.stdout.take()?;
    let mut stderr = child.stderr.take()?;
    let out = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = stdout.read_to_string(&mut s);
        s
    });
    let err = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = stderr.read_to_string(&mut s);
        s
    });
    let deadline = std::time::Instant::now() + timeout;
    let status = loop {
        if let Some(status) = child.try_wait().ok()? {
            break status;
        }
        if std::time::Instant::now() > deadline {
            let _ = child.kill();
            return None;
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    let text = out.join().unwrap_or_default() + &err.join().unwrap_or_default();
    Some((status.success(), text))
}

/// The installed CLI: how to run it and the file it really is (an npm shim's script on Windows).
fn locate(provider: &Provider) -> Option<(crate::env::Launcher, PathBuf)> {
    let launcher = crate::env::launcher(provider.command.as_deref()?)?;
    let file = launcher
        .args
        .first()
        .map(PathBuf::from)
        .unwrap_or_else(|| launcher.program.clone());
    let real = std::fs::canonicalize(&file).unwrap_or(file);
    Some((launcher, real))
}

fn current_version(launcher: &crate::env::Launcher) -> Option<String> {
    let mut args = launcher.args.clone();
    args.push("--version".into());
    let (_, out) = run(&launcher.program, &args, Duration::from_secs(15))?;
    out.lines().find_map(parse_version)
}

fn latest_version(provider: &Provider, install: &Install) -> Option<String> {
    if let Install::Brew { name, cask } = install {
        // The public API is current; `brew info` reads a cache that's only refreshed by
        // `brew update`, but it also knows formulae from third-party taps.
        let kind = if *cask { "cask" } else { "formula" };
        if let Some(latest) = http_get(&format!("https://formulae.brew.sh/api/{kind}/{name}.json"))
            .and_then(|body| brew_api_latest(&body, *cask))
        {
            return Some(latest);
        }
        let brew = crate::env::find_command("brew")?;
        let kind = if *cask { "--cask" } else { "--formula" };
        let args = ["info", "--json=v2", kind, name].map(String::from);
        let (ok, out) = run(&brew, &args, Duration::from_secs(30))?;
        return if ok { brew_latest(&out, *cask) } else { None };
    }
    let source = provider.update.as_ref()?;
    if let Some(package) = &source.npm {
        let body = http_get(&format!("https://registry.npmjs.org/{package}/latest"))?;
        let value: serde_json::Value = serde_json::from_str(&body).ok()?;
        return value["version"].as_str().map(String::from);
    }
    if let (Some(url), Some(after)) = (&source.url, &source.after) {
        return version_after(&http_get(url)?, after);
    }
    None
}

/// The command that updates the CLI, as program + arguments.
fn update_command(provider: &Provider, launcher: &crate::env::Launcher, install: &Install) -> Option<(PathBuf, Vec<String>)> {
    let source = provider.update.as_ref();
    match install {
        Install::Brew { name, cask } => {
            let mut args = vec!["upgrade".to_string()];
            if *cask {
                args.push("--cask".into());
            }
            args.push(name.clone());
            Some((crate::env::find_command("brew")?, args))
        }
        Install::Package { manager } if source.and_then(|s| s.npm.as_ref()).is_some() => {
            let package = format!("{}@latest", source?.npm.as_ref()?);
            let args = match manager.as_str() {
                "npm" => vec!["install".into(), "-g".into(), package],
                _ => vec!["add".into(), "-g".into(), package],
            };
            Some((crate::env::find_command(manager)?, args))
        }
        _ => {
            let own = source.filter(|s| !s.args.is_empty())?;
            let mut args = launcher.args.clone();
            args.extend(own.args.iter().cloned());
            Some((launcher.program.clone(), args))
        }
    }
}

pub fn status(provider: &Provider) -> Option<VersionStatus> {
    let (launcher, real) = locate(provider)?;
    let install = install_kind(&real);
    let current = current_version(&launcher);
    let latest = latest_version(provider, &install);
    let outdated = matches!((&latest, &current), (Some(l), Some(c)) if is_newer(l, c));
    Some(VersionStatus {
        id: provider.id.clone(),
        updatable: update_command(provider, &launcher, &install).is_some(),
        current,
        latest,
        outdated,
        install,
    })
}

/// Versions of every installed agent CLI, checked side by side.
#[tauri::command]
pub async fn providers_versions() -> Vec<VersionStatus> {
    let check = || {
        let handles: Vec<_> = providers::all()
            .into_iter()
            .filter(|p| p.command.is_some() && providers::is_installed(p))
            .map(|p| std::thread::spawn(move || status(&p)))
            .collect();
        handles.into_iter().filter_map(|h| h.join().ok().flatten()).collect()
    };
    tauri::async_runtime::spawn_blocking(check).await.unwrap_or_default()
}

/// Updates one CLI and returns its new status; on failure, the end of the command's output.
#[tauri::command]
pub async fn providers_update(id: String) -> Result<VersionStatus, String> {
    let provider = providers::find(&id).ok_or("unknown agent")?;
    tauri::async_runtime::spawn_blocking(move || {
        let (launcher, real) = locate(&provider).ok_or(format!("{} isn't installed", provider.name))?;
        let install = install_kind(&real);
        let (program, args) = update_command(&provider, &launcher, &install)
            .ok_or(format!("winfer doesn't know how to update {}", provider.name))?;
        let (ok, out) = run(&program, &args, Duration::from_secs(600)).ok_or("the update didn't finish within 10 minutes")?;
        if !ok {
            let tail: Vec<&str> = out.lines().filter(|l| !l.trim().is_empty()).collect();
            return Err(tail[tail.len().saturating_sub(6)..].join("\n"));
        }
        status(&provider).ok_or(format!("{} is gone after the update", provider.name))
    })
    .await
    .map_err(|e| e.to_string())?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tells_how_a_cli_was_installed() {
        assert_eq!(
            install_kind(Path::new("/opt/homebrew/Caskroom/codex/0.162.1/bin/codex")),
            Install::Brew { name: "codex".into(), cask: true }
        );
        assert_eq!(
            install_kind(Path::new("/opt/homebrew/Cellar/opencode-v2/2.0.28/bin/opencode")),
            Install::Brew { name: "opencode-v2".into(), cask: false }
        );
        assert_eq!(
            install_kind(Path::new("/usr/local/lib/node_modules/@openai/codex/bin/codex.js")),
            Install::Package { manager: "npm".into() }
        );
        assert_eq!(
            install_kind(Path::new("/Users/me/.bun/install/global/node_modules/x/cli.js")),
            Install::Package { manager: "bun".into() }
        );
        assert_eq!(
            install_kind(Path::new("/Users/me/Library/pnpm/global/5/node_modules/x/cli.js")),
            Install::Package { manager: "pnpm".into() }
        );
        assert_eq!(
            install_kind(Path::new("/Users/me/.local/share/claude/versions/2.1.301")),
            Install::Own
        );
    }

    #[test]
    fn reads_versions() {
        assert_eq!(parse_version("2.1.301 (Claude Code)\n").as_deref(), Some("2.1.301"));
        assert_eq!(parse_version("codex-cli 0.162.1").as_deref(), Some("0.162.1"));
        assert_eq!(parse_version("opencode v2.0.28").as_deref(), Some("2.0.28"));
        assert_eq!(parse_version("2026.08.11-e8db854").as_deref(), Some("2026.08.11-e8db854"));
        assert_eq!(parse_version("no version here"), None);
    }

    #[test]
    fn compares_releases() {
        assert!(is_newer("0.163.0", "0.162.1"));
        assert!(is_newer("2.1.10", "2.1.9"));
        assert!(!is_newer("2.1.301", "2.1.301"));
        assert!(is_newer("2026.10.14-941492e", "2026.08.11-e8db854"));
        assert!(!is_newer("2026.08.11-aaaaaaa", "2026.08.11-e8db854"));
        // npm's `latest` for OpenCode is still 1.x while v2 ships elsewhere.
        assert!(!is_newer("1.19.2", "2.0.28"));
        assert!(!is_newer("", "1.0.0"));
    }

    #[test]
    fn finds_the_version_in_a_page() {
        let script = "DOWNLOAD_URL=\"https://downloads.cursor.com/lab/2026.10.14-941492e/${OS}/${ARCH}/agent.tar.gz\"";
        assert_eq!(
            version_after(script, "downloads.cursor.com/lab/").as_deref(),
            Some("2026.10.14-941492e")
        );
        assert_eq!(version_after("downloads.cursor.com/lab/${V}/", "downloads.cursor.com/lab/"), None);
        assert_eq!(version_after("nothing", "downloads.cursor.com/lab/"), None);
    }

    #[test]
    fn reads_brew_info() {
        let cask = r#"{"formulae":[],"casks":[{"token":"codex","version":"0.163.0"}]}"#;
        assert_eq!(brew_latest(cask, true).as_deref(), Some("0.163.0"));
        let formula = r#"{"formulae":[{"versions":{"stable":"2.0.29"}}],"casks":[]}"#;
        assert_eq!(brew_latest(formula, false).as_deref(), Some("2.0.29"));
        assert_eq!(brew_latest(r#"{"casks":[{"version":"1.2.3,abc"}]}"#, true).as_deref(), Some("1.2.3"));
    }
}

#[cfg(test)]
mod live {
    /// Versions of the installed CLIs (machine-specific): cargo test -- --ignored live_versions
    #[test]
    #[ignore]
    fn live_versions() {
        for p in crate::providers::all().iter().filter(|p| crate::providers::is_installed(p)) {
            eprintln!("{}: {:?}", p.id, super::status(p));
        }
    }
}
