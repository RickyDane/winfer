//! What a winfer code chat pane shows a paired phone: its live thread and the exact approval or
//! question it waits on (port of `features/mobile/liveThread.ts`). The chat knows these itself, so
//! nothing is read back off a screen.

use super::names::{effort_levels, match_model, model_name};
use super::state::{CodeState, Item};
use crate::request::{AgentRequest, RequestOption};
use serde_json::{Value, json};

/// The transcript as the phone's thread: your messages, the replies, and a row per tool call.
pub fn live_thread(state: &CodeState, limit: usize) -> Vec<Value> {
    let mut out = Vec::new();
    for item in &state.items {
        match item {
            Item::User { key, text, at, .. } => out.push(json!({ "id": key, "role": "user", "text": text, "ts": (*at > 0).then_some(*at) })),
            Item::Text { key, text, .. } => out.push(json!({ "id": key, "role": "assistant", "text": text, "ts": null })),
            Item::Tool { key, name, title, input, ok, started, ended, .. } => {
                let task = state.tasks.iter().find(|t| t.ev.tool_id.as_deref() == Some(key.as_str()));
                let status = match task.and_then(|t| t.ev.status.as_deref()) {
                    Some("running") => "running",
                    Some("failed") => "failed",
                    _ if ended.is_none() => "running",
                    _ if *ok == Some(false) => "failed",
                    _ => "done",
                };
                let desc = input.get("description").and_then(|v| v.as_str()).map(str::trim).filter(|s| !s.is_empty());
                let mut row = json!({
                    "id": key,
                    "role": "tool",
                    "text": if title.is_empty() { name } else { title },
                    "tool": name,
                    "ts": (*started > 0).then_some(*started),
                    "ended": ended.filter(|e| *e > 0),
                    "state": status,
                    "background": task.is_some(),
                });
                if let Some(d) = desc {
                    row["desc"] = json!(d);
                }
                // A finished `visualize` call is a chart on the phone too.
                if let Some(report) = (status != "failed").then(|| winfer_backend::viz::phone_report(name, input)).flatten() {
                    row["report"] = report;
                }
                out.push(row);
            }
            Item::Notice { key, text, error: false } => {
                if let Some(n) = super::bg_note::parse(text) {
                    let detail = [n.cmd.as_str(), n.output.as_str()].iter().filter(|s| !s.is_empty()).copied().collect::<Vec<_>>().join("\n\n");
                    out.push(json!({ "id": key, "role": "bg", "text": n.title(), "detail": detail, "state": if n.failed() { "failed" } else { "done" }, "ts": null }));
                }
            }
            _ => {}
        }
    }
    if let (true, Some(Item::Reasoning { key, streaming: true, started, .. })) = (state.busy, state.items.last()) {
        out.push(json!({ "id": key, "role": "thinking", "text": "", "ts": started }));
    }
    let skip = out.len().saturating_sub(limit);
    out.split_off(skip)
}

/// The first pending approval or question as the phone shows it. For a form of several questions,
/// `answered` is how many the phone has answered so far.
pub fn request(state: &CodeState, answered: usize) -> Option<AgentRequest> {
    let a = state.approvals.first()?;
    if a.questions.is_empty() {
        let detail = a.detail.trim();
        let head = format!("Allow {}?\n{}", if a.tool.is_empty() { "this" } else { &a.tool }, a.title);
        let text = if detail.is_empty() || detail == a.title { head } else { format!("{head}\n\n{}", detail.chars().take(600).collect::<String>()) };
        return Some(AgentRequest { kind: "approval", text, options: None });
    }
    let total = a.questions.len();
    let q = &a.questions[answered.min(total - 1)];
    let progress = if total > 1 { format!("({}/{})", answered.min(total - 1) + 1, total) } else { String::new() };
    let head = match (q.header.is_empty(), progress.is_empty()) {
        (false, true) => format!("{}\n", q.header),
        (false, false) => format!("{} {progress}\n", q.header),
        (true, false) => format!("{progress}\n"),
        (true, true) => String::new(),
    };
    let multi = if q.multi { "\n(Several apply? Type them, comma-separated.)" } else { "" };
    let mut options: Vec<RequestOption> = q.options.iter().enumerate().map(|(i, o)| RequestOption { n: i + 1, label: o.label.clone(), detail: (!o.description.is_empty()).then(|| o.description.clone()) }).collect();
    options.push(RequestOption { n: options.len() + 1, label: "Type something".into(), detail: None });
    Some(AgentRequest { kind: "question", text: format!("{head}{}{multi}", q.question), options: Some(options) })
}

/// The answer a phone's pick stands for: option `n` of the current question, or typed text on the
/// row after the options. `None` when that choice doesn't exist.
pub fn pick(state: &CodeState, answered: usize, n: usize, typed: &str) -> Option<String> {
    let a = state.approvals.first().filter(|a| !a.questions.is_empty())?;
    let q = &a.questions[answered.min(a.questions.len() - 1)];
    let answer = if (1..=q.options.len()).contains(&n) { q.options[n - 1].label.clone() } else if n == q.options.len() + 1 { typed.trim().to_string() } else { String::new() };
    (!answer.is_empty()).then_some(answer)
}

