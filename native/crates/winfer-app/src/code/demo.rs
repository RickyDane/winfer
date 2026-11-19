//! Debug aid: `WINFER_NATIVE_CODE_DEMO=1` plays a scripted session into the chat pane
//! (through the same path as the real process), so the styling can be checked without a login.

use serde_json::{Value, json};

/// (delay in ms before it, a typed message or a protocol event).
pub enum Step {
    /// Waits until this file exists (and removes it): lets a test decide when the next step plays.
    Gate(&'static str),
    Type(&'static str),
    Ev(Value),
}

/// `mode` is the value of the variable: `welcome` stops after the ready event, `model` opens the model picker.
pub fn script(mode: &str) -> Vec<(u64, Step)> {
    use Step::*;
    let _ = std::fs::create_dir_all("/tmp/web-os-demo");
    for f in ["index.html", "os.css", "os.js"] {
        let _ = std::fs::write(format!("/tmp/web-os-demo/{f}"), "<!doctype html>\n<title>Web OS</title>\n<div id=\"desktop\"></div>\n");
    }
    let _diff = "--- a/index.html\n+++ b/index.html\n@@ -1,4 +1,6 @@\n <!doctype html>\n-<title>Untitled</title>\n+<title>Web OS</title>\n+<link rel=\"stylesheet\" href=\"os.css\">\n <body>\n+  <div id=\"desktop\"></div>\n </body>";
    let mut steps = vec![
        (300, Ev(json!({ "type": "ready", "session": "demo", "model": "claude/sonnet", "mode": "ask", "cwd": "/tmp/web-os-demo", "branch": "main", "effort": "high" }))),
        (100, Ev(json!({ "type": "models", "models": [
            { "spec": "claude/opus", "label": "Opus 5.5", "detail": "Most capable, for complex work", "efforts": ["low", "medium", "high", "xhigh", "max"] },
            { "spec": "claude/sonnet", "label": "Sonnet 5.5", "detail": "Best everyday balance of speed and skill", "efforts": ["low", "medium", "high", "xhigh", "max"] },
            { "spec": "claude/haiku", "label": "Haiku 5", "detail": "Fastest, for quick answers" },
            { "spec": "codex/gpt-5.5", "label": "GPT-5.5", "detail": "OpenAI's frontier coding model" },
            { "spec": "codex/gpt-5.5-fast", "label": "GPT-5.5 Fast", "detail": "" },
            { "spec": "codex/gpt-5.5-mini", "label": "GPT-5.5 mini", "detail": "Cheap and quick" },
            { "spec": "opencode/anthropic/claude-sonnet", "label": "Claude Sonnet via opencode", "detail": "" }
        ] }))),
        (1500, Type("Create a simple web OS with a desktop, a dock and draggable windows\r")),
        (300, Ev(json!({ "type": "turn_start" }))),
        (700, Ev(json!({ "type": "reasoning_delta", "id": "r1", "text": "The folder is empty, so I'll start from scratch: one HTML file, a stylesheet and a small script." }))),
        (1800, Ev(json!({ "type": "tool_start", "id": "t1", "name": "Bash", "title": "ls -la", "input": { "command": "ls -la", "description": "List the folder" } }))),
        (1400, Ev(json!({ "type": "tool_end", "id": "t1", "ok": true, "output": "total 0\ndrwxr-xr-x  2 dane  staff  64 Nov 19 16:40 .\ndrwxr-xr-x  5 dane  staff 160 Nov 19 16:39 .." }))),
        (500, Ev(json!({ "type": "text_delta", "id": "a1", "text": "The folder is empty. I'll build it in three files:\n\n- `index.html` — the desktop and dock\n- `os.css` — layout and window chrome\n- `os.js` — draggable windows" }))),
        (1200, Ev(json!({ "type": "todos", "todos": [{ "content": "Write the page structure", "status": "in_progress" }, { "content": "Style the desktop, dock and windows", "status": "pending" }, { "content": "Make windows draggable", "status": "pending" }] }))),
        (500, Ev(json!({ "type": "tool_start", "id": "t2", "name": "Write", "title": "index.html", "input": { "file_path": "index.html", "content": "<!doctype html>\n<title>Web OS</title>\n<div id=\"desktop\"></div>" } }))),
        (2200, Ev(json!({ "type": "tool_end", "id": "t2", "ok": true, "output": "File created successfully at: index.html" }))),
        (600, Ev(json!({ "type": "tool_start", "id": "t3", "name": "Edit", "title": "os.css", "input": { "file_path": "os.css", "old_string": "body { margin: 8px; }", "new_string": "body { margin: 0; background: #111; }" } }))),
        (900, Ev(json!({ "type": "tool_end", "id": "t3", "ok": true, "output": "" }))),
        (300, Ev(json!({ "type": "tool_start", "id": "t4", "name": "winfer · bg", "title": "start web-os static server", "input": { "action": "start", "cmd": "python3 -m http.server 8080", "name": "web-os static server" } }))),
        (400, Ev(json!({ "type": "tool_end", "id": "t4", "ok": true, "output": "started bg1 (pid 4242)" }))),
        (900, Ev(json!({ "type": "approval", "id": "ap1", "tool": "Bash", "title": "python3 -m http.server 8000", "detail": "", "questions": [] }))),
        (4500, Ev(json!({ "type": "approval", "id": "ap2", "tool": "", "title": "", "detail": "", "questions": [{ "header": "Style", "question": "Which look should the desktop have?", "multi": false, "options": [{ "label": "Classic", "description": "Platinum-grey, beveled windows" }, { "label": "Modern", "description": "Glass blur, rounded corners" }] }] }))),
        (3500, Ev(json!({ "type": "notice", "text": "Context is 68% full. Run /compact to free space." }))),
        (1200, Ev(json!({ "type": "text", "id": "a2", "text": "Done. The page now has a **desktop**, a **dock** and draggable windows.\n\n```bash\npython3 -m http.server 8000\n```\n\nOpen `http://localhost:8000` to try it." }))),
        (400, Ev(json!({ "type": "turn_done" }))),
    ];
    match mode {
        "phone" => {
            steps.truncate(2);
            steps.push((300, Type("Set up the web OS project\r")));
            steps.push((300, Ev(json!({ "type": "turn_start" }))));
            steps.push((500, Ev(json!({ "type": "tool_start", "id": "p1", "name": "Bash", "title": "ls -la", "input": { "command": "ls -la" } }))));
            steps.push((800, Ev(json!({ "type": "tool_end", "id": "p1", "ok": true, "output": "total 0" }))));
            steps.push((300, Ev(json!({ "type": "text", "id": "pa1", "text": "The folder is empty. Before I start the server I need your go-ahead." }))));
            steps.push((0, Gate("/tmp/winfer-demo-approval")));
            steps.push((200, Ev(json!({ "type": "approval", "id": "pap1", "tool": "Bash", "title": "python3 -m http.server 8000", "detail": "python3 -m http.server 8000", "questions": [] }))));
            steps.push((0, Gate("/tmp/winfer-demo-form")));
            steps.push((200, Ev(json!({ "type": "approval", "id": "pap2", "tool": "", "title": "", "detail": "", "questions": [
                { "header": "Style", "question": "Which look should the desktop have?", "multi": false, "options": [{ "label": "Classic", "description": "Platinum-grey, beveled windows" }, { "label": "Modern", "description": "Glass blur, rounded corners" }] },
                { "header": "Dock", "question": "Where should the dock go?", "multi": false, "options": [{ "label": "Bottom", "description": "" }, { "label": "Left", "description": "" }] }
            ] }))));
        }
        "welcome" => steps.truncate(1),
        "draft" => {
            steps.truncate(2);
            steps.push((600, Type("Add a settings window")));
        }
        "fast" => {
            steps.truncate(2);
            steps.push((300, Ev(json!({ "type": "ready", "session": "demo", "model": "codex/gpt-6-luna", "mode": "ask", "cwd": "/tmp/web-os-demo", "branch": "main", "effort": "high" }))));
        }
        "model" => steps.push((1200, Type("/model\r"))),
        "mode" => steps.push((1200, Type("/mode\r"))),
        "effort" => steps.push((1200, Type("/effort\r"))),
        _ => {}
    }
    steps
}
