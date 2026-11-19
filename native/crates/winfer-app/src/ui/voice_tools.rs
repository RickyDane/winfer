//! Everything live voice can do, as Realtime API function tools: winfer's UI actions (the tool
//! registry of `tools/registry.ts`, mapped onto the command bus) and the agent tools agents get
//! over MCP (port of `features/voice/live/tools.ts`).

use super::commands::{Command, PaneRef, ViewTarget};
use super::*;
use crate::model::Overlay;
use crate::model::layout::Preset;
use serde_json::{Value, json};

pub const END_CONVERSATION: &str = "end_conversation";

/// The pane arguments every pane tool takes.
fn pane_param() -> Value {
    json!({ "type": "string", "description": "Pane number as listed (1, 2, …) or pane id" })
}

fn target_param() -> Value {
    json!({ "type": "string", "description": "Pane number (1, 2, …) or id; focused (default) for the focused pane; all for every pane" })
}

fn tool(name: &str, description: &str, properties: Value, required: &[&str]) -> Value {
    let mut parameters = json!({ "type": "object", "properties": properties });
    if !required.is_empty() {
        parameters["required"] = json!(required);
    }
    json!({ "type": "function", "name": name, "description": description, "parameters": parameters })
}

/// Tools the model must confirm with the user before running.
const DESTRUCTIVE: [&str; 5] = ["close_pane", "close_all_panes", "discard_worktree", "merge_worktree", "interrupt_all"];

/// The agent tools, described for voice (no calling pane: "the user's workspace").
pub fn agent_tools() -> Vec<Value> {
    vec![
        tool("list_panes", "List the panes in the current workspace: number, agent, status (working, needs-input, done, idle, error), branch.", json!({}), &[]),
        tool("read_pane", "Read the latest terminal output of a pane, e.g. to tell the user what an agent did, is doing or is asking.", json!({ "pane": pane_param(), "lines": { "type": "integer", "description": "Lines from the end (default 60)" } }), &["pane"]),
        tool("send_to_pane", "Type a message into a pane, e.g. an instruction for an agent or a command in a shell pane. Submits unless submit is false. Write the message as the agent should receive it.", json!({ "pane": pane_param(), "text": { "type": "string" }, "submit": { "type": "boolean" } }), &["pane", "text"]),
        tool("spawn_agent", "Start one new agent pane, optionally with a first prompt and its own git worktree. Providers: the installed agent ids, or shell.", json!({ "provider": { "type": "string" }, "prompt": { "type": "string" }, "worktree": { "type": "boolean" } }), &["provider"]),
        tool("open_url", "Open a URL in winfer's browser panel (reuses a tab already on that site unless new_tab).", json!({ "url": { "type": "string" }, "new_tab": { "type": "boolean" } }), &["url"]),
        tool("open_file", "Show a file in winfer's file viewer (markdown rendered, images shown). Relative paths start at the workspace folder.", json!({ "path": { "type": "string" }, "line": { "type": "number" } }), &["path"]),
        tool("usage_status", "How much of an agent's plan usage is left (5-hour and weekly windows, resets). Defaults to the focused pane's agent.", json!({ "pane": pane_param() }), &[]),
    ]
}

