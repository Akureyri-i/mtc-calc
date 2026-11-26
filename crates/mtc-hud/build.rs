//! Embeds the program's icon and its version information (shown in the file's properties).

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
    println!("cargo:rerun-if-changed=assets/icon.ico");
    println!("cargo:rerun-if-changed=Cargo.toml");
}
