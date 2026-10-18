//! Design snapshots: renders sample screens into a ratatui test buffer and writes them as HTML,
//! so the TUI's look can be reviewed in a browser.
//! `WF_SNAP_DIR=/tmp/wf-tui cargo test --lib snapshots -- --ignored`

use std::sync::mpsc::channel;
use std::time::{Duration, Instant};

use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::style::{Color, Modifier};

use super::app::{App, Launch};
use super::picker::{PickItem, PickKind, Picker};
use super::transcript::{Block, Item, NoticeKind, Status};
use super::{anim::Tween, view};
use crate::harness::backend::Kind;
use crate::harness::event::{Approval, Mode, Todo, UserMsg};

const W: u16 = 110;

thread_local! {
    /// Width of the scene being rendered (most use `W`).
    static SNAP_W: std::cell::Cell<u16> = const { std::cell::Cell::new(W) };
}
const H: u16 = 40;

fn app() -> App {
    let (tx, _rx) = channel();
    let launch = Launch { model: "claude/opus".into(), resume: None, cont: false, prompt: None, mode: Some(Mode::Ask), system: None };
    let mut a = App::blank(tx, &launch);
    a.kind = Kind::Claude;
    a.actual_model = "claude-opus-4-5".into();
    a.cwd = "/Users/dev/Coding/winfer".into();
    a.branch = Some("main".into());
    a.alive = true;
    a.config.effort = Some("high".into());
    a.usage.window = 200_000;
    a.usage.context = 46_000;
    a.usage.cost = Some(0.42);
    a.ctx_shown = Tween::new(0.23);
    a
}

fn ago(s: u64) -> Instant {
    Instant::now() - Duration::from_secs(s)
}

const DIFF: &str = "--- a/src/lib/format.ts\n+++ b/src/lib/format.ts\n@@ -10,7 +10,9 @@\n export function formatBytes(n: number) {\n-  if (n < 1024) return n + \" B\";\n+  if (n < 1024) return `${n} B`;\n+  const units = [\"KB\", \"MB\", \"GB\"];\n+  let i = -1;\n   do {\n     n /= 1024;\n";

const ANSWER: &str = "Fixed. `formatBytes` now uses template strings and a unit table, so large values no longer print as `NaN GB`.\n\n## What changed\n\n- **src/lib/format.ts** — rewrote the loop, added `units`\n- **format.test.ts** — 3 new cases for TB values\n\n```ts\nformatBytes(1536) // \"1.5 KB\"\n```\n\n| Test | Result |\n|---|---|\n| unit | 42 passed |\n| e2e | 8 passed |\n";

fn conversation(a: &mut App) {
    let items = vec![
        Block::User { text: "formatBytes prints NaN for files over 1 TB, can you fix it and add tests?".into() },
        Block::Reasoning { id: "r".into(), text: "Look at format.ts first.".into(), started: ago(20), ended: Some(ago(16)) },
        Block::Tool { id: "1".into(), name: "Read".into(), title: "src/lib/format.ts".into(), output: "export function formatBytes(n: number) {\n  if (n < 1024) return n + \" B\";\n  do {\n    n /= 1024;\n  } while (n > 1024);\n}\n".into(), diff: None, status: Status::Ok, started: ago(15), ended: Some(ago(15)) },
        Block::Tool { id: "2".into(), name: "Edit".into(), title: "src/lib/format.ts".into(), output: String::new(), diff: Some(DIFF.into()), status: Status::Ok, started: ago(12), ended: Some(ago(11)) },
        Block::Tool { id: "3".into(), name: "Bash".into(), title: "pnpm vitest run src/lib".into(), output: " ✓ src/lib/format.test.ts (12 tests) 4ms\n ✓ src/lib/path.test.ts (30 tests) 9ms\n\n Test Files  2 passed (2)\n      Tests  42 passed (42)\n".into(), diff: None, status: Status::Ok, started: ago(9), ended: Some(ago(6)) },
        Block::Tool { id: "6".into(), name: "Skill".into(), title: "graphify".into(), output: "Launching skill: graphify".into(), diff: None, status: Status::Ok, started: ago(5), ended: Some(ago(5)) },
        Block::Tool { id: "7".into(), name: "winfer-browser · browser_navigate".into(), title: "http://localhost:5173/settings".into(), output: "### Ran Playwright code\n```js\nawait page.goto('http://localhost:5173/settings');\n```\n### Page\n- Page URL: http://localhost:5173/settings\n- Page Title: Settings".into(), diff: None, status: Status::Ok, started: ago(5), ended: Some(ago(4)) },
        Block::Tool { id: "4".into(), name: "Grep".into(), title: "\"formatBytes\" in src".into(), output: "No matches in vendor/".into(), diff: None, status: Status::Failed, started: ago(5), ended: Some(ago(5)) },
        Block::Assistant { id: "a".into(), text: ANSWER.into() },
        Block::Notice { text: "Compacted: 46k → 12k tokens".into(), kind: NoticeKind::Success },
    ];
    a.items = items.into_iter().map(Item::new).collect();
}

