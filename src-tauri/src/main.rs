// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    // `winfer mcp-stdio`: run as the MCP server agents launch (see mcp.rs), not as the app.
    if std::env::args().nth(1).as_deref() == Some("mcp-stdio") {
        return winfer_lib::run_mcp_stdio();
    }
    winfer_lib::run()
}
