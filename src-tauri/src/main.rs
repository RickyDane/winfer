// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    // `winfer mcp-stdio`: run as the MCP server agents launch (see mcp.rs), not as the app.
    if std::env::args().nth(1).as_deref() == Some("mcp-stdio") {
        return winfer_lib::run_mcp_stdio();
    }
    // `winfer hook <provider> <event>`: run by agent hooks to report status (see hooks.rs).
    if std::env::args().nth(1).as_deref() == Some("hook") {
        return winfer_lib::run_hook(&std::env::args().skip(2).collect::<Vec<_>>());
    }
    // `winfer code`: winfer's own coding agent in the terminal (see harness/).
    if std::env::args().nth(1).as_deref() == Some("code") {
        return winfer_lib::run_code(&std::env::args().skip(2).collect::<Vec<_>>());
    }
    // `winfer statusline`: Claude Code's status line inside winfer (see usage.rs).
    if std::env::args().nth(1).as_deref() == Some("statusline") {
        return winfer_lib::run_statusline();
    }
    winfer_lib::run()
}
