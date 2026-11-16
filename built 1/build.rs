//! Embeds the manifest (DPI awareness, `asInvoker`, Win10/11 compat) and the
//! tray icons (resource id 1 = light glyph for dark taskbars, 2 = dark glyph
//! for light taskbars). Only runs when *building on Windows for Windows*;
//! everywhere else (Linux CI running the engine tests) it is a no-op.

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=assets");

    #[cfg(windows)]
    {
        if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
            let mut res = winres::WindowsResource::new();
            res.set_icon_with_id("assets/glide-light.ico", "1");
            res.set_icon_with_id("assets/glide-dark.ico", "2");
            res.set_manifest_file("assets/glide.manifest");
            res.set("ProductName", "Glide");
            res.set("FileDescription", "Smooth scrolling for Windows mouse wheels");
            if let Err(e) = res.compile() {
                // Resources are cosmetic: never fail the build over them.
                println!("cargo:warning=could not embed Windows resources: {e}");
            }
        }
    }
}
