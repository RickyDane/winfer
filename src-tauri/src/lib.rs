mod android;
mod browser;
mod chatgpt;
mod context_breakdown;
mod editors;
mod env;
mod harness;
mod hooks;
mod llm;
mod mcp;
mod notify;
mod plugins;
mod skills;
#[cfg(target_os = "macos")]
mod menu;
mod provider_updates;
mod providers;
mod pty;
mod review;
mod store;
mod usage;
mod usage_history;
mod vault;
mod voice;
mod worktree;

use tauri::Manager;

/// `winfer mcp-stdio`: the MCP server agents launch to talk to the running app.
pub fn run_mcp_stdio() {
    mcp::run_stdio();
}

/// `winfer statusline`: reports Claude Code's usage limits, then runs the user's status line.
pub fn run_statusline() {
    usage::run_statusline();
}

/// `winfer code [args]`: winfer's own coding agent TUI (see harness/).
pub fn run_code(args: &[String]) {
    #[cfg(windows)]
    let args = &harness::attach_console(args.to_vec());
    log_panics();
    harness::run(args);
}

/// `winfer hook <provider> <event> [json]`: forwards an agent hook event to the running app.
pub fn run_hook(args: &[String]) {
    hooks::run_hook(args);
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
        // Updates come from the latest GitHub release's latest.json (see release.yml).
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .manage(pty::PtyManager::default())
        .manage(browser::BrowserManager::default())
        .manage(android::AndroidManager::default())
        .manage(voice::Voice::new())
        .setup(|app| {
            let data_dir = app.path().app_data_dir()?;
            std::fs::create_dir_all(&data_dir)?;
            plugins::init(&data_dir);
            app.manage(store::Store::open(&data_dir.join("winfer.db"))?);
            app.manage(usage_history::UsageHistory::open(&data_dir.join("usage.db"))?);
            usage_history::start(app.handle());
            hooks::start(app.handle().clone(), &data_dir)?;
            browser::kill_orphans(app.handle());
            voice::preload(app.handle());
            voice::register_shortcuts(app.handle());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            providers::providers_list,
            providers::providers_models,
            provider_updates::providers_versions,
            provider_updates::providers_update,
            plugins::plugins_list,
            plugins::plugins_set_enabled,
            plugins::plugins_dir,
            plugins::plugins_source,
            plugins::plugins_read_file,
            plugins::plugins_write_file,
            plugins::plugins_catalog,
            plugins::plugins_install,
            plugins::plugins_uninstall,
            skills::skills_targets,
            skills::skills_list,
            skills::skills_repo,
            skills::skills_install,
            skills::skills_uninstall,
            skills::skills_copy,
            vault::vault_list,
            vault::vault_set,
            vault::vault_delete,
            vault::vault_reveal,
            vault::vault_exec,
            browser::browser_fill_secret,
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
            worktree::worktree_conflicts,
            worktree::worktree_sync,
            editors::editors_list,
            editors::editor_open,
            notify::notify_action,
            android::android_status,
            android::android_start_avd,
            android::android_mirror_start,
            android::android_mirror_stop,
            android::android_screenshot,
            android::android_input,
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
            browser::log::browser_log,
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
            voice::voice_start,
            voice::voice_stop,
            mcp::mcp_respond,
            usage::usage_codex,
            usage::usage_claude,
            usage::usage_codex_context,
            context_breakdown::usage_context_breakdown,
            usage::usage_opencode,
            usage::usage_opencode_context,
            usage::usage_opencode_model,
            usage_history::usage_history,
            usage_history::usage_dirs,
            usage_history::usage_sessions,
            usage_history::usage_by_dir,
            llm::llm_compose,
            chatgpt::chatgpt_status,
            chatgpt::chatgpt_sign_in,
            chatgpt::chatgpt_cancel_sign_in,
            chatgpt::chatgpt_sign_out,
            chatgpt::chatgpt_models,
            plugins::plugins_app_exe,
            harness::harness_settings,
            harness::harness_set_settings,
            harness::harness_models,
            harness::harness_claude_skills,
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app, event| {
            if let tauri::RunEvent::Exit = event {
                app.state::<browser::BrowserManager>().shutdown();
                app.state::<android::AndroidManager>().shutdown();
                voice::shutdown(app);
            }
        });
}
