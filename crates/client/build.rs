//! Windows: embed the icon and version info in the .exe, so Explorer, the
//! taskbar and the installer show the logo.

fn main() {
    println!("cargo:rerun-if-changed=assets/logo.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let mut resource = winresource::WindowsResource::new();
    resource
        .set_icon("assets/logo.ico")
        .set("ProductName", "Sisyphus")
        .set("FileDescription", "Sisyphus")
        .set("OriginalFilename", "Sisyphus.exe");
    if let Err(e) = resource.compile() {
        panic!("embedding the Windows icon failed: {e}");
    }
}
