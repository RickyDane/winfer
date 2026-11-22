//! Windows: embeds the app icon (resource 1, which GPUI also uses for its windows) and the
//! version info, so the .exe shows winfer's icon in Explorer, the taskbar and Alt+Tab.

fn main() {
    println!("cargo:rerun-if-changed=../../../src-tauri/icons/icon.ico");
    #[cfg(windows)]
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut resource = winresource::WindowsResource::new();
        resource
            .set_icon("../../../src-tauri/icons/icon.ico")
            .set("ProductName", "winfer")
            .set("FileDescription", "winfer")
            .set("OriginalFilename", "winfer.exe");
        resource.compile().expect("couldn't embed the Windows icon");
    }
}
