//! macOS: CEF's framework beside the binaries, where `src/mac.rs` loads it
//! from, as cef-dll-sys copies libcef.so beside them on Linux. A link to
//! where cef-dll-sys unpacked it, not a copy of its few hundred MiB.

use std::path::PathBuf;

fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("macos")
        || std::env::var_os("CARGO_FEATURE_CEF").is_none()
    {
        return;
    }
    let framework = "Chromium Embedded Framework.framework";
    let cef = PathBuf::from(std::env::var("DEP_CEF_DLL_WRAPPER_CEF_DIR").expect("CEF's directory"));
    // `<profile>/build/ducktape-web-<hash>/out`: the profile directory
    // (`target/debug`) the binaries land in, as cef-dll-sys finds it
    let out = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR"));
    let link = out
        .ancestors()
        .nth(3)
        .expect("the profile directory")
        .join(framework);
    let _ = std::fs::remove_file(&link);
    // a macOS target builds on a Unix host
    #[cfg(unix)]
    std::os::unix::fs::symlink(cef.join(framework), &link)
        .expect("CEF's framework linked beside the binaries");
    // linked again when the link goes
    println!("cargo::rerun-if-changed={}", link.display());
}
