//! Writes `text` in Montserrat at each size to raw BGRA files (`<out>/<weight>_<size>_<n>.bgra`,
//! 240 × 40 px, text centred), for matching the strip's type against the game's readout:
//!   type_samples <out-dir> <medium|regular> <size,size,...> <text>...
fn roblox_font_dir() -> std::path::PathBuf {
    let versions = std::path::PathBuf::from(std::env::var("LOCALAPPDATA").expect("LOCALAPPDATA")).join("Roblox/Versions");
    std::fs::read_dir(versions)
        .expect("Roblox versions")
        .filter_map(Result::ok)
        .map(|e| e.path().join("content/fonts"))
        .filter(|p| p.join("Montserrat-Medium.ttf").is_file())
        .max_by_key(|p| std::fs::metadata(p).and_then(|m| m.modified()).ok())
        .expect("Montserrat in a Roblox install")
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let out = std::path::PathBuf::from(&args[0]);
    let medium = args[1] == "medium";
    let fonts = roblox_font_dir();
    for size in args[2].split(',') {
        for (n, text) in args[3..].iter().enumerate() {
            let pixels = mtc_overlay::render_text_sample(&fonts, text, size.parse().expect("size"), medium, 240, 40).expect("render");
            std::fs::write(out.join(format!("{}_{size}_{n}.bgra", args[1])), pixels).expect("write");
        }
    }
}
