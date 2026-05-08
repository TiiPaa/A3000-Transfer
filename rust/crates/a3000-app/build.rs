//! Build script — embed l'icône Windows + metadata du binaire via winres.
//!
//! Sur Windows uniquement (no-op ailleurs). L'icône doit être présente dans
//! `assets/icon.ico` (multi-résolution recommandé : 16x16 / 32x32 / 256x256).

#[cfg(windows)]
fn main() {
    let mut res = winres::WindowsResource::new();
    res.set_icon("assets/icon.ico");
    res.set("FileDescription", "A3000 Transfer — Yamaha SMDI sample transfer");
    res.set("ProductName", "A3000 Transfer");
    res.set("CompanyName", "TiiPaa");
    res.set("LegalCopyright", "MIT License");
    res.set("FileVersion", env!("CARGO_PKG_VERSION"));
    res.set("ProductVersion", env!("CARGO_PKG_VERSION"));
    if let Err(e) = res.compile() {
        eprintln!("winres compile error: {e}");
    }
}

#[cfg(not(windows))]
fn main() {}