/// winfer's UI actions as tools.
pub fn registry_tools(providers: &[(String, String)]) -> Vec<Value> {
    let names: Vec<String> = providers.iter().map(|(id, name)| format!("{id} ({name})")).collect();
    let counts: Value = providers.iter().map(|(id, _)| (id.clone(), json!({ "type": "integer", "minimum": 0, "maximum": 9 }))).collect::<serde_json::Map<_, _>>().into();
    let confirmed = json!({ "type": "boolean", "description": "True only after the user said yes to doing this" });
    let mut tools = vec![
        tool("spawn_panes", &format!("Open, start, launch or spin up new agents, terminals or windows, one or several, optionally with a specific model or in their own worktrees. Names may be misheard: cloud means Claude, codecs means Codex. Providers: {}", names.join(", ")),
            json!({ "counts": { "type": "object", "description": "How many new panes of each provider", "properties": counts }, "total": { "type": "integer", "minimum": 0, "maximum": 9, "description": "How many new panes in total when no provider is named" }, "prompt": { "type": "string", "description": "What the new agents should be told to do, if anything" }, "model": { "type": "string", "description": "A model the user named, e.g. opus" }, "worktree": { "type": "boolean", "description": "Start each new agent in its own git worktree and branch" } }), &[]),
        tool("close_pane", "Close a pane and stop its agent.", json!({ "target": target_param() }), &[]),
        tool("close_all_panes", "Close every pane in the workspace.", json!({}), &[]),
        tool("focus_pane", "Focus a pane, or move focus left, right, up or down.", json!({ "target": target_param(), "direction": { "type": "string", "enum": ["left", "right", "up", "down"] } }), &[]),
        tool("focus_attention", "Focus the next pane that needs the user (asking, finished or failed).", json!({}), &[]),
        tool("zoom_pane", "Zoom a pane to fill the workspace, or restore it.", json!({ "target": target_param() }), &[]),
        tool("archive_pane", "Archive a pane (keep it running, out of the way), or bring it back.", json!({ "target": target_param(), "restore": { "type": "boolean" } }), &[]),
        tool("show_archive", "Show the archived panes.", json!({}), &[]),
        tool("show_overview", "Show every workspace's panes at once, or go back.", json!({}), &[]),
        tool("set_layout", "Arrange the panes with a layout preset.", json!({ "preset": { "type": "string", "enum": ["auto", "main-stack", "grid", "columns", "rows"] } }), &["preset"]),
        tool("equalize_panes", "Make all panes the same size.", json!({}), &[]),
        tool("switch_workspace", "Go to another workspace by number or name.", json!({ "workspace": { "type": "string", "description": "Workspace number (1, 2, …) or name" } }), &["workspace"]),
        tool("create_workspace", "Create a new workspace (asks for a folder).", json!({}), &[]),
        tool("change_model", "Restart a pane's agent on another model.", json!({ "target": target_param(), "model": { "type": "string" } }), &["model"]),
        tool("interrupt_agent", "Interrupt (Esc) an agent that is working.", json!({ "target": target_param() }), &[]),
        tool("interrupt_all", "Interrupt every agent in the workspace.", json!({}), &[]),
        tool("approve_agent", "Approve what an agent is asking permission for.", json!({ "target": target_param() }), &[]),
        tool("broadcast", "Send the same message to every agent (or the named panes).", json!({ "text": { "type": "string" }, "submit": { "type": "boolean" } }), &["text"]),
        tool("show_progress", "Show the sessions overview with each agent's progress.", json!({}), &[]),
        tool("show_history", "Show the history of finished work.", json!({}), &[]),
        tool("review_work", "Open the review inbox with finished work to look over and merge.", json!({}), &[]),
        tool("show_changes", "Show an agent's uncommitted changes (diff).", json!({ "target": target_param() }), &[]),
        tool("merge_worktree", "Merge a pane's worktree branch back.", json!({ "target": target_param() }), &[]),
        tool("discard_worktree", "Throw a pane's worktree and branch away.", json!({ "target": target_param() }), &[]),
        tool("toggle_worktrees", "Switch whether new agents get their own worktree.", json!({}), &[]),
        tool("open_in_editor", "Open a pane's folder in the user's editor.", json!({ "target": target_param() }), &[]),
        tool("toggle_sidebar", "Show or hide the sidebar.", json!({}), &[]),
        tool("toggle_browser", "Show or hide the side panel.", json!({}), &[]),
        tool("show_side_panel", "Show a tab of the side panel.", json!({ "panel": { "type": "string", "enum": ["browser", "android", "terminal", "files"] } }), &["panel"]),
        tool("browser_close_tab", "Close the browser's current tab.", json!({}), &[]),
        tool("pick_element", "Let the user pick an element on the page to send to an agent.", json!({}), &[]),
        tool("set_theme", "Change the color theme.", json!({ "theme": { "type": "string", "description": "A theme id" } }), &["theme"]),
        tool("open_settings", "Open the settings.", json!({ "page": { "type": "string", "description": "A settings page, e.g. accounts or voice" } }), &[]),
        tool("show_shortcuts", "Show the keyboard shortcuts.", json!({}), &[]),
        tool("load_recipe", "Start a saved recipe (a set of agents and a layout).", json!({ "recipe": { "type": "string", "description": "The recipe's id or name" } }), &["recipe"]),
    ];
    for t in &mut tools {
        let name = t["name"].as_str().unwrap_or_default().to_string();
        if DESTRUCTIVE.contains(&name.as_str()) {
            t["parameters"]["properties"]["confirmed"] = confirmed.clone();
            let d = format!("{} Ask the user to confirm first.", t["description"].as_str().unwrap_or_default());
            t["description"] = json!(d);
        }
    }
    tools
}

