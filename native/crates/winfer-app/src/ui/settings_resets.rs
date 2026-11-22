//! Which settings can be reset, and whether each differs from its default. Rows show a mark and
//! a reset button for their entry, the menu dots pages that have changes, and "Reset page"
//! resets them all. The Tauri app has the same list (`src/features/settings/resets.ts`).

use crate::model::{AppState, DEFAULT_TERMINAL_FONT_SIZE};
use serde_json::{Value, json};

pub struct Reset {
    pub id: &'static str,
    /// The settings page it's on.
    pub page: &'static str,
    pub label: &'static str,
    pub changed: fn(&AppState) -> bool,
    pub reset: fn(&mut AppState),
}

const DEFAULT_PROGRESS_INTERVAL: u64 = 60;

fn rest_get<'a>(s: &'a AppState, group: &str, key: &str) -> Option<&'a Value> {
    s.rest.get(group).and_then(|g| g.get(key))
}

fn rest_set(s: &mut AppState, group: &str, key: &str, value: Value) {
    let entry = s.rest.entry(group.to_string()).or_insert_with(|| json!({}));
    if !entry.is_object() {
        *entry = json!({});
    }
    entry[key] = value;
}

pub const RESETS: &[Reset] = &[
    Reset { id: "terminal.size", page: "general", label: "Text size", changed: |s| s.terminal_font_size != DEFAULT_TERMINAL_FONT_SIZE, reset: |s| s.set_terminal_font_size(DEFAULT_TERMINAL_FONT_SIZE) },
    Reset { id: "editor", page: "general", label: "Code editor", changed: |s| s.editor.is_some(), reset: |s| s.editor = None },
    Reset { id: "shortcuts", page: "general", label: "Keyboard shortcuts", changed: |s| !s.shortcuts.is_empty(), reset: |s| s.shortcuts.clear() },
    Reset {
        id: "theme",
        page: "theme",
        label: "Light or dark",
        changed: |s| s.theme != "dark" || s.theme_mode != "manual" || s.theme_picks.get("light").is_some() || s.theme_picks.get("dark").is_some_and(|p| p != "dark"),
        reset: |s| {
            s.theme = "dark".into();
            s.theme_mode = "manual".into();
            s.theme_picks.clear();
        },
    },
    Reset { id: "agents.default", page: "agents", label: "Default agent", changed: |s| s.default_provider.as_deref().is_some_and(|p| p != "winfer"), reset: |s| s.default_provider = None },
    Reset { id: "agents.models", page: "agents", label: "Models", changed: |s| !s.agent_models.is_empty(), reset: |s| s.agent_models.clear() },
    Reset { id: "agents.autoApprove", page: "agents", label: "Auto-approve", changed: |s| !s.auto_approve.is_empty(), reset: |s| s.auto_approve.clear() },
    Reset { id: "agents.prompt", page: "agents", label: "System prompt", changed: |s| s.system_prompt.is_some(), reset: |s| s.system_prompt = None },
    Reset { id: "agents.tools", page: "agents", label: "Optional tools", changed: |s| !s.enabled_tools.is_empty(), reset: |s| s.enabled_tools.clear() },
    Reset { id: "review.tests", page: "agents", label: "Run tests when an agent finishes", changed: |s| rest_get(s, "review", "autoTests") == Some(&Value::Bool(true)), reset: |s| rest_set(s, "review", "autoTests", Value::Bool(false)) },
    Reset { id: "review.screenshots", page: "agents", label: "Screenshot the dev server", changed: |s| rest_get(s, "review", "screenshots") == Some(&Value::Bool(false)), reset: |s| rest_set(s, "review", "screenshots", Value::Bool(true)) },
    Reset { id: "progress.enabled", page: "agents", label: "Estimate how far working agents are", changed: |s| rest_get(s, "progress", "enabled") == Some(&Value::Bool(true)), reset: |s| rest_set(s, "progress", "enabled", Value::Bool(false)) },
    Reset {
        id: "progress.interval",
        page: "agents",
        label: "Check every",
        changed: |s| rest_get(s, "progress", "intervalSec").and_then(Value::as_u64).is_some_and(|n| n != DEFAULT_PROGRESS_INTERVAL),
        reset: |s| rest_set(s, "progress", "intervalSec", json!(DEFAULT_PROGRESS_INTERVAL)),
    },
    Reset { id: "voice.readback", page: "voice", label: "Speak agent updates aloud", changed: |s| rest_get(s, "voice", "readback") == Some(&Value::Bool(true)), reset: |s| rest_set(s, "voice", "readback", Value::Bool(false)) },
    Reset { id: "usage.cost", page: "usage", label: "Show task cost on panes", changed: |s| s.show_pane_cost, reset: |s| s.show_pane_cost = false },
    Reset { id: "plugins.icons", page: "plugins", label: "Hide names on plugin tabs", changed: |s| s.page_tab_icons_only, reset: |s| s.page_tab_icons_only = false },
];