fn background_shells(a: &mut App) {
    use crate::harness::event::TaskEv;
    a.items.push(Item::new(Block::User { text: "Watch CI and check the build in the background.".into() }));
    for (id, command, output, status, seconds, code) in [
        ("build", "cargo build --release", "Finished release profile in 28.04s\nBuild ready.", "completed", 28, Some(0)),
        ("tests", "pnpm test", "FAIL src/auth.test.ts\nExpected status 200, received 401", "failed", 12, Some(1)),
        ("watch", "gh run watch 39063165808", "Windows tests · running\nWaiting for test-harness (windows-latest)…", "running", 134, None),
    ] {
        a.items.push(Item::new(Block::Tool {
            id: id.into(), name: "Bash".into(), title: command.into(), output: output.into(), diff: None,
            status: Status::Ok, started: ago(seconds), ended: Some(ago(0)),
        }));
        a.on_agent_task(TaskEv {
            id: format!("bg-{id}"), tool_id: Some(id.into()), title: Some(command.into()),
            status: Some(status.into()), exit_code: code, ..Default::default()
        });
    }
    a.items.push(Item::new(Block::Assistant {
        id: "reply".into(), text: "Build passed. One auth test failed. Windows CI is still running in the background.".into(),
    }));
    a.composer.set("");
}

fn busy(a: &mut App) {
    conversation(a);
    a.items.truncate(3);
    a.items.push(Item::new(Block::Tool { id: "5".into(), name: "Bash".into(), title: "cargo build --release".into(), output: "   Compiling ratatui v0.30.2\n   Compiling winfer v0.1.73\n".into(), diff: None, status: Status::Running, started: ago(14), ended: None }));
    a.busy = true;
    a.turn_started = ago(74);
    a.verb = "Orbiting";
    a.usage.output = 25_600;
    a.todos = vec![
        Todo { content: "Read format.ts and its tests".into(), status: "completed".into() },
        Todo { content: "Rewrite the unit loop".into(), status: "completed".into() },
        Todo { content: "Build and run the test suite".into(), status: "in_progress".into() },
        Todo { content: "Add TB cases to format.test.ts".into(), status: "pending".into() },
    ];
    a.queue = vec![UserMsg { text: "also update the changelog".into(), images: vec![], skill: None }];
    // Two subagents: one backgrounded (its call already returned), one still in its call.
    for (id, title) in [("t8", "Audit date formatting"), ("t9", "Find other byte formatters")] {
        a.items.insert(2, Item::new(Block::Tool { id: id.into(), name: "Agent".into(), title: title.into(), output: String::new(), diff: None, status: Status::Running, started: ago(41), ended: None }));
    }
    a.on_agent_task(crate::harness::event::TaskEv { id: "k1".into(), tool_id: Some("t8".into()), agent: Some("Explore".into()), status: Some("running".into()), tools: Some(7), last: Some("Grep".into()), ..Default::default() });
}

fn approval(a: &mut App) {
    conversation(a);
    a.items.truncate(3);
    a.approvals.push_back(Approval { id: "x".into(), tool: "Edit".into(), title: "src/lib/format.ts".into(), detail: "Replace 2 lines, add 2".into(), diff: Some(DIFF.into()) });
}