pub fn end_tool() -> Value {
    tool(END_CONVERSATION, "End the voice conversation, when the user says goodbye or asks you to stop listening.", json!({}), &[])
}

const MAX_ENUM: usize = 40;
const MAX_DESCRIPTION: usize = 300;

/// Plain JSON schema the realtime model accepts: only type/description/properties/required/items/enum,
/// a single type, short descriptions and enums.
pub fn sanitize_schema(schema: &Value) -> Value {
    let Some(src) = schema.as_object() else { return json!({ "type": "string" }) };
    let mut out = serde_json::Map::new();
    let ty = match &src.get("type") {
        Some(Value::String(t)) => t.clone(),
        Some(Value::Array(list)) => list.iter().filter_map(Value::as_str).find(|t| *t != "null").unwrap_or("string").to_string(),
        _ if src.contains_key("properties") => "object".into(),
        _ => "string".into(),
    };
    out.insert("type".into(), json!(ty));
    let mut description = src.get("description").and_then(Value::as_str).unwrap_or_default().to_string();
    if let Some(list) = src.get("enum").and_then(Value::as_array) {
        if list.len() <= MAX_ENUM {
            out.insert("enum".into(), Value::Array(list.clone()));
        } else {
            let some: Vec<String> = list.iter().take(MAX_ENUM).map(|v| v.as_str().map_or(v.to_string(), str::to_string)).collect();
            description = format!("{description} One of: {}, …", some.join(", ")).trim().to_string();
        }
    }
    if !description.is_empty() {
        out.insert("description".into(), json!(description.chars().take(MAX_DESCRIPTION).collect::<String>()));
    }
    if ty == "object" {
        let props = src.get("properties").and_then(Value::as_object).cloned().unwrap_or_default();
        out.insert("properties".into(), Value::Object(props.iter().map(|(k, v)| (k.clone(), sanitize_schema(v))).collect()));
        let required: Vec<&Value> = src.get("required").and_then(Value::as_array).map(|r| r.iter().filter(|n| n.as_str().is_some_and(|n| props.contains_key(n))).collect()).unwrap_or_default();
        if !required.is_empty() {
            out.insert("required".into(), json!(required));
        }
    }
    if ty == "array" {
        out.insert("items".into(), sanitize_schema(src.get("items").unwrap_or(&json!({ "type": "string" }))));
    }
    Value::Object(out)
}

/// A tool as the model gets it: short description, plain schema.
pub fn clean(tool: &Value) -> Value {
    json!({
        "type": "function",
        "name": tool["name"],
        "description": tool["description"].as_str().unwrap_or_default().chars().take(MAX_DESCRIPTION).collect::<String>(),
        "parameters": sanitize_schema(&tool["parameters"]),
    })
}

/// Whether a registry tool needs the user's yes first.
pub fn needs_confirmation(name: &str, args: &Value) -> bool {
    DESTRUCTIVE.contains(&name) && args["confirmed"] != true
}

