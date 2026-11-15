//! Local models: an OpenAI-compatible server (Settings → winfer code → Local models, see
//! local_llm.rs), run on the ChatGPT backend's agent loop with winfer's own tools. What differs
//! from OpenAI lives here: the model list, the effort levels and how a request is adapted.

use std::collections::BTreeMap;
use std::sync::mpsc::Receiver;

use serde_json::{Map, Value, json};

use super::{In, Opts};
use crate::harness::config::LocalLlm;
use crate::harness::event::{Ev, Events, ModelInfo};
use crate::harness::tools::Outcome;
use crate::local_llm::EFFORT_RANK;

/// The settings a running session uses (Settings → Local models), read when it starts.
pub struct Local {
    /// Pass the effort to the chat template too.
    pub template_effort: bool,
    /// Thinking-token caps per effort level.
    pub thinking_budgets: BTreeMap<String, u64>,
    /// The server, to read each model's levels from its chat template (None in tests).
    pub server: Option<LocalLlm>,
    /// The server searches the web (oMLX), so the models get `web_search`.
    pub web_search: bool,
}

/// The tool searching the web through the local server.
pub const WEB_SEARCH: &str = "web_search";

impl Local {
    /// Tools the local server adds to winfer's own.
    pub(super) fn tool_definitions(&self) -> Vec<Value> {
        if !self.web_search {
            return Vec::new();
        }
        vec![json!({
            "type": "function",
            "name": WEB_SEARCH,
            "description": "Search the web; returns titles, URLs and snippets. Use for current facts or finding pages, then fetch a result for detail.",
            "parameters": { "type": "object", "properties": { "query": { "type": "string" } }, "required": ["query"] },
        })]
    }

    /// Runs one of [`Self::tool_definitions`]; None for any other tool.
    pub(super) fn run_tool(&self, name: &str, args: &Value) -> Option<Outcome> {
        if name != WEB_SEARCH || !self.web_search {
            return None;
        }
        let query = args["query"].as_str().unwrap_or_default().trim();
        if query.is_empty() {
            return Some(Outcome::err("query is required"));
        }
        let server = self.server.as_ref()?;
        Some(match crate::local_llm::web_search(server, query) {
            Ok(text) => Outcome::ok(text),
            Err(e) => Outcome::err(e),
        })
    }
}

impl Local {
    /// Adapts a Responses API request for the local server: drops OpenAI-only fields, caps the
    /// output to what the context has room for, and applies the effort (chat template, thinking
    /// budget and a note telling the model about the budget).
    pub(super) fn adapt(&self, o: &mut Map<String, Value>, instructions: &str, effort: Option<&str>, window: u64, cap: Option<u64>) {
        // Local servers keep nothing between requests and many reject OpenAI-only fields.
        o.remove("include");
        o.remove("prompt_cache_key");
        if let Some(n) = effort.and_then(|e| self.thinking_budgets.get(e)).copied().filter(|n| *n > 0) {
            o.insert("instructions".into(), json!(format!("{instructions}\n\n{}", thinking_budget_note(n))));
            o.insert("thinking_budget".into(), json!(n));
        }
        // Without a limit, servers apply a small default that long reasoning runs into; allow
        // what the context has room for.
        let bytes = serde_json::to_string(o).map_or(0, |s| s.len());
        let budget = super::chatgpt::output_budget(window, bytes);
        o.insert("max_output_tokens".into(), json!(cap.map_or(budget, |c| c.min(budget))));
        // Only a level the model's template takes: llama.cpp fails the request on any other.
        let model = o.get("model").and_then(Value::as_str).unwrap_or_default().to_string();
        let levels = self.server.as_ref().and_then(|c| crate::local_llm::template_levels(c, &model));
        let effort = effort.and_then(|e| fit_effort(e, levels.as_deref()));
        let effort = effort.as_deref();
        if let Some(reasoning) = o.get_mut("reasoning").and_then(Value::as_object_mut) {
            match effort {
                Some(e) => reasoning.insert("effort".into(), json!(e)),
                None => reasoning.remove("effort"),
            };
        }
        if let Some(kwargs) = template_kwargs(effort, self.template_effort) {
            o.insert("chat_template_kwargs".into(), kwargs);
        }
    }
}

/// The models on the local server (Settings → winfer code → Local models), as `local/<id>`;
/// empty when that's off or the server isn't up.
pub fn models() -> Vec<ModelInfo> {
    if !crate::local_llm::enabled() {
        return Vec::new();
    }
    let address = crate::local_llm::address();
    let local = crate::harness::config::Config::load().local;
    // The levels each model's chat template takes when the server shows it, else the configured ones.
    let configured: Vec<String> = local
        .efforts
        .iter()
        .map(|e| match e.trim().to_ascii_lowercase().as_str() {
            "off" => "none".to_string(),
            e => e.to_string(),
        })
        .filter(|e| !e.is_empty())
        .collect();
    crate::local_llm::list(&local)
        .unwrap_or_default()
        .into_iter()
        .map(|m| ModelInfo {
            spec: format!("local/{}", m.id),
            efforts: Some(crate::local_llm::template_levels(&local, &m.id).unwrap_or_else(|| configured.clone())),
            label: m.id,
            detail: match m.context {
                Some(c) => format!("Local · {address} · {}k context · winfer tools", c / 1000),
                None => format!("Local · {address} · winfer tools"),
            },
        })
        .collect()
}

