//! Windows: embeds the app icon (resource 1, which GPUI also uses for its windows) and the
//! version info, so the .exe shows winfer's icon in Explorer, the taskbar and Alt+Tab.
//!
//! macOS: compiles `src/dictation/macos.swift` (the composer's dictation) into a static library
//! and links it with the Swift runtime. The privacy usage texts it needs are in the bundle's
//! Info.plist (`scripts/bundle-macos.sh`, or `scripts/dev-app-macos.sh` for debug builds).

use std::path::PathBuf;
use std::process::Command;

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
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        build_swift_dictation();
    }
}

fn xcrun(args: &[&str]) -> String {
    let out = Command::new("xcrun").args(args).output().unwrap_or_else(|e| panic!("xcrun {args:?}: {e}"));
    assert!(out.status.success(), "xcrun {args:?} failed: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8(out.stdout).expect("xcrun output").trim().to_string()
}

fn build_swift_dictation() {
    let src = "src/dictation/macos.swift";
    println!("cargo:rerun-if-changed={src}");
    let out_dir = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR"));
    let arch = match std::env::var("CARGO_CFG_TARGET_ARCH").as_deref() {
        Ok("aarch64") => "arm64",
        Ok("x86_64") => "x86_64",
        other => panic!("unsupported macOS architecture {other:?}"),
    };
    let sdk = xcrun(&["--sdk", "macosx", "--show-sdk-path"]);
    let swiftc = xcrun(&["--sdk", "macosx", "--find", "swiftc"]);
    let lib = out_dir.join("libwinfer_dictation.a");
    let status = Command::new(&swiftc)
        .args(["-emit-library", "-static", "-parse-as-library", "-O", "-swift-version", "5", "-module-name", "WinferDictation"])
        .args(["-target", &format!("{arch}-apple-macos11.0"), "-sdk", &sdk, "-module-cache-path"])
        .arg(out_dir.join("swift-module-cache"))
        .arg("-o")
        .arg(&lib)
        .arg(src)
        .status()
        .expect("couldn't run swiftc");
    assert!(status.success(), "swiftc failed to build {src}");
    println!("cargo:rustc-link-search=native={}", out_dir.display());
    println!("cargo:rustc-link-lib=static=winfer_dictation");
    for framework in ["Foundation", "AVFoundation", "Speech"] {
        println!("cargo:rustc-link-lib=framework={framework}");
    }
    // The Swift runtime: the OS's (macOS 11+ ships it) plus the toolchain's compatibility shims.
    let toolchain = PathBuf::from(&swiftc).parent().and_then(|bin| bin.parent()).map(|usr| usr.join("lib/swift/macosx")).expect("toolchain path");
    println!("cargo:rustc-link-search=native={}", toolchain.display());
    println!("cargo:rustc-link-search=native={sdk}/usr/lib/swift");
    println!("cargo:rustc-link-arg=-Wl,-rpath,/usr/lib/swift");
}
