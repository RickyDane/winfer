mod env;
mod hooks;
#[cfg(target_os = "macos")]
mod menu;
mod providers;
mod pty;
mod store;
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
        .setup(|app| {
            let data_dir = app.path().app_data_dir()?;
            std::fs::create_dir_all(&data_dir)?;
            app.manage(store::Store::open(&data_dir.join("winfer.db"))?);
            hooks::start(app.handle().clone(), &data_dir)?;
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
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
