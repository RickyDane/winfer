//! Agent CLI presets. Data-driven so new CLIs or changed flags don't need code changes.

use serde::{Deserialize, Serialize};
use std::sync::OnceLock;

use crate::env::login_env;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Provider {
    pub id: String,
    pub name: String,
    /// `None` means the user's login shell.
    pub command: Option<String>,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub resume_args: Vec<String>,
    /// Resumes one specific session; `{session}` is replaced with its id (from the hooks).
    #[serde(default)]
    pub resume_session_args: Vec<String>,
    #[serde(default)]
    pub aliases: Vec<String>,
    /// Reports status through hooks (see hooks.rs) instead of output heuristics.
    #[serde(default)]
    pub status_hooks: bool,
    /// Accepts an initial prompt as its last argument (otherwise it's typed in after start).
    #[serde(default)]
    pub prompt_arg: bool,
    /// Option that takes the initial prompt, e.g. `--prompt` (OpenCode).
    #[serde(default)]
    pub prompt_flag: Option<String>,
    /// Keys that accept a pending permission prompt (default: Enter).
    #[serde(default)]
    pub approve_input: Option<String>,
    /// Flag that selects the model at launch, e.g. `--model`.
    #[serde(default)]
    pub model_arg: Option<String>,
    /// Known models (aliases) offered even without listing them from the CLI.
    #[serde(default)]
    pub models: Vec<ModelInfo>,
    /// Subcommand that prints the available models, e.g. `["models"]`.
    #[serde(default)]
    pub models_command: Vec<String>,
    /// Typed into a running session to switch models; `{model}` is replaced.
    #[serde(default)]
    pub model_command: Option<String>,
    /// Launch flags that skip permission prompts, added when auto-approve is on in settings.
    #[serde(default)]
    pub auto_approve_args: Vec<String>,
    /// Where to find the newest version and how the CLI updates itself (see provider_updates.rs).
    #[serde(default)]
    pub update: Option<UpdateSource>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateSource {
    /// Subcommand that updates the CLI in place, e.g. `["update"]`.
    #[serde(default)]
    pub args: Vec<String>,
    /// npm package whose `latest` tag is the newest version; also how npm installs are updated.
    #[serde(default)]
    pub npm: Option<String>,
    /// A page that names the newest version right after `after`, e.g. an install script.
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub after: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ModelInfo {
    pub id: String,
    pub label: String,
}

/// Parses a CLI's model list: `id<TAB>label`, `id - label` or bare ids; skips headers and noise.
pub fn parse_models(output: &str) -> Vec<ModelInfo> {
    let ansi = |s: &str| {
        let mut out = String::new();
        let mut chars = s.chars().peekable();
        while let Some(c) = chars.next() {
            if c == '\x1b' {
                for c in chars.by_ref() {
                    if c.is_ascii_alphabetic() {
                        break;
                    }
                }
            } else {
                out.push(c);
            }
        }
        out
    };
    let valid = |id: &str| {
        !id.is_empty()
            && id.chars().next().is_some_and(|c| c.is_ascii_alphanumeric())
            && id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "-._/:#@[]".contains(c))
    };
    let mut seen = std::collections::HashSet::new();
    output
        .lines()
        .map(|l| ansi(l).trim().to_string())
        .filter_map(|line| {
            let (id, label) = if let Some((id, label)) = line.split_once('\t') {
                (id.trim().to_string(), label.trim().to_string())
            } else if let Some((id, label)) = line.split_once(" - ") {
                (id.trim().to_string(), label.trim().to_string())
            } else {
                (line.clone(), line.clone())
            };
            (valid(&id) && seen.insert(id.clone())).then(|| ModelInfo {
                label: if label.is_empty() { id.clone() } else { label },
                id,
            })
        })
        .collect()
}

/// Static aliases plus whatever the CLI lists (cached for the app's lifetime).
pub fn models(provider: &Provider) -> Vec<ModelInfo> {
    static CACHE: OnceLock<std::sync::Mutex<std::collections::HashMap<String, Vec<ModelInfo>>>> =
        OnceLock::new();
    let cache = CACHE.get_or_init(Default::default);
    if let Some(hit) = cache.lock().unwrap().get(&provider.id) {
        return hit.clone();
    }
    let mut list = provider.models.clone();
    if let (Some(command), false) = (&provider.command, provider.models_command.is_empty()) {
        if let Some(output) = run_with_timeout(
            command,
            &provider.models_command,
            std::time::Duration::from_secs(20),
        ) {
            for model in parse_models(&output) {
                if !list.iter().any(|m| m.id == model.id) {
                    list.push(model);
                }
            }
        }
    }
    cache
        .lock()
        .unwrap()
        .insert(provider.id.clone(), list.clone());
    list
}

fn run_with_timeout(
    command: &str,
    args: &[String],
    timeout: std::time::Duration,
) -> Option<String> {
    use std::io::Read;
    let launcher = crate::env::launcher(command)?;
    let mut child = crate::env::no_window(&mut std::process::Command::new(&launcher.program))
        .args(&launcher.args)
        .args(args)
        .envs(login_env())
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .ok()?;
    let mut stdout = child.stdout.take()?;
    let reader = std::thread::spawn(move || {
        let mut out = String::new();
        let _ = stdout.read_to_string(&mut out);
        out
    });
    let deadline = std::time::Instant::now() + timeout;
    while child.try_wait().ok()?.is_none() {
        if std::time::Instant::now() > deadline {
            let _ = child.kill();
            return None;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    reader.join().ok()
}

#[tauri::command]
pub async fn providers_models(id: String) -> Result<Vec<ModelInfo>, String> {
    let provider = find(&id).ok_or("unknown provider")?;
    tauri::async_runtime::spawn_blocking(move || models(&provider))
        .await
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_opencode_major_version() {
        assert_eq!(opencode_major("opencode v2.0.28\n"), Some(2));
        assert_eq!(opencode_major("0.15.3"), Some(0));
        assert_eq!(opencode_major("1.4.0"), Some(1));
        assert_eq!(opencode_major(""), None);
    }

    #[test]
    fn parses_the_cli_model_list_formats() {
        let agy = "Fetching available models...\ngemini-3.8-flash-high\tGemini 3.8 Flash (High)\nclaude-opus-4-6-thinking\tClaude Opus 4.6 (Thinking)\n";
        assert_eq!(
            parse_models(agy),
            vec![
                ModelInfo {
                    id: "gemini-3.8-flash-high".into(),
                    label: "Gemini 3.8 Flash (High)".into()
                },
                ModelInfo {
                    id: "claude-opus-4-6-thinking".into(),
                    label: "Claude Opus 4.6 (Thinking)".into()
                },
            ]
        );
        let cursor =
            "Available models\n\n\x1b[1mauto - Auto (default)\x1b[0m\ngpt-5.3-codex - Codex 5.3\n";
        assert_eq!(
            parse_models(cursor)
                .iter()
                .map(|m| m.id.as_str())
                .collect::<Vec<_>>(),
            ["auto", "gpt-5.3-codex"]
        );
        let opencode = "opencode/big-pickle\nopencode/exo-free\n";
        assert_eq!(
            parse_models(opencode)[1],
            ModelInfo {
                id: "opencode/exo-free".into(),
                label: "opencode/exo-free".into()
            }
        );
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderInfo {
    #[serde(flatten)]
    pub provider: Provider,
    pub installed: bool,
}

static PROVIDERS: OnceLock<Vec<Provider>> = OnceLock::new();

/// The agents winfer ships with (resources/providers.json).
pub fn builtin() -> &'static [Provider] {
    PROVIDERS.get_or_init(|| {
        serde_json::from_str(include_str!("../resources/providers.json"))
            .expect("resources/providers.json is valid")
    })
}

/// Built-in agents followed by those from enabled plugins.
pub fn all() -> Vec<Provider> {
    let mut list = builtin().to_vec();
    list.extend(crate::plugins::providers());
    list
}

pub fn find(id: &str) -> Option<Provider> {
    all().into_iter().find(|p| p.id == id)
}

pub fn is_installed(provider: &Provider) -> bool {
    let Some(command) = &provider.command else {
        return true;
    };
    crate::env::find_command(command).is_some()
}

/// Whether the installed OpenCode is version 2 or later, which has `--standalone` and the plugin
/// API our status plugin uses. Asked once.
pub fn opencode_v2(launcher: &crate::env::Launcher) -> bool {
    static V2: OnceLock<bool> = OnceLock::new();
    *V2.get_or_init(|| {
        let output = crate::env::no_window(
            std::process::Command::new(&launcher.program)
                .args(&launcher.args)
                .arg("--version")
                .env("PATH", crate::env::path_var()),
        )
        .output();
        output.is_ok_and(|o| opencode_major(&String::from_utf8_lossy(&o.stdout)) >= Some(2))
    })
}

/// Major version from `opencode --version` ("opencode v2.0.28", "0.15.3").
fn opencode_major(text: &str) -> Option<u32> {
    let start = text.find(|c: char| c.is_ascii_digit())?;
    text[start..].split('.').next()?.parse().ok()
}

/// Async so that asking OpenCode its version stays off the main thread.
#[tauri::command]
pub async fn providers_list() -> Vec<ProviderInfo> {
    let list = || {
        all()
            .into_iter()
            .map(|mut p| {
                let installed = is_installed(&p);
                // Only OpenCode 2 gets our status plugin (see pty.rs); OpenCode 1 keeps the
                // output heuristics.
                if p.id == "opencode" && p.status_hooks {
                    p.status_hooks = installed
                        && crate::hooks::server().is_some()
                        && p.command.as_deref().and_then(crate::env::launcher).is_some_and(|l| opencode_v2(&l));
                }
                ProviderInfo { installed, provider: p }
            })
            .collect()
    };
    tauri::async_runtime::spawn_blocking(list).await.unwrap_or_default()
}

#[cfg(test)]
mod live {
    /// Lists models from the installed CLIs (machine-specific): cargo test -- --ignored live
    #[test]
    #[ignore]
    fn live_model_lists() {
        for id in ["claude", "agy", "cursor", "opencode", "codex"] {
            let p = super::find(id).unwrap();
            if !super::is_installed(&p) {
                continue;
            }
            let models = super::models(&p);
            eprintln!(
                "{id}: {} models, e.g. {:?}",
                models.len(),
                models.iter().take(3).map(|m| &m.id).collect::<Vec<_>>()
            );
        }
    }
}
