fn main() {
    // ScreenCaptureKit's Swift bridge needs the Swift compatibility libraries at link time. With
    // only the Command Line Tools installed (no full Xcode) they live under the CLT root, not the
    // Xcode toolchain path the bridge's own build script assumes, so add whichever exists.
    #[cfg(target_os = "macos")]
    {
        let dev = std::process::Command::new("xcode-select").arg("-p").output().ok().and_then(|o| String::from_utf8(o.stdout).ok()).unwrap_or_default();
        let dev = dev.trim();
        for rel in ["usr/lib/swift/macosx", "Toolchains/XcodeDefault.xctoolchain/usr/lib/swift/macosx"] {
            let p = format!("{dev}/{rel}");
            if std::path::Path::new(&p).is_dir() {
                println!("cargo:rustc-link-search=native={p}");
            }
        }
        println!("cargo:rustc-link-arg=-Wl,-rpath,/usr/lib/swift");
    }
    tauri_build::build()
}
