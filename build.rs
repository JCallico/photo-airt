//! Embeds the app icon in the Windows executable, so Explorer, the Start menu
//! and shortcuts show it. Other platforms take their icon from the package
//! (the macOS app bundle, the Linux icon theme) and from the window at run time.

fn main() {
    println!("cargo:rerun-if-changed=assets/icon/photo-airt.rc");
    println!("cargo:rerun-if-changed=assets/icon/photo-airt.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        embed_resource::compile("assets/icon/photo-airt.rc", embed_resource::NONE).manifest_required().expect("embedding the app icon");
    }
}
