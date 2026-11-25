fn main() {
    println!("cargo:rerun-if-changed=assets/icon.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut res = winresource::WindowsResource::new();
        // Resource id 1 is loaded by the tray code.
        res.set_icon_with_id("assets/icon.ico", "1");
        res.set("FileDescription", "ezclip");
        res.set("ProductName", "ezclip");
        if let Err(e) = res.compile() {
            // Not fatal: the app draws its icon at runtime as a fallback.
            println!("cargo:warning=could not embed icon: {e}");
        }
    }
}