fn picker(a: &mut App) {
    conversation(a);
    let items = vec![
        PickItem::new("Opus", "Most capable · 200k", "claude/opus").group("Claude Code").current(true),
        PickItem::new("Sonnet", "Fast, smart · 1M", "claude/sonnet").group("Claude Code"),
        PickItem::new("Haiku", "Fastest · 200k", "claude/haiku").group("Claude Code"),
        PickItem::new("gpt-5.5", "Frontier · 400k", "chatgpt/gpt-5.5").group("ChatGPT"),
        PickItem::new("gpt-5.5-mini", "Cheap · 400k", "chatgpt/gpt-5.5-mini").group("ChatGPT"),
        PickItem::new("gpt-5.5-codex", "Agentic coding", "codex/gpt-5.5-codex").group("Codex"),
    ];
    a.picker = Some(Picker::new(PickKind::Model, "Switch model", items));
    a.overlay = Tween::new(1.0);
}

fn slash(a: &mut App) {
    conversation(a);
    a.items.truncate(2);
    a.composer.set("/c");
    let items = super::app::COMMANDS.iter().filter(|c| c.0.starts_with("/c")).map(|(c, d)| PickItem::new(*c, *d, *c)).collect();
    a.completion = Some(super::app::Completion { items, selected: 1, start: 0, slash: true });
}

fn typed(a: &mut App) {
    a.composer.set("Refactor @src/lib/format.ts to use Intl.NumberFormat\nand keep the old signature");
}

fn hex(c: Color, fallback: &str) -> String {
    match c {
        Color::Rgb(r, g, b) => format!("#{r:02x}{g:02x}{b:02x}"),
        Color::Reset => fallback.into(),
        Color::Black => "#000000".into(),
        Color::White => "#ffffff".into(),
        Color::Gray => "#a0a0a0".into(),
        Color::DarkGray => "#606060".into(),
        Color::Red | Color::LightRed => "#ff7b72".into(),
        Color::Green | Color::LightGreen => "#7ee787".into(),
        Color::Yellow | Color::LightYellow => "#f5b544".into(),
        Color::Blue | Color::LightBlue => "#79c0ff".into(),
        Color::Magenta | Color::LightMagenta => "#d2a8ff".into(),
        Color::Cyan | Color::LightCyan => "#79c0ff".into(),
        Color::Indexed(_) => fallback.into(),
    }
}

const TERM_BG: &str = "#0e0e13";
const TERM_FG: &str = "#e6e8ee";

/// Draws the frames onto each canvas and plays them.
const JS: &str = r#"
const CW=8.2,CH=17,PAD=14,FONT="13.5px 'JetBrains Mono','SF Mono',Menlo,monospace";
document.querySelectorAll('canvas').forEach((cv,ci)=>{
  const frames=F[ci], rows=frames[0].length, cols=+(cv.dataset.cols||110), dpr=devicePixelRatio||1;
  cv.width=(cols*CW+PAD*2)*dpr; cv.height=(rows*CH+PAD*2)*dpr;
  cv.style.width=(cols*CW+PAD*2)+'px'; cv.style.height=(rows*CH+PAD*2)+'px';
  const g=cv.getContext('2d'); g.scale(dpr,dpr); g.textBaseline='middle';
  const draw=f=>{
    g.clearRect(0,0,cv.width,cv.height);
    f.forEach((row,y)=>row.forEach(([x,t,fg,bg,m,ul])=>{
      const px=PAD+x*CW, py=PAD+y*CH, chars=[...t];
      if(bg){g.fillStyle=bg;g.fillRect(px,py,chars.length*CW+.5,CH+.5);}
      if(ul){g.fillStyle=ul;g.fillRect(px,py+CH-3,chars.length*CW,1.2);}
      g.font=(m&2?'italic ':'')+(m&1?'700 ':'')+FONT; g.fillStyle=fg;
      chars.forEach((c,i)=>{const cx=px+i*CW;
        if(c==='█')g.fillRect(cx,py,CW+.5,CH+.5);
        else if(c==='▀')g.fillRect(cx,py,CW+.5,CH/2);
        else if(c==='▄')g.fillRect(cx,py+CH/2,CW+.5,CH/2);
        else if(c==='▌')g.fillRect(cx,py,CW/2,CH+.5);
        else if(c==='─'||c==='━'){g.fillRect(cx,py+CH/2-(c==='━'?1:.5),CW+.5,c==='━'?2:1);}
        else if(c==='│'){g.fillRect(cx+CW/2-.5,py,1,CH+.5);}
        else if(c!==' ')g.fillText(c,cx,py+CH/2+1);
      });
    }));
  };
  let i=0; draw(frames[0]);
  if(frames.length>1){const once=cv.dataset.once;const id=setInterval(()=>{i++;if(i>=frames.length){if(once){i=0;clearInterval(id);setTimeout(()=>location.hash||0,0);setInterval(()=>{},1e9);draw(frames[frames.length-1]);return;}i=0;}draw(frames[i]);},+cv.dataset.ms);}
});
"#;

