//! `winferd`: winfer for servers. Runs agents and terminals without a window and serves them to the
//! winfer instances it's paired with (Winfer Connect). See docs/plans/winfer-connect.md (headless).

mod control;
mod host;

use std::time::Duration;
use winfer_backend::{harness, hooks, mcp, usage};
use winfer_core::backend;

const HELP: &str = "winferd: winfer for servers

usage:
  winferd run [--lan] [--name <name>] [--pair <link>]   run in the foreground (systemd, launchd, Docker)
  winferd pair <link>                                    pair with a winfer desktop (Settings → Connect)
  winferd status                                         paired machines, links and panes
  winferd unpair <name>                                  forget a paired machine
  winferd --version

environment:
  WINFERD_PAIR=<link>        pair at startup (Docker)
  WINFERD_NAME=<name>        this server's name in winfer (default: the host name)
  WINFER_NATIVE_IDENTIFIER   another data folder, to run a second copy";

fn main() {
    backend::configure(backend::SERVER);
    let args: Vec<String> = std::env::args().skip(1).collect();
    let rest = args.get(1..).unwrap_or(&[]);
    match args.first().map(String::as_str) {
        // What agents running here call (the same subcommands as the desktop binary).
        Some("mcp-stdio") => mcp::run_stdio(),
        Some("mcp-filter") => std::process::exit(mcp::run_filter(rest)),
        Some("hook") => hooks::run_hook(rest),
        Some("statusline") => usage::run_statusline(),
        Some("code") => harness::run(rest),
        Some("--version" | "-v" | "-V") => println!("winferd {}", env!("CARGO_PKG_VERSION")),
        None | Some("run") => run(rest),
        Some("pair") => match rest.first() {
            Some(link) => std::process::exit(control::pair(link)),
            None => fail("usage: winferd pair <link>"),
        },
        Some("status") => std::process::exit(control::status()),
        Some("unpair") if !rest.is_empty() => std::process::exit(control::unpair(&rest.join(" "))),
        Some("unpair") => fail("usage: winferd unpair <name>"),
        Some("help" | "--help" | "-h") => println!("{HELP}"),
        Some(other) => fail(&format!("unknown command {other}\n\n{HELP}")),
    }
}

fn fail(message: &str) -> ! {
    eprintln!("{message}");
    std::process::exit(2)
}

fn flag(args: &[String], name: &str) -> Option<String> {
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned()
}

fn run(args: &[String]) {
    let Some(_lock) = backend::acquire_instance_lock() else {
        fail("winferd is already running (one per user and data folder)");
    };
    if let Err(err) = backend::start() {
        fail(&format!("winferd failed to start: {err}"));
    }
    let connect = winfer_core::connect::get().expect("connect started");
    if let Some(name) = flag(args, "--name").or_else(|| std::env::var("WINFERD_NAME").ok()).filter(|n| !n.trim().is_empty()) {
        connect.rename(&name);
    }
    // Servers are reached through the relay; the LAN listener only on request.
    connect.set_discoverable(args.iter().any(|a| a == "--lan"));
    let host = host::Host::start();
    if let Some(link) = flag(args, "--pair").or_else(|| std::env::var("WINFERD_PAIR").ok()).filter(|l| !l.trim().is_empty()) {
        control::request_pair(&link);
    }
    eprintln!("winferd {} · {} · data in {}", env!("CARGO_PKG_VERSION"), connect.status().name, backend::data_dir().display());
    loop {
        std::thread::sleep(Duration::from_secs(1));
        host.tick();
        control::tick(connect);
    }
}