/// A pane argument as a command target: a number, an id, or the focused pane.
pub fn pane_ref(v: &Value) -> PaneRef {
    match v {
        Value::Number(n) => n.as_u64().map_or(PaneRef::Focused, |n| PaneRef::Index(n as usize)),
        Value::String(s) if s.is_empty() || s == "focused" => PaneRef::Focused,
        Value::String(s) if s.bytes().all(|b| b.is_ascii_digit()) => PaneRef::Index(s.parse().unwrap_or(1)),
        Value::String(s) => PaneRef::Id(s.clone()),
        _ => PaneRef::Focused,
    }
}

/// What the registry needs to know about the app to turn a request into commands.
pub struct Context {
    /// (id, name) of installed providers.
    pub providers: Vec<(String, String)>,
    /// Pane ids in layout order.
    pub panes: Vec<String>,
    /// (id, name) of workspaces.
    pub workspaces: Vec<(String, String)>,
    pub themes: Vec<String>,
    /// (id, name) of saved recipes.
    pub recipes: Vec<(String, String)>,
}

fn preset(name: &str) -> Option<Preset> {
    Some(match name {
        "auto" => Preset::Auto,
        "main-stack" => Preset::MainStack,
        "grid" => Preset::Grid,
        "columns" => Preset::Columns,
        "rows" => Preset::Rows,
        _ => return None,
    })
}