/// One frame as JSON rows of runs: [col, text, fg, bg, bold, italic].
fn frame_json(buf: &Buffer, cursor: Option<(u16, u16)>) -> String {
    let (term_bg, term_fg) = term_colors();
    let mut rows = Vec::new();
    for y in 0..buf.area.height {
        let mut runs: Vec<(u16, String, String, String, bool, bool, String)> = Vec::new();
        let mut x = 0;
        while x < buf.area.width {
            let cell = &buf[(x, y)];
            let mut fg = hex(cell.fg, &term_fg);
            let mut bg = hex(cell.bg, "");
            if cell.modifier.contains(Modifier::REVERSED) || cursor == Some((x, y)) {
                std::mem::swap(&mut fg, &mut bg);
                if fg.is_empty() {
                    fg = term_bg.clone();
                }
                if bg.is_empty() {
                    bg = term_fg.clone();
                }
            }
            let bold = cell.modifier.contains(Modifier::BOLD);
            let italic = cell.modifier.contains(Modifier::ITALIC);
            let ul = if cell.modifier.contains(Modifier::UNDERLINED) { hex(cell.underline_color, &fg) } else { String::new() };
            let sym = if cell.symbol().is_empty() { " " } else { cell.symbol() };
            let w = unicode_width::UnicodeWidthStr::width(sym).max(1) as u16;
            match runs.last_mut() {
                Some(r) if r.2 == fg && r.3 == bg && r.4 == bold && r.5 == italic && r.6 == ul && w == 1 && r.0 + r.1.chars().count() as u16 == x => r.1.push_str(sym),
                _ => runs.push((x, sym.to_string(), fg, bg, bold, italic, ul)),
            }
            x += w;
        }
        let cells: Vec<String> = runs
            .into_iter()
            .filter(|r| !(r.1.trim().is_empty() && r.3.is_empty()))
            .map(|r| format!("[{},{},\"{}\",\"{}\",{},\"{}\"]", r.0, serde_json::to_string(&r.1).unwrap(), r.2, r.3, u8::from(r.4) + 2 * u8::from(r.5), r.6))
            .collect();
        rows.push(format!("[{}]", cells.join(",")));
    }
    format!("[{}]", rows.join(","))
}

/// The terminal's default colors under the current palette, as CSS hex.
fn term_colors() -> (String, String) {
    let p = super::theme::pal();
    (hex(p.void, TERM_BG), hex(p.fg, TERM_FG))
}

