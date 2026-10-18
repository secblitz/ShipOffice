//! Embeds the sprite atlases the app draws, so the release build is one self-contained binary.
//! The list comes from assets/meta.json; preview-only files (raw sheets, source thumbnails) stay out.

use std::fmt::Write;
use std::path::Path;

fn main() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("assets");
    let meta_path = dir.join("meta.json");
    println!("cargo:rerun-if-changed={}", meta_path.display());
    let meta: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&meta_path).expect("assets/meta.json")).expect("valid meta.json");
    let mut files = vec!["meta.json".to_owned()];
    for a in meta["anims"].as_object().expect("anims").values() {
        for k in ["file", "mask"] {
            if let Some(f) = a[k].as_str() {
                files.push(f.to_owned());
            }
        }
    }
    for k in ["props", "tiles", "cafe", "deskEmpty"] {
        if let Some(f) = meta[k]["file"].as_str() {
            files.push(f.to_owned());
        }
    }
    files.sort();
    files.dedup();
    let mut out = String::from("pub static FILES: &[(&str, &[u8])] = &[\n");
    for f in &files {
        let p = dir.join(f);
        assert!(p.exists(), "missing asset {f}");
        println!("cargo:rerun-if-changed={}", p.display());
        writeln!(out, "    ({f:?}, include_bytes!({:?})),", p.display().to_string()).unwrap();
    }
    out.push_str("];\n");
    std::fs::write(Path::new(&std::env::var("OUT_DIR").unwrap()).join("assets.rs"), out).unwrap();

    // the .exe gets the app icon on Windows
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        #[cfg(windows)]
        {
            let mut res = winresource::WindowsResource::new();
            res.set_icon("assets/brand/shipoffice.ico");
            res.set("ProductName", "ShipOffice");
            res.set("FileDescription", "ShipOffice");
            let _ = res.compile();
        }
    }
}
