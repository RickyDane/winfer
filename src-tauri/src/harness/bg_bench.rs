//! Benchmark: the `bg` tool vs. background processes through plain `bash` (`&`, `nohup`, log
//! files, poll loops, `kill`). Each scenario replays the calls an agent makes with each approach
//! through the real tools and measures calls, wall time, bytes the model writes (call arguments)
//! and reads (tool output), whether it got the answer, and processes left running.
//!
//! cargo test --lib bg_bench -- --ignored --nocapture --test-threads=1

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use super::tools::{self, Ctx, ReadCache};

#[derive(Default)]
struct Run {
    calls: usize,
    written: usize,
    read: usize,
    outputs: Vec<String>,
    times: Vec<f32>,
    /// Whether the process still ran when the agent was done with it (before stopping it).
    alive_mid: bool,
}

impl Run {
    fn call(&mut self, dir: &Path, name: &str, args: Value) -> String {
        self.calls += 1;
        self.written += args.to_string().len();
        let mut snapshots = Default::default();
        let reads = ReadCache::default();
        let mut ctx = Ctx { cwd: dir, stop: Arc::new(AtomicBool::new(false)), output: &|_| {}, call_id: "call", bg_owner: "", snapshots: &mut snapshots, reads: &reads };
        let t = Instant::now();
        let out = tools::run(name, &args, &mut ctx).output;
        self.times.push(t.elapsed().as_secs_f32());
        self.read += out.len();
        self.outputs.push(out.clone());
        out
    }
    fn bash(&mut self, dir: &Path, cmd: &str) -> String {
        self.call(dir, "bash", json!({ "cmd": cmd }))
    }
    fn bash_t(&mut self, dir: &Path, cmd: &str, timeout: u64) -> String {
        self.call(dir, "bash", json!({ "cmd": cmd, "timeout": timeout }))
    }
    fn bg(&mut self, dir: &Path, args: Value) -> String {
        self.call(dir, "bg", args)
    }
}

/// Processes whose command line has `tag` (each scenario's scripts carry a unique one).
fn alive(tag: &str) -> usize {
    let out = std::process::Command::new("pgrep").args(["-f", tag]).output().unwrap();
    String::from_utf8_lossy(&out.stdout).lines().count()
}

fn kill_left(tag: &str) {
    let _ = std::process::Command::new("pkill").args(["-9", "-f", tag]).status();
}

fn bg_id(out: &str) -> String {
    out.split_whitespace().nth(1).unwrap_or("").to_string()
}

struct Row {
    scenario: &'static str,
    approach: &'static str,
    run: Run,
    secs: f32,
    ok: bool,
    left: usize,
}