/// A chat pane's model and effort as the phone shows them.
pub fn chat_config(state: &CodeState) -> Value {
    let offered = match_model(&state.models, &state.model).and_then(|i| i.efforts.clone());
    json!({
        "model": state.model,
        "label": if state.model.is_empty() { String::new() } else { model_name(&state.model, state.actual_model.as_deref()) },
        "effort": if state.effort.is_empty() { "default" } else { state.effort.as_str() },
        "efforts": effort_levels(offered),
        "mode": state.mode.label(),
        "yolo": matches!(state.mode, super::protocol::Mode::Yolo | super::protocol::Mode::YoloPlan),
        "cwd": state.cwd,
        "branch": state.branch,
    })
}

/// The models a chat pane offers, each with the effort levels it takes.
pub fn chat_models(state: &CodeState) -> Vec<Value> {
    state.models.iter().map(|m| json!({
        "id": m.spec,
        "name": if m.label.is_empty() { model_name(&m.spec, None) } else { m.label.clone() },
        "detail": m.detail,
        "efforts": effort_levels(m.efforts.clone()),
    })).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::code::protocol::{Approval, Ev, Question, QuestionOption};
    use serde_json::json;

    fn ev(v: Value) -> Ev {
        serde_json::from_value(v).unwrap()
    }

    fn form() -> Approval {
        let q = |text: &str| Question { header: "Style".into(), question: text.into(), multi: false, options: vec![QuestionOption { label: "Classic".into(), description: "Grey".into() }, QuestionOption { label: "Modern".into(), description: String::new() }] };
        Approval { id: "a1".into(), questions: vec![q("Which look?"), q("Which dock?")], ..Default::default() }
    }

    #[test]
    fn thread_has_messages_tools_and_thinking() {
        let mut s = CodeState::default();
        s.apply(ev(json!({ "type": "turn_start" })));
        s.apply(ev(json!({ "type": "text", "id": "a", "text": "Hello" })));
        s.apply(ev(json!({ "type": "tool_start", "id": "t", "name": "Bash", "title": "ls", "input": { "description": "List files" } })));
        let t = live_thread(&s, 10);
        assert_eq!(t.iter().map(|m| m["role"].as_str().unwrap()).collect::<Vec<_>>(), ["assistant", "tool"]);
        assert_eq!(t[1]["state"], "running");
        assert_eq!(t[1]["desc"], "List files");
        assert!(t[1]["ended"].is_null());
        s.apply(ev(json!({ "type": "tool_end", "id": "t", "ok": false, "output": "" })));
        let done = live_thread(&s, 10);
        assert_eq!(done[1]["state"], "failed");
        assert!(done[1]["ended"].as_u64().is_some());
        assert_eq!(live_thread(&s, 1).len(), 1);
    }

    #[test]
    fn visualize_calls_carry_their_report() {
        let mut s = CodeState::default();
        let input = json!({ "title": "Times", "blocks": [{ "type": "bar", "labels": ["a", "b"], "series": [{ "name": "s", "values": [1, 2] }] }] });
        s.apply(ev(json!({ "type": "tool_start", "id": "v", "name": "winfer · visualize", "title": "Times", "input": input })));
        s.apply(ev(json!({ "type": "tool_end", "id": "v", "ok": true, "output": "" })));
        s.apply(ev(json!({ "type": "tool_start", "id": "b", "name": "winfer · visualize", "title": "Bad", "input": { "blocks": [] } })));
        let t = live_thread(&s, 10);
        assert_eq!(t[0]["report"]["title"], "Times");
        assert_eq!(t[0]["report"]["blocks"][0]["type"], "bar");
        assert!(t[1].get("report").is_none());
    }

    #[test]
    fn bg_notes_become_compact_rows() {
        let mut s = CodeState::default();
        s.apply(ev(json!({ "type": "bg_note", "text": "Background task bg1 \"Dev\" (`npm run dev`) exited with code 1 after 4s. Its output since your last read:\nboom" })));
        s.apply(ev(json!({ "type": "notice", "text": "Resumed" })));
        let t = live_thread(&s, 10);
        assert_eq!(t.len(), 1);
        assert_eq!((t[0]["role"].as_str(), t[0]["text"].as_str(), t[0]["state"].as_str()), (Some("bg"), Some("Dev · exit 1 · 4s"), Some("failed")));
        assert_eq!(t[0]["detail"], "npm run dev\n\nboom");
    }

    #[test]
    fn approvals_become_requests() {
        let mut s = CodeState::default();
        s.approvals.push(Approval { id: "a".into(), tool: "Bash".into(), title: "rm -rf build".into(), detail: "rm -rf build".into(), ..Default::default() });
        let r = request(&s, 0).unwrap();
        assert_eq!(r.kind, "approval");
        assert_eq!(r.text, "Allow Bash?\nrm -rf build");
        assert!(request(&CodeState::default(), 0).is_none());
    }

    #[test]
    fn questions_show_progress_options_and_a_free_text_row() {
        let mut s = CodeState::default();
        s.approvals.push(form());
        let r = request(&s, 1).unwrap();
        assert_eq!(r.kind, "question");
        assert!(r.text.starts_with("Style (2/2)\nWhich dock?"));
        let options = r.options.unwrap();
        assert_eq!(options.iter().map(|o| o.label.as_str()).collect::<Vec<_>>(), ["Classic", "Modern", "Type something"]);
        assert_eq!(pick(&s, 0, 2, ""), Some("Modern".into()));
        assert_eq!(pick(&s, 0, 3, " Pixel art "), Some("Pixel art".into()));
        assert_eq!(pick(&s, 0, 3, ""), None);
        assert_eq!(pick(&s, 0, 9, ""), None);
    }
}