/// The same agent loop on the local model server.
pub fn run(mut opts: Opts, rx: Receiver<In>, events: Events) {
    if opts.model.is_empty() {
        match crate::local_llm::list(&crate::harness::config::Config::load().local) {
            Ok(list) if !list.is_empty() => opts.model = list[0].id.clone(),
            Ok(_) => {
                let _ = events.send(Ev::Exited(format!("The local model server at {} lists no models.", crate::local_llm::address())));
                return;
            }
            Err(e) => {
                let _ = events.send(Ev::Exited(format!("{e}. Start it, or change its address in winfer → Settings → winfer code → Local models.")));
                return;
            }
        }
    }
    let config = crate::harness::config::Config::load().local;
    let web_search = config.web_search && crate::local_llm::has_web_search(&config);
    let local = Local { template_effort: config.template_effort, thinking_budgets: config.thinking_budgets.clone(), server: Some(config), web_search };
    super::chatgpt::run_with(opts, rx, events, Some(local));
}

/// What a local model's chat template gets for `effort`: "none" switches thinking off (Qwen's
/// `enable_thinking`); another level goes in as `reasoning_effort` when `to_template` is on.
fn template_kwargs(effort: Option<&str>, to_template: bool) -> Option<Value> {
    match effort? {
        "none" => Some(json!({ "enable_thinking": false })),
        e if to_template => Some(json!({ "enable_thinking": true, "reasoning_effort": e })),
        _ => None,
    }
}

/// The level of `levels` (a model's template's, None: unknown) closest to `effort`, the way
/// Unsloth Studio fits them: "high" and "max" go to "xhigh" on Qwen3.8, "minimal" to "low"; a
/// tie goes up. None when nothing fits, e.g. "none" on a template that can't switch thinking off.
fn fit_effort(effort: &str, levels: Option<&[String]>) -> Option<String> {
    let Some(levels) = levels else { return Some(effort.to_string()) };
    if levels.iter().any(|l| l == effort) {
        return Some(effort.to_string());
    }
    let rank = |e: &str| EFFORT_RANK.iter().position(|r| *r == e).map(|r| r as i64);
    let want = rank(effort)?;
    levels
        .iter()
        .filter_map(|l| rank(l).map(|r| (r, l)))
        .min_by_key(|(r, _)| ((r - want).abs(), -r))
        .map(|(_, l)| l.clone())
}

/// Tells a local model its thinking is capped at `n` tokens (the server cuts it off there), so it
/// plans for that instead of being stopped mid-thought. Appended to the instructions, so the
/// prefix cache only misses when the effort changes.
fn thinking_budget_note(n: u64) -> String {
    format!(
        "# Thinking budget\nYour thinking is limited to about {n} tokens per response; anything past that is cut off. \
         Keep reasoning short and focused: settle the key question first, skip re-checking what is already clear, \
         and leave room to act. For a long task, think about the next step only, then call a tool or answer; \
         you can think again after each tool result."
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thinking_budget_note_names_the_budget() {
        assert!(thinking_budget_note(1024).contains("about 1024 tokens"));
    }

    #[test]
    fn effort_reaches_the_chat_template() {
        assert_eq!(template_kwargs(None, true), None);
        assert_eq!(template_kwargs(Some("none"), false), Some(json!({ "enable_thinking": false })));
        assert_eq!(template_kwargs(Some("xhigh"), true), Some(json!({ "enable_thinking": true, "reasoning_effort": "xhigh" })));
        assert_eq!(template_kwargs(Some("low"), false), None);
    }

    #[test]
    fn effort_fits_the_template() {
        let qwen: Vec<String> = ["none", "low", "medium", "xhigh"].map(String::from).into();
        let fit = |e| fit_effort(e, Some(&qwen));
        assert_eq!(fit("low").as_deref(), Some("low"));
        assert_eq!(fit("high").as_deref(), Some("xhigh"));
        assert_eq!(fit("max").as_deref(), Some("xhigh"));
        assert_eq!(fit("minimal").as_deref(), Some("low"));
        assert_eq!(fit("none").as_deref(), Some("none"));
        assert_eq!(fit_effort("none", Some(&["low".to_string()])), None);
        assert_eq!(fit_effort("high", None).as_deref(), Some("high"));
        assert_eq!(fit_effort("low", Some(&[])), None);
    }

    #[test]
    fn budget_adds_the_cap_and_the_note() {
        let local = Local { template_effort: true, thinking_budgets: [("low".to_string(), 1024)].into(), server: None, web_search: false };
        let mut o = json!({ "instructions": "base", "include": [], "prompt_cache_key": "k" }).as_object().unwrap().clone();
        local.adapt(&mut o, "base", Some("low"), 32_000, None);
        assert_eq!(o["thinking_budget"], json!(1024));
        assert!(o["instructions"].as_str().unwrap().starts_with("base\n\n# Thinking budget"));
        assert!(!o.contains_key("include") && !o.contains_key("prompt_cache_key"));
        let mut o = json!({ "instructions": "base" }).as_object().unwrap().clone();
        local.adapt(&mut o, "base", Some("xhigh"), 32_000, None);
        assert!(!o.contains_key("thinking_budget"));
        assert_eq!(o["instructions"], json!("base"));
    }
}