/// The commands a registry tool stands for.
pub fn registry_commands(name: &str, a: &Value, ctx: &Context) -> Result<Vec<Command>, String> {
    use Command as C;
    let target = pane_ref(&a["target"]);
    let all = a["target"] == "all";
    let per_pane = |make: &dyn Fn(PaneRef) -> C| -> Vec<C> {
        if all { ctx.panes.iter().map(|id| make(PaneRef::Id(id.clone()))).collect() } else { vec![make(target.clone())] }
    };
    let text = |k: &str| a[k].as_str().map(str::trim).filter(|s| !s.is_empty()).map(str::to_string);
    Ok(match name {
        "spawn_panes" => {
            let prompt = text("prompt");
            let model = text("model").filter(|m| m != "default");
            let worktree = a["worktree"].as_bool();
            let mut out = Vec::new();
            if let Some(counts) = a["counts"].as_object() {
                for (id, n) in counts {
                    let n = n.as_u64().unwrap_or(0).min(9) as usize;
                    if n > 0 && ctx.providers.iter().any(|(p, _)| p == id) {
                        out.push(C::Spawn { provider: Some(id.clone()), count: n, prompt: prompt.clone(), worktree, model: model.clone() });
                    }
                }
            }
            if out.is_empty() {
                let total = a["total"].as_u64().unwrap_or(1).clamp(1, 9) as usize;
                out.push(C::Spawn { provider: None, count: total, prompt, worktree, model });
            }
            out
        }
        "close_pane" => per_pane(&|t| C::Close(t)),
        "close_all_panes" => vec![C::CloseAll],
        "focus_pane" => match text("direction").as_deref() {
            Some("left") => vec![C::FocusDir(super::commands::Direction::Left)],
            Some("right") => vec![C::FocusDir(super::commands::Direction::Right)],
            Some("up") => vec![C::FocusDir(super::commands::Direction::Up)],
            Some("down") => vec![C::FocusDir(super::commands::Direction::Down)],
            _ => vec![C::Focus(target)],
        },
        "focus_attention" => vec![C::NextAttention],
        "zoom_pane" => vec![C::Zoom(target)],
        "archive_pane" => per_pane(&|t| C::Archive { target: t, restore: a["restore"] == true }),
        "show_archive" => vec![C::View(ViewTarget::Set(View::Archive))],
        "show_overview" => vec![C::View(ViewTarget::Toggle)],
        "set_layout" => vec![C::LayoutPreset(Some(preset(&text("preset").unwrap_or_default()).ok_or("Unknown layout; use auto, main-stack, grid, columns or rows.")?))],
        "equalize_panes" => vec![C::LayoutEqualize],
        "switch_workspace" => {
            let w = text("workspace").ok_or("Which workspace?")?;
            match w.parse::<usize>() {
                Ok(n) if (1..=ctx.workspaces.len()).contains(&n) => vec![C::WorkspaceSwitchIndex(n)],
                _ => {
                    let lower = w.to_lowercase();
                    let found = ctx.workspaces.iter().find(|(id, name)| *id == w || name.to_lowercase() == lower).or_else(|| ctx.workspaces.iter().find(|(_, name)| name.to_lowercase().contains(&lower)));
                    vec![C::WorkspaceSwitch(found.ok_or_else(|| format!("No workspace “{w}”."))?.0.clone())]
                }
            }
        }
        "create_workspace" => vec![C::WorkspaceCreate],
        "change_model" => vec![C::SetModel { target, model: Some(text("model").ok_or("Which model?")?) }],
        "interrupt_agent" => per_pane(&|t| C::Interrupt(t)),
        "interrupt_all" => vec![C::InterruptAll],
        "approve_agent" => per_pane(&|t| C::Approve(t)),
        "broadcast" => vec![C::Broadcast { text: text("text").ok_or("What should I send?")?, submit: a["submit"] != false, pane_ids: None }],
        "show_progress" => vec![C::Overlay { overlay: Overlay::Progress, provider: None }],
        "show_history" => vec![C::Overlay { overlay: Overlay::History, provider: None }],
        "review_work" => vec![C::Overlay { overlay: Overlay::Review, provider: None }],
        "show_changes" => vec![C::ShowChanges(target)],
        "merge_worktree" => vec![C::MergeWorktree(target)],
        "discard_worktree" => vec![C::DiscardWorktree(target)],
        "toggle_worktrees" => vec![C::ToggleWorktrees],
        "open_in_editor" => vec![C::EditorOpen { target: Some(target), editor: None }],
        "toggle_sidebar" => vec![C::ToggleSidebar],
        "toggle_browser" => vec![C::ToggleBrowser],
        "show_side_panel" => vec![C::SidePanel {
            panel: match text("panel").as_deref() {
                Some("browser") => SidePanel::Browser,
                Some("android") | Some("device") => SidePanel::Device,
                Some("terminal") => SidePanel::Terminal,
                Some("files") | Some("viewer") => SidePanel::Viewer,
                _ => return Err("Unknown panel; use browser, android, terminal or files.".into()),
            },
            show: true,
        }],
        "browser_close_tab" => vec![C::BrowserCloseTab],
        "pick_element" => vec![C::BrowserPick],
        "set_theme" => {
            let t = text("theme").ok_or("Which theme?")?;
            let id = ctx.themes.iter().find(|id| id.eq_ignore_ascii_case(&t)).ok_or_else(|| format!("Unknown theme “{t}”. Themes: {}.", ctx.themes.join(", ")))?;
            vec![C::Theme(id.clone())]
        }
        "open_settings" => vec![C::Settings(text("page"))],
        "show_shortcuts" => vec![C::CheatSheet],
        "load_recipe" => {
            let r = text("recipe").ok_or("Which recipe?")?;
            let lower = r.to_lowercase();
            let found = ctx.recipes.iter().find(|(id, name)| *id == r || name.to_lowercase() == lower).or_else(|| ctx.recipes.iter().find(|(_, name)| name.to_lowercase().contains(&lower)));
            vec![C::RecipeLoad(found.ok_or_else(|| format!("No recipe “{r}”."))?.0.clone())]
        }
        _ => return Err(format!("Unknown tool {name}")),
    })
}

/// Whether `name` is one of the registry's tools.
pub fn is_registry_tool(name: &str) -> bool {
    registry_tools(&[]).iter().any(|t| t["name"] == name)
}