pub fn find(id: &str) -> Option<&'static Reset> {
    RESETS.iter().find(|r| r.id == id)
}

/// The entries on `page` that differ from their defaults.
pub fn changed_on(s: &AppState, page: &str) -> Vec<&'static Reset> {
    RESETS.iter().filter(|r| r.page == page && (r.changed)(s)).collect()
}

/// Resets every changed setting on `page`; returns how many.
pub fn reset_page(s: &mut AppState, page: &str) -> usize {
    let entries = changed_on(s, page);
    for r in &entries {
        (r.reset)(s);
    }
    entries.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_unique_and_pages_known() {
        let mut ids: Vec<&str> = RESETS.iter().map(|r| r.id).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), RESETS.len());
        for r in RESETS {
            assert!(["general", "theme", "agents", "voice", "usage", "plugins"].contains(&r.page), "{}", r.id);
        }
    }

    #[test]
    fn a_fresh_state_has_nothing_changed() {
        let s = AppState::new();
        for r in RESETS {
            assert!(!(r.changed)(&s), "{} counts as changed by default", r.id);
        }
    }

    #[test]
    fn reset_page_puts_changed_settings_back_and_leaves_other_pages() {
        let mut s = AppState::new();
        s.default_provider = Some("codex".into());
        s.agent_models.insert("claude".into(), "opus".into());
        rest_set(&mut s, "review", "autoTests", Value::Bool(true));
        rest_set(&mut s, "progress", "intervalSec", json!(300));
        s.show_pane_cost = true;
        assert_eq!(changed_on(&s, "agents").len(), 4);
        assert_eq!(reset_page(&mut s, "agents"), 4);
        assert!(s.default_provider.is_none() && s.agent_models.is_empty());
        assert_eq!(rest_get(&s, "review", "autoTests"), Some(&Value::Bool(false)));
        assert!(s.show_pane_cost);
        assert!(changed_on(&s, "agents").is_empty());
    }

    #[test]
    fn the_default_interval_is_not_a_change() {
        let mut s = AppState::new();
        rest_set(&mut s, "progress", "intervalSec", json!(60));
        assert!(!(find("progress.interval").unwrap().changed)(&s));
    }

    #[test]
    fn theme_reset_clears_picks_and_system_mode() {
        let mut s = AppState::new();
        s.theme_mode = "system".into();
        s.pick_theme("github-light", false);
        assert_eq!(s.theme, "github-light");
        assert!((find("theme").unwrap().changed)(&s));
        reset_page(&mut s, "theme");
        assert_eq!((s.theme.as_str(), s.theme_mode.as_str(), s.theme_picks.len()), ("dark", "manual", 0));
    }

    #[test]
    fn following_the_system_uses_the_pick_for_its_mode() {
        let mut s = AppState::new();
        s.theme_mode = "system".into();
        s.pick_theme("github-dark", true);
        s.pick_theme("github-light", true); // a light pick while the system is dark leaves the theme alone
        assert_eq!(s.theme, "github-dark");
        assert_eq!(s.theme_for_system(true), "github-dark");
        assert_eq!(s.theme_for_system(false), "github-light");
        assert_eq!(AppState::new().theme_for_system(false), crate::theme::theme_by_id("dark").pair);
    }
}