/// Renders `frames` frames of a scene, `step_ms` apart, as a flipbook.
fn render(name: &str, setup: fn(&mut App), frames: usize, step_ms: u64) -> String {
    let mut a = app();
    setup(&mut a);
    let height = if name == "background shells" { 24 } else { H };
    let base = Instant::now();
    let mut out = format!("<section><h2>{name}</h2><canvas data-ms={step_ms} data-cols={} style=\"background:{}\"></canvas><script>F.push([", SNAP_W.with(|w| w.get()), term_colors().0);
    for i in 0..frames {
        // Shift every clock back so frame i is drawn i*step later (fade-ins settle by frame ~6).
        let shift = Duration::from_millis(1200 + i as u64 * step_ms);
        a.epoch = base - shift;
        for item in &mut a.items {
            item.born = base - shift;
        }
        let mut term = Terminal::new(TestBackend::new(SNAP_W.with(|w| w.get()), height)).unwrap();
        term.draw(|f| view::draw(f, &mut a)).unwrap();
        let cursor = term.get_cursor_position().ok().filter(|_| a.picker.is_none() && a.approvals.is_empty()).map(|p| (p.x, p.y));
        out.push_str(&frame_json(term.backend().buffer(), cursor));
        out.push(',');
    }
    out.push_str("]);</script></section>");
    out
}

/// The welcome screen as it fades in, from launch.
fn intro() -> String {
    let mut a = app();
    let base = Instant::now();
    let mut out = String::from("<section><h2>launch (fade-in)</h2><canvas data-ms=60 data-once=1></canvas><script>F.push([");
    for i in 0..30u64 {
        a.epoch = base - Duration::from_millis(i * 40);
        let mut term = Terminal::new(TestBackend::new(W, H)).unwrap();
        term.draw(|f| view::draw(f, &mut a)).unwrap();
        out.push_str(&frame_json(term.backend().buffer(), None));
        out.push(',');
    }
    out.push_str("]);</script></section>");
    out
}

#[test]
#[ignore]
fn snapshots() {
    let dir = std::env::var("WF_SNAP_DIR").unwrap_or_else(|_| "/tmp/wf-tui".into());
    std::fs::create_dir_all(&dir).unwrap();
    let scenes: [(&str, fn(&mut App), usize); 11] = [
        ("background shells", background_shells, 60),
        ("welcome", |_| {}, 60),
        ("conversation", conversation, 1),
        ("edit card opened (clicked)", |a| {
            conversation(a);
            a.items.truncate(5);
            a.items[3].open = true;
        }, 1),
        ("working", busy, 60),
        ("approval", approval, 24),
        ("model picker", picker, 60),
        ("slash commands", slash, 1),
        ("plan opened (clicked)", |a| {
            busy(a);
            a.plan_open = true;
        }, 30),
        ("plan sidebar (wide pane)", |a| {
            busy(a);
            a.todos.push(Todo { content: "Update the changelog and the README section on sizes".into(), status: "pending".into() });
        }, 30),
        ("typing", |a| {
            conversation(a);
            typed(a)
        }, 1),
    ];
    let only = std::env::var("WF_SNAP_ONLY").ok();
    let mut body = String::new();
    if only.as_deref().is_none_or(|o| o == "launch") {
        body.push_str(&intro());
    }
    for (n, f, frames) in scenes {
        if only.as_deref().is_some_and(|o| o != n) {
            continue;
        }
        SNAP_W.with(|w| w.set(if n.contains("sidebar") { 150 } else { W }));
        body.push_str(&render(n, f, frames, 60));
        SNAP_W.with(|w| w.set(W));
    }
    let page = format!(
        "<!doctype html><meta charset=utf-8><title>winfer code TUI</title><style>\
body{{background:#050507;color:#ccc;font-family:-apple-system,sans-serif;margin:24px}}\
h2{{font-weight:500;font-size:12px;letter-spacing:.12em;text-transform:uppercase;color:#777;margin:28px 0 8px}}\
canvas{{background:{TERM_BG};border-radius:10px;box-shadow:0 10px 40px #000a;display:block}}</style>\
<script>const F=[];</script>{body}<script>{JS}</script>"
    );
    std::fs::write(format!("{dir}/index.html"), page).unwrap();
}

