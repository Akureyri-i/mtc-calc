//! The game's shell type icons (the official icon library, `tools/overlay/shell_icons_bin.py`):
//! 64 px premultiplied BGRA, drawn beside the shell list's names as the game's cards draw them.

use std::sync::OnceLock;

/// Each icon's file name ("he", "time-fuze") and pixels.
fn icons() -> &'static [(String, Vec<u8>)] {
    static ICONS: OnceLock<Vec<(String, Vec<u8>)>> = OnceLock::new();
    ICONS.get_or_init(|| {
        let bytes: &[u8] = include_bytes!("../assets/shell_icons.bin");
        assert_eq!(&bytes[..4], b"MTCS", "shell icon asset");
        let count = usize::from(u16::from_le_bytes([bytes[4], bytes[5]]));
        let side = usize::from(u16::from_le_bytes([bytes[6], bytes[7]]));
        let mut at = 8;
        (0..count)
            .map(|_| {
                let len = usize::from(bytes[at]);
                let name = String::from_utf8(bytes[at + 1..at + 1 + len].to_vec()).expect("icon name");
                at += 1 + len;
                let pixels = bytes[at..at + side * side * 4].to_vec();
                at += side * side * 4;
                (name, pixels)
            })
            .collect()
    })
}

/// Pixels per side of every icon.
pub const SIDE: u32 = 64;

/// The icon file of a shell data `type` ("HE" -> "he").
pub fn for_kind(kind: &str) -> Option<&'static [u8]> {
    let name = match kind {
        "HE" => "he",
        "TIME FUZE" => "time-fuze",
        "PROXIMITY FUZE" => "proximity-fuze",
        "SMOKE" => "smoke-shell",
        "CLUSTER" => "cluster",
        "ROCKET" => "rocket",
        "HEAT" => "heat",
        "HEAT-FS" => "heat-fs",
        "HESH" => "hesh",
        "SAPHE" => "saphe",
        "THERMOBARIC" => "thermobaric",
        "INCENDIARY" => "incendiary",
        "Top Attack Shell" => "top-attack-shell",
        _ => return None,
    };
    icons().iter().find(|(n, _)| n == name).map(|(_, p)| p.as_slice())
}
