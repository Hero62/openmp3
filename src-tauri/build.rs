use std::{fmt::Write, path::Path};

/// Embeds the Default theme and the theme runtime into the binary
/// (`OUT_DIR/embedded.rs`), so release builds are self-contained.
fn embed_dir(out: &mut String, name: &str, root: &Path) {
    let mut files = Vec::new();
    walk(root, root, &mut files);
    files.sort();
    writeln!(out, "pub static {name}: &[(&str, &[u8])] = &[").unwrap();
    for rel in files {
        let abs = root.join(&rel);
        println!("cargo:rerun-if-changed={}", abs.display());
        writeln!(out, "    ({:?}, include_bytes!({:?})),", rel.replace('\\', "/"), abs.display().to_string()).unwrap();
    }
    writeln!(out, "];").unwrap();
}

fn walk(root: &Path, dir: &Path, out: &mut Vec<String>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            walk(root, &p, out);
        } else if let Ok(rel) = p.strip_prefix(root) {
            out.push(rel.to_string_lossy().to_string());
        }
    }
}

fn main() {
    let manifest = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    let repo = Path::new(&manifest).parent().unwrap();
    let mut out = String::new();
    embed_dir(&mut out, "DEFAULT_THEME", &repo.join("themes/default"));
    embed_dir(&mut out, "RUNTIME", &repo.join("ui/runtime"));
    for f in ["themes/theme.schema.json", "themes/layout.schema.json", "THEME_GUIDE.md"] {
        println!("cargo:rerun-if-changed={}", repo.join(f).display());
    }
    println!("cargo:rerun-if-changed={}", repo.join("themes/default").display());
    println!("cargo:rerun-if-changed={}", repo.join("ui/runtime").display());
    std::fs::write(Path::new(&std::env::var("OUT_DIR").unwrap()).join("embedded.rs"), out).unwrap();
    tauri_build::build()
}
