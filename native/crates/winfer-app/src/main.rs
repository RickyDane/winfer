//! winfer as a native GPUI app. The backend comes unchanged from the Tauri app (see
//! `winfer-backend`), and the binary answers the same subcommands so agents can use it.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod audio;
mod assets;
mod backend;
mod code;
mod fonts;
mod mermaid;
mod model;
mod pdf;
mod request;
mod status;
mod terminal;
mod updates;
mod voice;
mod theme;
mod ui;

use winfer_backend::{harness, hooks, mcp, usage};

fn run_subcommand() -> bool {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let rest = args.get(1..).unwrap_or(&[]);
    match args.first().map(String::as_str) {
        Some("mcp-stdio") => mcp::run_stdio(),
        Some("mcp-filter") => std::process::exit(mcp::run_filter(rest)),
        Some("hook") => hooks::run_hook(rest),
        Some("statusline") => usage::run_statusline(),
        Some("code") => harness::run(rest),
        Some("--version") | Some("-v") | Some("-V") => println!("winfer {}", env!("CARGO_PKG_VERSION")),
        _ => return false,
    }
    true
}

fn main() {
    if run_subcommand() {
        return;
    }
    // Lean Codex always comes with the short system prompt in this build (Settings has no switch
    // for it): keep the shared harness setting in step.
    let mut harness = winfer_backend::harness::config::Config::load();
    if harness.lean_codex_prompt != harness.lean_codex {
        harness.lean_codex_prompt = harness.lean_codex;
        harness.save();
    }
    // Started by an update (updates::relaunch): the old copy is still quitting, so wait for it to let
    // go of the instance lock. Not passed on to the agents' processes.
    let relaunched = std::env::var_os(updates::RELAUNCH_ENV).is_some();
    if relaunched {
        // SAFETY: no other threads yet.
        unsafe { std::env::remove_var(updates::RELAUNCH_ENV) };
    }
    let wait_until = std::time::Instant::now() + std::time::Duration::from_secs(if relaunched { 30 } else { 0 });
    let instance = loop {
        match backend::acquire_instance_lock() {
            None if std::time::Instant::now() < wait_until => std::thread::sleep(std::time::Duration::from_millis(200)),
            lock => break lock,
        }
    };
    // One winfer at a time: two copies would share the database and kill each other's browsers.
    let Some(_instance) = instance else {
        eprintln!("winfer is already running");
        return;
    };
    if let Err(err) = backend::start() {
        eprintln!("winfer failed to start: {err}");
        std::process::exit(1);
    }
    gpui_kit::application().with_assets(assets::AppAssets).run(|cx| {
        gpui_kit::init(cx);
        ui::root::bind_keys(cx);
        ui::root::open_main_window(cx);
        cx.on_app_quit(|_| async { backend::shutdown() }).detach();
    });
}
