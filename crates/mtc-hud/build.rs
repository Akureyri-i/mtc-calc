//! Embeds the program's icon and its version information (shown in the file's properties), and
//! with the `bundle` feature lists the bundled files for `include_bytes!` (`src/bundle.rs`).

use std::path::Path;

fn main() {
    let manifest = std::env::var("CARGO_MANIFEST_DIR").expect("manifest dir");
    let out = std::env::var("OUT_DIR").expect("out dir");
    let version = std::env::var("CARGO_PKG_VERSION").expect("version");
    let numbers: Vec<u16> = version.split(['.', '-']).take(3).map(|p| p.parse().unwrap_or(0)).collect();
    let (major, minor, patch) = (numbers[0], numbers.get(1).copied().unwrap_or(0), numbers.get(2).copied().unwrap_or(0));
    let icon = Path::new(&manifest).join("assets").join("icon.ico");
    let rc = format!(
        r#"1 ICON "{icon}"
1 VERSIONINFO
FILEVERSION {major},{minor},{patch},0
PRODUCTVERSION {major},{minor},{patch},0
BEGIN
  BLOCK "StringFileInfo"
  BEGIN
    BLOCK "040904B0"
    BEGIN
      VALUE "FileDescription", "MTC Calculator"
      VALUE "ProductName", "MTC Calculator"
      VALUE "FileVersion", "{version}"
      VALUE "ProductVersion", "{version}"
      VALUE "LegalCopyright", "Copyright (c) 2026 MTC Calculator contributors. MIT License."
      VALUE "OriginalFilename", "MTC-Calculator.exe"
    END
  END
  BLOCK "VarFileInfo"
  BEGIN
    VALUE "Translation", 0x409, 1200
  END
END
"#,
        icon = icon.display().to_string().replace('\\', "\\\\"),
    );
    let path = Path::new(&out).join("mtc-hud.rc");
    std::fs::write(&path, rc).expect("write the resource script");
    embed_resource::compile(&path, embed_resource::NONE).manifest_optional().expect("compile the resources");
    write_bundle_list(&manifest, &out);
    println!("cargo:rerun-if-changed=assets/icon.ico");
    println!("cargo:rerun-if-changed=Cargo.toml");
}

/// `bundle.rs` in `OUT_DIR`: every file under the bundle folder as (path relative to it, bytes);
/// empty without the `bundle` feature.
fn write_bundle_list(manifest: &str, out: &str) {
    let mut entries = Vec::new();
    if std::env::var_os("CARGO_FEATURE_BUNDLE").is_some() {
        let dir = std::env::var_os("MTC_BUNDLE_DIR").map_or_else(|| Path::new(manifest).join("../../scratch/bundle"), std::path::PathBuf::from);
        let dir = dir.canonicalize().expect("bundle folder (python -I tools/release/bundle.py)");
        let mut stack = vec![dir.clone()];
        while let Some(folder) = stack.pop() {
            for entry in std::fs::read_dir(&folder).expect("bundle folder") {
                let path = entry.expect("bundle entry").path();
                if path.is_dir() {
                    stack.push(path);
                } else {
                    let relative = path.strip_prefix(&dir).expect("inside the bundle").to_string_lossy().replace('\\', "/");
                    entries.push((relative, path));
                }
            }
        }
        entries.sort();
        assert!(!entries.is_empty(), "empty bundle folder");
        println!("cargo:rerun-if-changed={}", dir.display());
        println!("cargo:rerun-if-env-changed=MTC_BUNDLE_DIR");
    }
    let mut code = String::from("pub static FILES: &[(&str, &[u8])] = &[\n");
    for (relative, path) in &entries {
        code.push_str(&format!("    ({relative:?}, include_bytes!({:?})),\n", path.display().to_string()));
    }
    code.push_str("];\n");
    std::fs::write(Path::new(out).join("bundle.rs"), code).expect("write the bundle list");
}
