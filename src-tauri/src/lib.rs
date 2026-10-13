mod browser;
mod env;
mod hooks;
#[cfg(target_os = "macos")]
mod menu;
mod providers;
mod pty;
mod store;
mod voice;
mod worktree;

use tauri::Manager;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // Resolve the login-shell environment off the main thread so the first spawn is fast.
    std::thread::spawn(|| {
        env::login_env();
    });

    let builder = tauri::Builder::default();
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
            if let Err(err) = voice::register_shortcuts(app.handle()) {
                eprintln!("voice shortcuts unavailable: {err}");
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            providers::providers_list,
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
            voice::voice_set_credentials,
            voice::voice_clear_credentials,
            voice::voice_decide,
            voice::voice_test_connection,
            voice::voice_start,
            voice::voice_stop,
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
