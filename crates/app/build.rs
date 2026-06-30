//! Embed the app icon into the Windows executable (best-effort).

fn main() {
    #[cfg(windows)]
    {
        println!("cargo:rerun-if-changed=icon.ico");
        if std::path::Path::new("icon.ico").exists() {
            let mut res = winresource::WindowsResource::new();
            res.set_icon("icon.ico");
            if let Err(e) = res.compile() {
                // No resource compiler available - ship without an embedded icon
                // (the installed shortcut still points at icon.ico).
                println!("cargo:warning=icon embed skipped: {e}");
            }
        }
    }
}
