fn main() {
    // whisper.cpp's Metal backend uses `@available` checks, which call
    // `__isPlatformVersionAtLeast` from clang's runtime when the deployment target is older than
    // the SDK. rustc doesn't link that library, so do it here.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        let dir = std::process::Command::new("clang")
            .arg("--print-resource-dir")
            .output()
            .expect("clang is required to build on macOS");
        let dir = String::from_utf8(dir.stdout).unwrap();
        println!("cargo:rustc-link-search=native={}/lib/darwin", dir.trim());
        println!("cargo:rustc-link-lib=static=clang_rt.osx");
    }
    tauri_build::build()
}