/// The same screens under several of winfer's terminal themes (dark, grey, light).
#[test]
#[ignore]
fn theme_snapshots() {
    use super::theme::{Palette, set_local};
    let dir = std::env::var("WF_SNAP_DIR").unwrap_or_else(|_| "/tmp/wf-tui/themes".into());
    std::fs::create_dir_all(&dir).unwrap();
    let themes: [(&str, (u8, u8, u8), (u8, u8, u8)); 5] = [
        ("winfer dark", (0x12, 0x12, 0x15), (0xe6, 0xe6, 0xea)),
        ("one dark (grey)", (0x28, 0x2c, 0x34), (0xab, 0xb2, 0xbf)),
        ("gruvbox dark", (0x28, 0x28, 0x28), (0xeb, 0xdb, 0xb2)),
        ("winfer light", (0xfb, 0xfb, 0xfc), (0x1b, 0x1b, 0x21)),
        ("solarized light", (0xfd, 0xf6, 0xe3), (0x58, 0x6e, 0x75)),
    ];
    let mut body = String::new();
    for (name, bg, fg) in themes {
        set_local(Some(Palette::derive(bg, Some(fg))));
        body.push_str(&format!("<h1>{name}</h1>"));
        body.push_str(&render(&format!("{name} · working"), busy, 30, 60));
        body.push_str(&render(&format!("{name} · conversation"), |a| {
            conversation(a);
            typed(a)
        }, 1, 60));
        body.push_str(&render(&format!("{name} · approval"), approval, 1, 60));
        body.push_str(&render(&format!("{name} · picker"), picker, 30, 60));
        body.push_str(&render(&format!("{name} · welcome"), |_| {}, 30, 60));
    }
    set_local(None);
    let page = format!(
        "<!doctype html><meta charset=utf-8><title>winfer code TUI themes</title><style>\
body{{background:#3a3a40;color:#ddd;font-family:-apple-system,sans-serif;margin:24px}}\
h1{{font-size:18px;margin:40px 0 0}}h2{{font-weight:500;font-size:12px;letter-spacing:.12em;text-transform:uppercase;color:#bbb;margin:20px 0 8px}}\
canvas{{border-radius:10px;box-shadow:0 10px 40px #0006;display:block}}</style>\
<script>const F=[];</script>{body}<script>{JS}</script>"
    );
    std::fs::write(format!("{dir}/index.html"), page).unwrap();
}

/// Frame cost with a long transcript: `cargo test --release --lib frame_cost -- --ignored --nocapture`.
#[test]
#[ignore]
fn frame_cost() {
    let mut a = app();
    for _ in 0..150 {
        conversation(&mut a);
        let more: Vec<Item> = std::mem::take(&mut a.items);
        a.tasks.clear();
        a.todos.clear();
        a.items = more;
    }
    let mut all = Vec::new();
    for _ in 0..150 {
        let mut b = app();
        conversation(&mut b);
        all.extend(b.items);
    }
    for item in &mut all {
        item.born -= Duration::from_secs(5);
    }
    a.items = all;
    a.busy = true;
    let mut term = Terminal::new(TestBackend::new(160, 50)).unwrap();
    term.draw(|f| view::draw(f, &mut a)).unwrap();
    let n = 60;
    let t0 = Instant::now();
    for _ in 0..n {
        term.draw(|f| view::draw(f, &mut a)).unwrap();
    }
    let per = t0.elapsed() / n;
    println!("items: {} · per frame: {per:?}", a.items.len());
}

/// How many distinct colors 10 s of animation draws (each is a glyph-cache entry in xterm.js).
#[test]
fn animation_uses_few_colors() {
    let mut colors = std::collections::HashSet::new();
    for scene in [busy as fn(&mut App), |_: &mut App| {}, picker] {
        let mut a = app();
        scene(&mut a);
        let base = Instant::now();
        for i in 0..std::env::var("WF_FRAMES").ok().and_then(|v| v.parse().ok()).unwrap_or(250u64) {
            a.epoch = base - Duration::from_millis(1000 + i * 40);
            let mut term = Terminal::new(TestBackend::new(W, H)).unwrap();
            term.draw(|f| view::draw(f, &mut a)).unwrap();
            for c in term.backend().buffer().content() {
                colors.insert((c.fg, c.bg));
            }
        }
    }
    println!("{} color pairs", colors.len());
    assert!(colors.len() < 1500, "{} color pairs", colors.len());
}
