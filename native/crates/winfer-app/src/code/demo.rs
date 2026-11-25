//! Debug aid: `WINFER_NATIVE_CODE_DEMO=1` plays a scripted session into the chat pane
//! (through the same path as the real process), so the styling can be checked without a login.

use serde_json::{Value, json};

/// (delay in ms before it, a typed message or a protocol event).
pub enum Step {
    /// Waits until this file exists (and removes it): lets a test decide when the next step plays.
    Gate(&'static str),
    Type(&'static str),
    OpenTaskLog(&'static str),
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
        "agents" => {
            // The live subagent tree: three subagents, one finishing, one failing, one still going.
            steps.truncate(2);
            steps.push((300, Type("Find why the login test is flaky\r")));
            steps.push((300, Ev(json!({ "type": "turn_start" }))));
            steps.push((300, Ev(json!({ "type": "tool_start", "id": "ag1", "name": "Agent", "title": "Map the auth flow", "input": { "subagent_type": "Explore" } }))));
            steps.push((100, Ev(json!({ "type": "task", "id": "s1", "tool_id": "ag1", "agent": "Explore", "description": "Map the auth flow", "model": "claude/haiku", "status": "running", "tools": 3, "last": "Grep \"session_token\" src/" }))));
            steps.push((300, Ev(json!({ "type": "task", "id": "s2", "agent": "general", "description": "Rerun the login test 20 times", "model": "claude/sonnet", "status": "running", "tools": 1, "last": "Bash npm test -- login --repeat 20" }))));
            steps.push((300, Ev(json!({ "type": "task", "id": "s3", "agent": "Explore", "description": "Check CI logs for timeouts", "model": "claude/haiku", "status": "running", "tools": 2, "last": "Read .github/workflows/ci.yml" }))));
            steps.push((900, Ev(json!({ "type": "task", "id": "s1", "tools": 5, "last": "Read src/auth/session.ts" }))));
            steps.push((900, Ev(json!({ "type": "task", "id": "s1", "tools": 7, "last": "Read src/auth/refresh.ts", "status": "done" }))));
            steps.push((0, Ev(json!({ "type": "tool_end", "id": "ag1", "ok": true, "output": "The session token is refreshed in src/auth/refresh.ts on a 30 s timer, and the login test checks it right after sign-in, so a refresh in between makes it flaky." }))));
            steps.push((700, Ev(json!({ "type": "task", "id": "s3", "tools": 4, "last": "No CI run in the last 30 days", "status": "failed" }))));
            steps.push((700, Ev(json!({ "type": "task", "id": "s2", "tools": 2, "last": "7 of 20 runs passed so far" }))));
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
        "viz" => {
            steps.truncate(2);
            steps.push((300, Type("Show me how the build time splits up\r")));
            steps.push((300, Ev(json!({ "type": "turn_start" }))));
            steps.push((400, Ev(json!({ "type": "tool_start", "id": "v0", "name": "Bash", "title": "cargo build --timings", "input": { "command": "cargo build --timings" } }))));
            steps.push((500, Ev(json!({ "type": "tool_end", "id": "v0", "ok": true, "output": "Finished" }))));
            steps.push((300, Ev(json!({ "type": "tool_start", "id": "v1", "name": "winfer · visualize", "title": "Build time report", "input": { "title": "Build time report", "subtitle": "cargo build --timings, release", "blocks": [
                { "type": "stats", "items": [{ "label": "Total", "value": "41.2 s", "delta": "-8% vs last week" }, { "label": "Crates", "value": "318" }, { "label": "Cache hits", "value": "92%", "note": "sccache" }] },
                { "type": "pie", "ring": true, "title": "Where the time goes", "center": { "value": "41 s", "label": "total" }, "unit": "s", "slices": [{ "label": "gpui", "value": 14.2 }, { "label": "winfer-app", "value": 11.1 }, { "label": "winfer-backend", "value": 8.4 }, { "label": "tokio", "value": 3.2 }, { "label": "serde", "value": 2.1 }, { "label": "other", "value": 2.2 }] },
                { "type": "bar", "title": "Seconds per crate", "unit": "s", "labels": ["gpui", "winfer-app", "winfer-backend", "tokio"], "series": [{ "name": "Debug", "values": [9.1, 7.4, 5.2, 2.0] }, { "name": "Release", "values": [14.2, 11.1, 8.4, 3.2] }] },
                { "type": "line", "area": true, "title": "Build time, last 8 runs", "unit": "s", "labels": ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun", "Mon"], "series": [{ "name": "Release", "values": [52, 49, 47, 51, 44, 43, 42, 41.2] }] },
                { "type": "bar", "orientation": "vertical", "stacked": true, "title": "Stacked by phase", "labels": ["Mon", "Tue", "Wed", "Thu"], "series": [{ "name": "Compile", "values": [30, 28, 27, 25] }, { "name": "Link", "values": [12, 11, 12, 10] }, { "name": "Other", "values": [4, 3, 4, 3] }] },
                { "type": "progress", "title": "Budget", "items": [{ "label": "CI minutes", "value": 72 }, { "label": "Disk", "value": 18, "max": 50 }] },
                { "type": "table", "title": "Slowest crates", "columns": ["Crate", { "key": "t", "label": "Time" }, "Note"], "rows": [["gpui", 14.2, "proc macros"], ["winfer-app", 11.1, "large file"], { "Crate": "winfer-backend", "t": 8.4, "Note": "" }] },
                { "type": "text", "markdown": "**gpui** dominates; a shared target dir would cut about 10 s." }
            ] } }))));
            steps.push((300, Ev(json!({ "type": "tool_end", "id": "v1", "ok": true, "output": "Shown to the user in the chat: 1 stat row." }))));
            steps.push((300, Ev(json!({ "type": "text", "id": "va1", "text": "Two crates make up 60% of the build." }))));
            steps.push((200, Ev(json!({ "type": "turn_done" }))));
        }
        "table" => {
            steps.truncate(2);
            steps.push((300, Type("Compare the build profiles\r")));
            steps.push((300, Ev(json!({ "type": "turn_start" }))));
            steps.push((400, Ev(json!({ "type": "text", "id": "ta1", "text": "Here is how the profiles compare:\n\n| Profile | Time | Size | Notes |\n|:--------|-----:|-----:|:-----:|\n| debug | 9.1 s | 212 MB | fast incremental |\n| release | 41.2 s | 38 MB | `lto = thin` |\n| bench | 44.8 s | 41 MB | **slowest** |\n\nUse `release` for shipping and `debug` while iterating." }))));
            steps.push((200, Ev(json!({ "type": "turn_done" }))));
        }
        "bgnotes" => {
            steps.truncate(2);
            steps.push((300, Type("Test the signed saves\r")));
            steps.push((200, Ev(json!({ "type": "turn_start" }))));
            steps.push((300, Ev(json!({ "type": "text", "id": "bn1", "text": "I'll run the signed save test, then start the listener." }))));
            steps.push((200, Ev(json!({ "type": "tool_start", "id": "bn-t1", "name": "Read", "title": "src/auth.rs", "input": { "file_path": "src/auth.rs" } }))));
            steps.push((200, Ev(json!({ "type": "tool_end", "id": "bn-t1", "ok": true, "output": "ok" }))));
            steps.push((200, Ev(json!({ "type": "notice", "text": "Background task bg1 \"Test signed development mapping saves\" (`cargo test`) exited with code 0 after 24s. No new output." }))));
            steps.push((200, Ev(json!({ "type": "notice", "text": "Background task bg2 \"Start paired LAN listener\" (`./listen.sh`) exited with code 1 after 3s. Its output since your last read:\nerror: address in use" }))));
            steps.push((200, Ev(json!({ "type": "notice", "text": "Background task bg3 \"Build signed-save test APK\" (`./gradlew assemble`) exited with code 0 after 184s. No new output." }))));
            steps.push((200, Ev(json!({ "type": "turn_done" }))));
        }
        "handoff" => {
            steps.truncate(2);
            steps.push((200, Ev(json!({ "type": "ready", "session": "demo", "model": "claude/fable", "mode": "ask", "cwd": "/tmp/web-os-demo", "branch": "main", "effort": "high" }))));
            steps.push((300, Type("Sketch the auth middleware\r")));
            steps.push((200, Ev(json!({ "type": "turn_start" }))));
            steps.push((300, Ev(json!({ "type": "model", "model": "claude-fable-5-2" }))));
            steps.push((400, Ev(json!({ "type": "text", "id": "ha1", "text": "I've mapped out the JWT verification middleware and added session cookie parsing in `src/auth.rs`." }))));
            steps.push((200, Ev(json!({ "type": "turn_done" }))));
            steps.push((500, Ev(json!({ "type": "handoff", "from_model": "claude/fable", "from_actual": "claude-fable-5-2", "to_model": "agy/gemini-4-neon", "replace": false }))));
            steps.push((50, Ev(json!({ "type": "model", "model": "agy/gemini-4-neon" }))));
            steps.push((400, Type("Now add unit tests for expired tokens\r")));
            steps.push((200, Ev(json!({ "type": "turn_start" }))));
            steps.push((400, Ev(json!({ "type": "text", "id": "ha2", "text": "Continuing from the middleware in `src/auth.rs`, I'll add unit tests covering expired JWTs and missing cookies." }))));
            steps.push((200, Ev(json!({ "type": "turn_done" }))));
        }
        "tasklog" => {
            steps.truncate(2);
            steps.push((300, Type("Run the background build & test watcher\r")));
            steps.push((200, Ev(json!({ "type": "turn_start" }))));
            steps.push((200, Ev(json!({ "type": "task", "id": "bg-demo", "title": "Live build & test watcher", "description": "cargo watch -x test", "status": "running", "output_file": "/tmp/web-os-demo/watch.log", "last": "Watching native/crates/winfer-app…" }))));
            steps.push((400, OpenTaskLog("bg-demo")));
            let mut lines: Vec<String> = (1..=60).map(|i| format!("[16:10:{:02}] step {i:03}: compiling & testing module_{i:02} ... ok", i % 60)).collect();
            steps.push((150, Ev(json!({ "type": "task_log", "id": "bg-demo", "file": "/tmp/web-os-demo/watch.log", "text": lines.join("\n") }))));
            for i in 61..=180 {
                lines.push(format!("[16:{:02}:{:02}] live log {i:03}: streaming background shell output ... ok", 10 + (i / 60), i % 60));
                steps.push((900, Ev(json!({ "type": "task_log", "id": "bg-demo", "file": "/tmp/web-os-demo/watch.log", "text": lines.join("\n") }))));
            }
        }
        "model" => steps.push((1200, Type("/model\r"))),
        "mode" => steps.push((1200, Type("/mode\r"))),
        "effort" => steps.push((1200, Type("/effort\r"))),
        _ => {}
    }
    steps
}
