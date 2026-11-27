//! macOS: compiles `src/ios_sim_macos.m` (CoreSimulator IOSurface + VideoToolbox H.264 +
//! SimulatorKit IndigoHID) into a static library linked only against public Apple frameworks;
//! private frameworks are loaded dynamically at runtime via `dlopen`.

use std::path::PathBuf;
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=src/ios_sim_macos.m");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("macos") {
        return;
    }
    let out_dir = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR"));
    let obj = out_dir.join("ios_sim_macos.o");
    let lib = out_dir.join("libwinfer_ios_sim.a");

    let mut clang = Command::new("clang");
    clang.args(["-c", "-O2", "-fobjc-arc", "-fmodules", "-Wall", "-Wno-deprecated-declarations"]);
    if let Ok(arch) = std::env::var("CARGO_CFG_TARGET_ARCH") {
        let apple_arch = match arch.as_str() {
            "aarch64" => "arm64",
            other => other,
        };
        clang.args(["-arch", apple_arch]);
    }
    let status = clang
        .arg("src/ios_sim_macos.m")
        .arg("-o")
        .arg(&obj)
        .status()
        .expect("couldn't run clang for ios_sim_macos.m");
    assert!(status.success(), "clang failed to compile src/ios_sim_macos.m");

    let ar = Command::new("ar")
        .args(["rcs"])
        .arg(&lib)
        .arg(&obj)
        .status()
        .expect("couldn't run ar");
    assert!(ar.success(), "ar failed to archive libwinfer_ios_sim.a");

    println!("cargo:rustc-link-search=native={}", out_dir.display());
    println!("cargo:rustc-link-lib=static=winfer_ios_sim");
    for framework in ["Foundation", "CoreGraphics", "CoreVideo", "CoreMedia", "VideoToolbox", "IOSurface"] {
        println!("cargo:rustc-link-lib=framework={framework}");
    }
}