/// The live state of winfer as part of the instructions (see `stateSummary` in instructions.ts).
pub struct Summary {
    pub workspace: String,
    pub root: String,
    /// (number, provider name, id, status, extras)
    pub panes: Vec<(usize, String, String, String, String)>,
    pub other_workspaces: Vec<String>,
    pub agents: Vec<String>,
    pub browser_tabs: Vec<String>,
}

pub fn state_summary(s: &Summary) -> String {
    let panes: Vec<String> = s.panes.iter().map(|(n, name, id, status, extra)| format!("  {n}. {name} (id {id}), {status}{}", if extra.is_empty() { String::new() } else { format!(", {extra}") })).collect();
    let list = |v: &[String]| if v.is_empty() { "none".to_string() } else { v.join(", ") };
    [
        format!("Current workspace: {} ({})", s.workspace, s.root),
        format!("Panes:\n{}", if panes.is_empty() { "  none".to_string() } else { panes.join("\n") }),
        format!("Other workspaces: {}", list(&s.other_workspaces)),
        format!("Installed agents: {}", list(&s.agents)),
        format!("Browser tabs: {}", list(&s.browser_tabs)),
    ]
    .join("\n")
}

#[cfg(test)]
mod tests {
    use super::{Command, Context, PaneRef, Summary, clean, is_registry_tool, needs_confirmation, pane_ref, registry_commands, registry_tools, sanitize_schema, state_summary};
    use serde_json::{Value, json};

    fn ctx() -> Context {
        Context {
            providers: vec![("claude".into(), "Claude Code".into()), ("codex".into(), "Codex".into())],
            panes: vec!["p-a".into(), "p-b".into()],
            workspaces: vec![("w1".into(), "api".into()), ("w2".into(), "web app".into())],
            themes: vec!["dark".into(), "nord".into()],
            recipes: vec![("r1".into(), "Full stack".into())],
        }
    }

    #[test]
    fn spawns_per_provider_or_in_total() {
        let c = registry_commands("spawn_panes", &json!({ "counts": { "claude": 2, "codex": 0, "nope": 3 }, "prompt": "fix it", "worktree": true }), &ctx()).unwrap();
        assert_eq!(c, [Command::Spawn { provider: Some("claude".into()), count: 2, prompt: Some("fix it".into()), worktree: Some(true), model: None }]);
        let c = registry_commands("spawn_panes", &json!({ "total": 3, "model": "default" }), &ctx()).unwrap();
        assert_eq!(c, [Command::Spawn { provider: None, count: 3, prompt: None, worktree: None, model: None }]);
    }

    #[test]
    fn targets_resolve_to_panes() {
        assert_eq!(pane_ref(&json!("2")), PaneRef::Index(2));
        assert_eq!(pane_ref(&json!(3)), PaneRef::Index(3));
        assert_eq!(pane_ref(&json!("p-a")), PaneRef::Id("p-a".into()));
        assert_eq!(pane_ref(&json!("focused")), PaneRef::Focused);
        assert_eq!(pane_ref(&Value::Null), PaneRef::Focused);
        let c = registry_commands("interrupt_agent", &json!({ "target": "all" }), &ctx()).unwrap();
        assert_eq!(c, [Command::Interrupt(PaneRef::Id("p-a".into())), Command::Interrupt(PaneRef::Id("p-b".into()))]);
        assert_eq!(registry_commands("zoom_pane", &json!({}), &ctx()).unwrap(), [Command::Zoom(PaneRef::Focused)]);
    }

    #[test]
    fn workspaces_by_number_or_name() {
        assert_eq!(registry_commands("switch_workspace", &json!({ "workspace": "2" }), &ctx()).unwrap(), [Command::WorkspaceSwitchIndex(2)]);
        assert_eq!(registry_commands("switch_workspace", &json!({ "workspace": "WEB" }), &ctx()).unwrap(), [Command::WorkspaceSwitch("w2".into())]);
        assert!(registry_commands("switch_workspace", &json!({ "workspace": "zzz" }), &ctx()).is_err());
    }

