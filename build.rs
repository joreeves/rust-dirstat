//! Embeds the app icon and version details into rust-dirstat.exe on Windows. File Explorer,
//! shortcuts and Settings > Apps show the icon; Task Manager shows the description.

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=assets/rust-dirstat.ico");
    #[cfg(windows)]
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut resource = winresource::WindowsResource::new();
        resource
            .set_icon("assets/rust-dirstat.ico")
            .set("ProductName", "rust-dirstat")
            .set("FileDescription", "rust-dirstat")
            .set("CompanyName", "Josiah Reeves")
            .set("LegalCopyright", "Copyright (c) 2026 Josiah Reeves");
        resource.compile().expect("embedding the icon needs the Windows SDK resource compiler (rc.exe)");
    }
}
