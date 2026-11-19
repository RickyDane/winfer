//! Model names as the pane headers show them (port of `features/code/names.ts` and `lib/modelName.ts`).

use parking_lot::Mutex;
use std::collections::HashMap;
use std::sync::LazyLock;

/// winfer code's backends: their spec prefix and name (see `harness::backend::Kind`).
pub const BACKENDS: [(&str, &str); 9] = [
    ("chatgpt", "ChatGPT"),
    ("claude", "Claude Code"),
    ("codex", "Codex"),
    ("agy", "Antigravity"),
    ("cursor", "Cursor"),
    ("opencode", "OpenCode"),
    ("pi", "Pi"),
    ("muse", "Muse Code"),
    ("local", "Local model"),
];

/// The backend a spec runs on (`opencode/anthropic/claude-sonnet` → opencode).
pub fn backend_of(spec: &str) -> Option<(&'static str, &'static str)> {
    BACKENDS.into_iter().find(|(id, _)| spec.starts_with(&format!("{id}/")) || spec == *id)
}

/// A `backend/model` spec, as opposed to a model id that contains a slash.
pub fn is_spec(s: &str) -> bool {
    backend_of(s).is_some()
}

fn tail(spec: &str) -> &str {
    if backend_of(spec).is_some() { spec.split_once('/').map_or(spec, |(_, t)| t) } else { spec }
}

static RESOLVED: LazyLock<Mutex<HashMap<String, String>>> = LazyLock::new(Default::default);

/// Remembers what an alias (claude/opus) resolved to, so the picker shows versions for every one seen.
pub fn remember_resolved(spec: &str, actual: &str) {
    if actual.is_empty() || actual == tail(spec) {
        return;
    }
    RESOLVED.lock().insert(spec.to_string(), actual.to_string());
}

fn alias(spec: &str) -> Option<&'static str> {
    Some(match spec {
        "claude/fable" => "claude-fable-5-1",
        "claude/opus" => "claude-opus-5-5",
        "claude/sonnet" => "claude-sonnet-5-5",
        "claude/haiku" => "claude-haiku-5-5",
        _ => return None,
    })
}

/// `claude/opus` (resolved to claude-opus-5-5) → "Opus 5.5"; `codex/gpt-6.1-sol-fast` → "GPT 6.1 Sol".
pub fn model_name(spec: &str, actual: Option<&str>) -> String {
    if let Some(a) = actual.filter(|a| !a.is_empty()) {
        return display_model_name(a);
    }
    if let Some(r) = RESOLVED.lock().get(spec) {
        return display_model_name(r);
    }
    display_model_name(alias(spec).unwrap_or_else(|| tail(spec)))
}

const SUFFIXES: [&str; 10] = ["fast", "minimal", "low", "medium", "high", "xhigh", "max", "latest", "preview", "thinking"];

fn all_digits(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_ascii_digit())
}

fn dotted(s: &str) -> bool {
    !s.is_empty() && s.split('.').all(all_digits)
}

/// Human-friendly model name: drops the provider and speed/effort/date suffixes.
pub fn display_model_name(model: &str) -> String {
    let base = model.rsplit('/').next().unwrap_or(model);
    let mut stripped = String::new();
    let mut depth = 0;
    for ch in base.chars() {
        match ch {
            '[' => depth += 1,
            ']' if depth > 0 => depth -= 1,
            c if depth == 0 => stripped.push(c),
            _ => {}
        }
    }
    let mut tokens: Vec<String> = stripped.split(|c: char| c == '-' || c == '_' || c.is_whitespace()).filter(|t| !t.is_empty()).map(String::from).collect();
    while tokens.len() > 1 {
        let last = tokens[tokens.len() - 1].to_lowercase();
        if SUFFIXES.contains(&last.as_str()) || (last.len() >= 6 && all_digits(&last)) {
            tokens.pop();
        } else {
            break;
        }
    }
    let starts_digit = |t: &String| t.chars().next().is_some_and(|c| c.is_ascii_digit());
    if tokens.first().is_some_and(|t| t.eq_ignore_ascii_case("claude")) && tokens.iter().skip(1).any(|t| !starts_digit(t)) {
        tokens.remove(0);
        if let Some(name) = tokens.iter().position(|t| !starts_digit(t)) {
            if name > 0 {
                let t = tokens.remove(name);
                tokens.insert(0, t);
            }
        }
    }
    let mut words: Vec<String> = Vec::new();
    for t in tokens {
        let numeric = all_digits(&t);
        if numeric && words.last().is_some_and(|p| dotted(p)) {
            let last = words.last_mut().unwrap();
            last.push('.');
            last.push_str(&t);
        } else if matches!(t.to_lowercase().as_str(), "gpt" | "glm") {
            words.push(t.to_uppercase());
        } else {
            let mut cs = t.chars();
            let first = cs.next().unwrap();
            let o_version = first == 'o' && cs.clone().next().is_some_and(|c| c.is_ascii_digit());
            if first.is_ascii_lowercase() && !o_version {
                words.push(first.to_ascii_uppercase().to_string() + cs.as_str());
            } else {
                words.push(t);
            }
        }
    }
    if words.is_empty() { model.to_string() } else { words.join(" ") }
}

