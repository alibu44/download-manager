fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let mut res = winresource::WindowsResource::new();
    res.set_icon("assets/icon.ico");
    res.set("ProductName", "Download Manager");
    res.set("FileDescription", "Download Manager");
    if let Err(e) = res.compile() {
        println!("cargo:warning=icon resource not embedded: {e}");
    }
}