    #[test]
    fn themes_layouts_and_recipes_are_checked() {
        assert_eq!(registry_commands("set_theme", &json!({ "theme": "Nord" }), &ctx()).unwrap(), [Command::Theme("nord".into())]);
        assert!(registry_commands("set_theme", &json!({ "theme": "neon" }), &ctx()).unwrap_err().contains("dark, nord"));
        assert!(registry_commands("set_layout", &json!({ "preset": "spiral" }), &ctx()).is_err());
        assert_eq!(registry_commands("load_recipe", &json!({ "recipe": "full" }), &ctx()).unwrap(), [Command::RecipeLoad("r1".into())]);
        assert!(registry_commands("no_such_tool", &json!({}), &ctx()).is_err());
    }

    #[test]
    fn destructive_tools_ask_first() {
        assert!(needs_confirmation("close_pane", &json!({})));
        assert!(!needs_confirmation("close_pane", &json!({ "confirmed": true })));
        assert!(!needs_confirmation("zoom_pane", &json!({})));
        let tools = registry_tools(&[]);
        let close = tools.iter().find(|t| t["name"] == "close_pane").unwrap();
        assert!(close["description"].as_str().unwrap().ends_with("Ask the user to confirm first."));
        assert!(close["parameters"]["properties"]["confirmed"].is_object());
    }

    #[test]
    fn every_registry_tool_has_commands() {
        let c = ctx();
        for t in registry_tools(&c.providers) {
            let name = t["name"].as_str().unwrap();
            let args = match name {
                "set_layout" => json!({ "preset": "grid" }),
                "switch_workspace" => json!({ "workspace": "1" }),
                "change_model" => json!({ "model": "opus" }),
                "broadcast" => json!({ "text": "hi" }),
                "show_side_panel" => json!({ "panel": "browser" }),
                "set_theme" => json!({ "theme": "dark" }),
                "load_recipe" => json!({ "recipe": "r1" }),
                _ => json!({}),
            };
            let commands = registry_commands(name, &args, &c).unwrap_or_else(|e| panic!("{name}: {e}"));
            assert!(!commands.is_empty(), "{name}");
        }
        assert!(is_registry_tool("zoom_pane") && !is_registry_tool("list_panes"));
    }

    #[test]
    fn schemas_are_cleaned_for_the_model() {
        let dirty = json!({
            "$schema": "x", "type": ["string", "null"], "additionalProperties": false,
            "properties": { "a": { "type": "object", "properties": { "b": { "type": ["null", "integer"], "enum": (0..50).collect::<Vec<_>>() } }, "required": ["b", "zzz"] } },
        });
        let s = sanitize_schema(&dirty);
        assert_eq!(s["type"], "string");
        assert!(s.get("$schema").is_none() && s.get("additionalProperties").is_none());
        let b = &sanitize_schema(&json!({ "type": "object", "properties": { "a": dirty["properties"]["a"].clone() } }))["properties"]["a"];
        assert_eq!(b["required"], json!(["b"]));
        assert_eq!(b["properties"]["b"]["type"], "integer");
        assert!(b["properties"]["b"].get("enum").is_none());
        assert!(b["properties"]["b"]["description"].as_str().unwrap().contains("One of:"));
        let long = clean(&json!({ "name": "t", "description": "x".repeat(500), "parameters": { "type": "object" } }));
        assert_eq!(long["description"].as_str().unwrap().len(), 300);
    }

    #[test]
    fn the_state_summary_lists_panes() {
        let s = state_summary(&Summary {
            workspace: "api".into(), root: "/x".into(),
            panes: vec![(1, "Claude Code".into(), "p1".into(), "working".into(), "branch fix, focused".into())],
            other_workspaces: vec![], agents: vec!["Claude Code (claude)".into()], browser_tabs: vec![],
        });
        assert!(s.contains("1. Claude Code (id p1), working, branch fix, focused"));
        assert!(s.contains("Other workspaces: none") && s.contains("Browser tabs: none"));
    }
}