const FAST: &str = "-fast";

/// A backend's fast-tier twin of a model (`codex/gpt-6.1-sol-fast`).
pub fn is_fast(spec: &str) -> bool {
    spec.ends_with(FAST) && tail(spec) != FAST
}

pub fn base_spec(spec: &str) -> String {
    if is_fast(spec) { spec[..spec.len() - FAST.len()].to_string() } else { spec.to_string() }
}

/// The fast twin's spec whether or not the backend lists it (Codex maps any `-fast` model to its
/// fast service tier), for the backends that have one.
pub fn fast_twin(spec: &str) -> Option<String> {
    spec.starts_with("codex/").then(|| format!("{}{FAST}", base_spec(spec)))
}

/// The spec of a model's fast twin, when the backend lists one.
pub fn fast_spec(spec: &str, models: &[super::protocol::ModelInfo]) -> Option<String> {
    let f = format!("{}{FAST}", base_spec(spec));
    models.iter().any(|m| m.spec == f).then_some(f)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModelTier {
    Fast,
    Balanced,
    Frontier,
}

fn has_word(text: &str, words: &[&str]) -> bool {
    text.split(|c: char| !c.is_alphanumeric()).any(|w| words.contains(&w))
}

const FAST_WORDS: [&str; 10] = ["haiku", "mini", "nano", "flash", "lite", "small", "spark", "instant", "fast", "fastest"];
const FRONTIER_WORDS: [&str; 12] = ["fable", "opus", "pro", "max", "ultra", "frontier", "flagship", "complex", "deep", "hardest", "capable", "cheap"];

/// What a model is for, from its name and the backend's description of it.
pub fn model_tier(spec: &str, detail: &str) -> ModelTier {
    let base = base_spec(spec);
    let name = base.split('/').skip(1).collect::<Vec<_>>().join(" ").to_lowercase();
    let d = detail.to_lowercase();
    if has_word(&name, &FAST_WORDS) || has_word(&name, &["cheap"]) {
        return ModelTier::Fast;
    }
    if has_word(&name, &FRONTIER_WORDS[..10]) || name.contains("most capable") {
        return ModelTier::Frontier;
    }
    if has_word(&d, &FAST_WORDS) || has_word(&d, &["cheap"]) {
        return ModelTier::Fast;
    }
    if has_word(&d, &FRONTIER_WORDS[..10]) || d.contains("most capable") {
        return ModelTier::Frontier;
    }
    ModelTier::Balanced
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn friendly_names() {
        assert_eq!(display_model_name("codex/gpt-6.1-sol-fast"), "GPT 6.1 Sol");
        assert_eq!(display_model_name("claude-opus-4-7"), "Opus 4.7");
        assert_eq!(display_model_name("claude-3-5-haiku"), "Haiku 3.5");
        assert_eq!(display_model_name("claude-opus-5-5-20261001"), "Opus 5.5");
        assert_eq!(display_model_name("o3-mini"), "o3 Mini");
        assert_eq!(display_model_name("opus[1m]"), "Opus");
    }

    #[test]
    fn specs_and_names() {
        assert!(is_spec("claude/opus"));
        assert!(!is_spec("anthropic/claude-sonnet"));
        assert_eq!(model_name("claude/opus", None), "Opus 5.5");
        assert_eq!(model_name("claude/opus", Some("claude-opus-5-6")), "Opus 5.6");
        assert_eq!(model_name("opencode/anthropic/claude-sonnet-4-5", None), "Sonnet 4.5");
    }

    #[test]
    fn fast_twins_and_tiers() {
        assert!(is_fast("codex/gpt-6.1-sol-fast"));
        assert_eq!(base_spec("codex/gpt-6.1-sol-fast"), "codex/gpt-6.1-sol");
        assert_eq!(fast_twin("codex/gpt-6-luna").as_deref(), Some("codex/gpt-6-luna-fast"));
        assert_eq!(fast_twin("codex/gpt-6-luna-fast").as_deref(), Some("codex/gpt-6-luna-fast"));
        assert_eq!(fast_twin("claude/opus"), None);
        assert_eq!(model_tier("claude/haiku", ""), ModelTier::Fast);
        assert_eq!(model_tier("claude/opus", ""), ModelTier::Frontier);
        assert_eq!(model_tier("codex/gpt-6.1-sol", "Latest workhorse"), ModelTier::Balanced);
        assert_eq!(model_tier("muse/wonder", "most capable"), ModelTier::Frontier);
    }
}
