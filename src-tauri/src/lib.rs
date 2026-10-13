mod env;
#[cfg(target_os = "macos")]
mod menu;
mod providers;
mod pty;

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
        .manage(pty::PtyManager::default())
        .invoke_handler(tauri::generate_handler![
            providers::providers_list,
            pty::pty_spawn,
            pty::pty_write,
            pty::pty_resize,
            pty::pty_kill,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