fn dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("winfer-bg-bench-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// A dev server: compiles 2 s, prints "ready", then logs a request every 50 ms. It runs as a
/// parent with a child (like npm → node), so killing only the parent leaves the child.
fn server(d: &Path, tag: &str) -> String {
    let script = d.join(format!("{tag}.sh"));
    std::fs::write(&script, format!("echo compiling; sleep 2; echo 'ready on http://localhost:5173'; sh -c 'while :; do echo \"GET / 200\"; sleep 0.05; done' {tag}-child & wait\n")).unwrap();
    format!("sh {}", script.display())
}

/// A test watcher: a tick every 100 ms, a failure after ~1.5 s, then keeps watching.
fn watcher(d: &Path, tag: &str) -> String {
    let script = d.join(format!("{tag}.sh"));
    std::fs::write(&script, "i=0; while :; do i=$((i+1)); echo \"watch: tick $i, 42 passed\"; [ $i = 15 ] && echo 'FAIL src/a.test.ts: expected 1 to be 2'; sleep 0.1; done\n").unwrap();
    format!("sh {}", script.display())
}

/// A build: 300 lines over 3 s, then an error and exit code 2.
fn build(d: &Path, tag: &str) -> String {
    let script = d.join(format!("{tag}.sh"));
    std::fs::write(&script, "for i in $(seq 1 300); do echo \"   Compiling crate$i v0.1.0\"; sleep 0.01; done; echo 'error[E0308]: mismatched types at src/lib.rs:12'; exit 2\n").unwrap();
    format!("sh {}", script.display())
}

fn row(scenario: &'static str, approach: &'static str, tag: &str, started: Instant, run: Run, ok: impl Fn(&Run) -> bool) -> Row {
    let secs = started.elapsed().as_secs_f32();
    let ok = ok(&run);
    let left = alive(tag);
    kill_left(tag);
    Row { scenario, approach, run, secs, ok, left }
}

fn any(run: &Run, s: &str) -> bool {
    run.outputs.iter().any(|o| o.contains(s))
}

fn dev_server() -> Vec<Row> {
    let mut rows = Vec::new();
    let check = |r: &Run| any(r, "ready on") && any(r, "GET / 200") && r.alive_mid;

    let (d, tag) = (dir("srv-bg"), "bbsrvbg");
    let (cmd, t, mut r) = (server(&d, tag), Instant::now(), Run::default());
    let id = bg_id(&r.bg(&d, json!({ "action": "start", "cmd": cmd, "until": "ready" })));
    r.bg(&d, json!({ "action": "read", "id": id, "wait": 300 }));
    r.alive_mid = alive(tag) > 0;
    r.bg(&d, json!({ "action": "stop", "id": id }));
    rows.push(row("dev server", "bg tool", tag, t, r, check));

    // What agents usually write first: `&` keeps bash's pipes open, so the call blocks until
    // its timeout (120 s by default, 10 s here) and the kill takes the server with it.
    let (d, tag) = (dir("srv-amp"), "bbsrvamp");
    let (cmd, t, mut r) = (server(&d, tag), Instant::now(), Run::default());
    r.bash_t(&d, &format!("{cmd} &"), 10_000);
    r.bash(&d, "tail -n 5 /dev/null; echo checked");
    r.alive_mid = alive(tag) > 0;
    rows.push(row("dev server", "bash `&` (naive)", tag, t, r, check));

    let (d, tag) = (dir("srv-sleep"), "bbsrvsleep");
    let (cmd, t, mut r) = (server(&d, tag), Instant::now(), Run::default());
    let pid = r.bash(&d, &format!("nohup {cmd} > server.log 2>&1 & echo $!"));
    r.bash(&d, "sleep 5; tail -n 20 server.log");
    r.bash(&d, "tail -n 20 server.log");
    r.alive_mid = alive(tag) > 0;
    r.bash(&d, &format!("kill {}", pid.trim()));
    rows.push(row("dev server", "bash nohup + sleep", tag, t, r, check));

    let (d, tag) = (dir("srv-poll"), "bbsrvpoll");
    let (cmd, t, mut r) = (server(&d, tag), Instant::now(), Run::default());
    let pid = r.bash(&d, &format!("nohup {cmd} > server.log 2>&1 & echo $!"));
    r.bash(&d, "for i in $(seq 1 100); do grep -q 'ready on' server.log && break; sleep 0.2; done; tail -n 20 server.log");
    r.bash(&d, "sleep 0.3; tail -n 20 server.log");
    r.alive_mid = alive(tag) > 0;
    r.bash(&d, &format!("kill {}", pid.trim()));
    rows.push(row("dev server", "bash nohup + poll loop", tag, t, r, check));

    // Careful bash: the job shares the bash call's process group (winfer starts each call in its
    // own), so killing that group gets the child too. (`set -m`/`setsid` don't work here: zsh -c
    // refuses `set -m` and macOS has no setsid.)
    let (d, tag) = (dir("srv-pg"), "bbsrvpg");
    let (cmd, t, mut r) = (server(&d, tag), Instant::now(), Run::default());
    let pid = r.bash(&d, &format!("nohup {cmd} > server.log 2>&1 & echo $!"));
    r.bash(&d, "for i in $(seq 1 100); do grep -q 'ready on' server.log && break; sleep 0.2; done; tail -n 20 server.log");
    r.bash(&d, "sleep 0.3; tail -n 20 server.log");
    r.alive_mid = alive(tag) > 0;
    r.bash(&d, &format!("kill -- -$(ps -o pgid= -p {} | tr -d ' ')", pid.trim()));
    rows.push(row("dev server", "bash nohup + poll + kill group", tag, t, r, check));
    rows
}

fn watch() -> Vec<Row> {
    let mut rows = Vec::new();
    let check = |r: &Run| any(r, "FAIL src/a.test.ts");

    let (d, tag) = (dir("w-bg"), "bbwbg");
    let (cmd, t, mut r) = (watcher(&d, tag), Instant::now(), Run::default());
    let id = bg_id(&r.bg(&d, json!({ "action": "start", "cmd": cmd, "until": "FAIL|PASS" })));
    r.bg(&d, json!({ "action": "stop", "id": id }));
    rows.push(row("test watcher", "bg tool", tag, t, r, check));

    let (d, tag) = (dir("w-sleep"), "bbwsleep");
    let (cmd, t, mut r) = (watcher(&d, tag), Instant::now(), Run::default());
    let pid = r.bash(&d, &format!("nohup {cmd} > watch.log 2>&1 & echo $!"));
    r.bash(&d, "sleep 3; tail -n 30 watch.log");
    r.bash(&d, &format!("kill {}", pid.trim()));
    rows.push(row("test watcher", "bash nohup + sleep", tag, t, r, check));

    let (d, tag) = (dir("w-poll"), "bbwpoll");
    let (cmd, t, mut r) = (watcher(&d, tag), Instant::now(), Run::default());
    let pid = r.bash(&d, &format!("nohup {cmd} > watch.log 2>&1 & echo $!"));
    r.bash(&d, "for i in $(seq 1 300); do grep -qE 'FAIL|PASS' watch.log && break; sleep 0.2; done; grep -E 'FAIL|PASS' watch.log; tail -n 5 watch.log");
    r.bash(&d, &format!("kill {}", pid.trim()));
    rows.push(row("test watcher", "bash nohup + poll loop", tag, t, r, check));
    rows
}

fn long_build() -> Vec<Row> {
    let mut rows = Vec::new();
    let check = |r: &Run| any(r, "E0308") && (any(r, "exited 2") || any(r, "exit code 2") || any(r, "EXIT 2"));

    // Started in the background so the agent can work meanwhile (one read call stands in for that).
    let (d, tag) = (dir("b-bg"), "bbbbg");
    let (cmd, t, mut r) = (build(&d, tag), Instant::now(), Run::default());
    let id = bg_id(&r.bg(&d, json!({ "action": "start", "cmd": cmd })));
    r.call(&d, "glob", json!({ "pattern": "*" }));
    r.bg(&d, json!({ "action": "read", "id": id, "wait": 60000 }));
    r.bg(&d, json!({ "action": "stop", "id": id }));
    rows.push(row("build (exit 2)", "bg tool", tag, t, r, check));

    let (d, tag) = (dir("b-poll"), "bbbpoll");
    let (cmd, t, mut r) = (build(&d, tag), Instant::now(), Run::default());
    r.bash(&d, &format!("nohup sh -c '{cmd}; echo \"EXIT $?\"' > build.log 2>&1 &"));
    r.call(&d, "glob", json!({ "pattern": "*" }));
    r.bash(&d, "for i in $(seq 1 300); do grep -q '^EXIT ' build.log && break; sleep 0.2; done; tail -n 20 build.log");
    rows.push(row("build (exit 2)", "bash nohup + EXIT marker", tag, t, r, check));

    // Blocking in the foreground: simplest, but the agent can't do anything meanwhile and the
    // whole log fills its context.
    let (d, tag) = (dir("b-fg"), "bbbfg");
    let (cmd, t, mut r) = (build(&d, tag), Instant::now(), Run::default());
    r.call(&d, "glob", json!({ "pattern": "*" }));
    r.bash(&d, &cmd);
    rows.push(row("build (exit 2)", "bash foreground", tag, t, r, check));
    rows
}

/// The session ends (or the agent forgets to stop) while things still run.
fn cleanup() -> Vec<Row> {
    let mut rows = Vec::new();
    let check = |_: &Run| true;

    let (d, tag) = (dir("c-bg"), "bbcbg");
    let (cmd, t, mut r) = (server(&d, tag), Instant::now(), Run::default());
    r.bg(&d, json!({ "action": "start", "cmd": cmd }));
    super::bg::stop_all(); // what winfer code does on exit
    std::thread::sleep(Duration::from_millis(200));
    rows.push(row("forgotten / exit", "bg tool", tag, t, r, check));

    let (d, tag) = (dir("c-bash"), "bbcbash");
    let (cmd, t, mut r) = (server(&d, tag), Instant::now(), Run::default());
    r.bash(&d, &format!("nohup {cmd} > server.log 2>&1 &"));
    std::thread::sleep(Duration::from_millis(200));
    rows.push(row("forgotten / exit", "bash nohup", tag, t, r, check));
    rows
}

#[test]
#[ignore = "benchmark: takes ~40 s"]
fn bg_bench() {
    let _ = crate::env::path_var(); // resolved once (~1 s), not part of any approach
    let mut rows = Vec::new();
    for f in [dev_server, watch, long_build, cleanup] {
        rows.extend(f());
    }
    println!("\n| scenario | approach | calls | time s | model writes B | model reads B | ok | left running |");
    println!("|---|---|--:|--:|--:|--:|:-:|--:|");
    for r in &rows {
        println!(
            "| {} | {} | {} | {:.1} | {} | {} | {} | {} |",
            r.scenario, r.approach, r.run.calls, r.secs, r.run.written, r.run.read, if r.ok { "yes" } else { "NO" }, r.left
        );
    }
    if std::env::var_os("BG_BENCH_VERBOSE").is_some() {
        for r in &rows {
            println!("\n## {} / {}", r.scenario, r.approach);
            for (o, t) in r.run.outputs.iter().zip(&r.run.times) {
                println!("--- {t:.2}s\n{}", tools::clip(o, 600));
            }
        }
    }
}
