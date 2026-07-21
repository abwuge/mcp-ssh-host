fn main() {
    const ICON_PATH: &str = "assets/mcp-target-ops.ico";

    println!("cargo:rerun-if-changed={ICON_PATH}");

    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        winresource::WindowsResource::new()
            .set_icon(ICON_PATH)
            .compile()
            .expect("failed to embed the application icon");
    }
}
