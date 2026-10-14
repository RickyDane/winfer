mod browser;
mod env;
mod hooks;
mod llm;
mod mcp;
mod notify;
#[cfg(target_os = "macos")]
mod menu;
mod providers;
mod pty;
mod review;
mod store;
mod voice;
mod worktree;

use tauri::Manager;

/// `winfer mcp-stdio`: the MCP server agents launch to talk to the running app.
pub fn run_mcp_stdio() {
    mcp::run_stdio();
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
/// Release builds abort on panic with no console to print to (especially on Windows), so the
/// message goes to `crash.log` in the app data folder for bug reports.
fn log_panics() {
    let Some(dir) = dirs::data_dir().map(|d| d.join("dev.winfer.app")) else {
        return;
    };
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = std::fs::create_dir_all(&dir);
        let entry = format!(
            "[{}] winfer {} {}: {info}\n{}\n\n",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_secs()),
            env!("CARGO_PKG_VERSION"),
            std::env::consts::OS,
            std::backtrace::Backtrace::force_capture(),
        );
        if let Ok(mut file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(dir.join("crash.log"))
        {
            use std::io::Write;
            let _ = file.write_all(entry.as_bytes());
        }
        default(info);
    }));
}

pub fn run() {
    log_panics();
    // Resolve the login-shell environment off the main thread so the first spawn is fast.
    std::thread::spawn(|| {
        env::login_env();
    });

    let builder =
        tauri::Builder::default()
        // One winfer at a time: a second launch focuses the running window instead. Two copies
        // would share the database and kill each other's browsers as "orphans".
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.unminimize();
                let _ = window.show();
                let _ = window.set_focus();
            }
        }));
    #[cfg(target_os = "macos")]
    let builder = builder.menu(menu::build);

    builder
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_notification::init())
        .manage(pty::PtyManager::default())
        .manage(browser::BrowserManager::default())
        .manage(voice::Voice::new())
        .setup(|app| {
            let data_dir = app.path().app_data_dir()?;
            std::fs::create_dir_all(&data_dir)?;
            app.manage(store::Store::open(&data_dir.join("winfer.db"))?);
            hooks::start(app.handle().clone(), &data_dir)?;
            browser::kill_orphans(app.handle());
            voice::preload(app.handle());
            voice::register_shortcuts(app.handle());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            providers::providers_list,
            providers::providers_models,
            pty::pty_spawn,
            pty::pty_write,
            pty::pty_resize,
            pty::pty_kill,
            store::state_load,
            store::state_save,
            worktree::worktree_create,
            worktree::worktree_diff,
            worktree::worktree_merge,
            worktree::worktree_remove,
            worktree::worktree_commit,
            worktree::worktree_list,
            worktree::worktree_branches,
            worktree::worktree_from_branch,
            worktree::worktree_fetch,
            notify::notify_action,
            store::history_add,
            store::history_search,
            store::history_delete,
            review::review_test_command,
            review::review_run_tests,
            browser::browser_screenshot,
            browser::browser_status,
            browser::browser_install,
            browser::browser_open,
            browser::browser_new_tab,
            browser::browser_close_tab,
            browser::browser_navigate,
            browser::browser_history,
            browser::browser_attach,
            browser::browser_detach,
            browser::browser_resize,
            browser::browser_input,
            browser::browser_selection,
            browser::inspect::browser_console,
            browser::inspect::browser_console_clear,
            browser::inspect::browser_pick_start,
            browser::inspect::browser_pick_cancel,
            voice::voice_status,
            voice::voice_download_model,
            voice::voice_configure,
            voice::voice_input_devices,
            voice::voice_set_shortcuts,
            voice::voice_test_microphone,
            voice::voice_set_credentials,
            voice::voice_clear_credentials,
            voice::voice_decide,
            voice::voice_test_connection,
            voice::voice_start,
            voice::voice_stop,
            mcp::mcp_respond,
            llm::llm_compose,
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app, event| {
            if let tauri::RunEvent::Exit = event {
                app.state::<browser::BrowserManager>().shutdown();
                voice::shutdown(app);
            }
        });
}
