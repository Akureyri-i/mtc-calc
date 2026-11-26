//! Development: per-stage time of the live HUD's per-frame readers on whole-screen PNGs (the HUD's
//! `--snapshots`, Roblox window at the screen's top left with its 34 px title bar).
//!   cargo run --release -p mtc-vision --example frame_cost <png ...>
use std::time::Instant;

use mtc_dataset::image::RgbImage;
use mtc_vision::FrameView;
use mtc_vision::aim::{AimReader, AimResult};
use mtc_vision::bar::BarReader;
use mtc_vision::loading::{ShellIcons, countdown_shown, gunner_bar_bags, gunner_bar_highlight, read_loader_bar, read_menu, read_weapon_label};
use mtc_vision::map::CueReader;
use mtc_vision::text::TextReader;

fn main() {
    let (mut aim, cues, bar, icons, reader) = (AimReader::new(), CueReader::new(), BarReader::new(), ShellIcons::new(), TextReader::new());
    let origin = (0, 34);
    for path in std::env::args().skip(1) {
        let image = RgbImage::load_png(std::path::Path::new(&path)).expect("png");
        let view = FrameView::rgb(&image.data, image.width, image.height);
        let mut times = Vec::new();
        let mut lap = |name: &'static str, t: Instant| times.push((name, t.elapsed().as_secs_f64() * 1000.0));
        let t = Instant::now();
        let result = aim.read(&view, origin);
        lap("aim", t);
        let t = Instant::now();
        let c = cues.read(&view, origin);
        lap("cues", t);
        let t = Instant::now();
        let _ = read_menu(&view, origin);
        lap("menu", t);
        let t = Instant::now();
        let _ = read_loader_bar(&view, origin);
        lap("loader_bar", t);
        let label_x = match &result {
            AimResult::Read(r) => Some(r.label_x),
            AimResult::Unreadable { label_x, .. } => Some(*label_x),
            AimResult::Absent => None,
        };
        let t = Instant::now();
        let layout = label_x.and_then(|x| bar.layout(&view, origin, x));
        lap("bar_layout", t);
        let t = Instant::now();
        let slot = layout.as_ref().and_then(|l| gunner_bar_highlight(&view, origin, l));
        lap("highlight", t);
        let t = Instant::now();
        if let (Some(l), Some(s)) = (&layout, slot) {
            let _ = gunner_bar_bags(&view, origin, l, s);
            let _ = icons.read_among(&view, origin, l, s, None);
        }
        lap("bags+icon", t);
        let t = Instant::now();
        let _ = countdown_shown(&view, origin);
        lap("countdown", t);
        let t = Instant::now();
        let label = read_weapon_label(&view, origin, &reader);
        lap("weapon_label", t);
        // The loader strip fallback alone (LOADER_LABEL, and shifted for a dismounted loader).
        for dy in [0, 41] {
            let t = Instant::now();
            let text = reader.read_centred(&view, mtc_vision::text::TextStyle::Weapon, (1000, 1294 + 34 + dy, 1560, 1314 + 34 + dy), 90);
            println!("  loader strip dy {dy}: {:.1} ms {:?}", t.elapsed().as_secs_f64() * 1000.0, text);
        }
        let name = std::path::Path::new(&path).file_name().unwrap().to_string_lossy().to_string();
        let parts: Vec<String> = times.iter().map(|(n, ms)| format!("{n} {ms:.1}")).collect();
        println!("{name} view {:?} scoped {} layout {} label {:?}: {}", c.primary(), c.scoped(), layout.is_some(), label, parts.join(" · "));
    }
}
