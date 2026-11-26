//! Development: the loaded slot's shell icon on the gunner's bar over every recording: per session
//! and slot, which icon was read how often, with the weakest score and lead; frames where no icon
//! was decided are counted apart. A crew-loading session is checked against what
//! was loaded (gunner's bar with a loader seated: 1 HE, 2 proximity fuze, 3 time fuze, 4 smoke).
//!   cargo run --release -p mtc-vision --example bar_icon_eval
use std::collections::BTreeMap;

use mtc_dataset::image::RgbImage;
use mtc_dataset::{Session, datasets_dir, sessions};
use mtc_vision::FrameView;
use mtc_vision::aim::{AimReader, AimResult};
use mtc_vision::bar::BarReader;
use mtc_vision::loading::{ShellIcons, gunner_bar_highlight};

fn main() {
    let mut aim = AimReader::new();
    let bar = BarReader::new();
    let icons = ShellIcons::new();
    // (session, slot) -> icon -> (frames, weakest score, weakest lead); undecided frames
    let mut table: BTreeMap<(String, usize), BTreeMap<String, (u32, f32, f32)>> = BTreeMap::new();
    let mut undecided: BTreeMap<(String, usize), u32> = BTreeMap::new();
    let (mut crew_right, mut crew_wrong) = (0, 0);
    for info in sessions().expect("sessions") {
        let id = info.info.id.clone();
        let session = Session::open(datasets_dir().join("raw").join(&id)).expect("session");
        for meta in &session.frames {
            let image = RgbImage::load_png(&session.frame_path(meta)).expect("frame");
            let view = FrameView::rgb(&image.data, image.width, image.height);
            let o = meta.geometry.client_in_frame();
            let origin = (o.x, o.y);
            let label_x = match aim.read(&view, origin) {
                AimResult::Read(r) => r.label_x,
                AimResult::Unreadable { label_x, .. } => label_x,
                AimResult::Absent => continue,
            };
            let Some(layout) = bar.layout(&view, origin, label_x) else { continue };
            let Some(slot) = gunner_bar_highlight(&view, origin, &layout) else { continue };
            match icons.read(&view, origin, &layout, slot) {
                Some(m) => {
                    let e = table.entry((id.clone(), slot + 1)).or_default().entry(m.icon.clone()).or_insert((0, f32::MAX, f32::MAX));
                    e.0 += 1;
                    e.1 = e.1.min(m.score);
                    e.2 = e.2.min(m.margin);
                    if id == "20261119-154607" && meta.t_ms > 46_000 {
                        let expected = ["he", "proximity-fuze", "time-fuze", "smoke-shell"][slot.min(3)];
                        if m.icon == expected { crew_right += 1 } else { crew_wrong += 1 }
                    }
                }
                None => *undecided.entry((id.clone(), slot + 1)).or_default() += 1,
            }
        }
    }
    for ((id, slot), icons) in &table {
        let list: Vec<String> = icons.iter().map(|(n, (c, s, m))| format!("{n} ×{c} (score ≥ {s:.2}, lead ≥ {m:.2})")).collect();
        let none = undecided.get(&(id.clone(), *slot)).copied().unwrap_or(0);
        println!("{id} slot {slot}: {}{}", list.join(", "), if none > 0 { format!(", undecided ×{none}") } else { String::new() });
    }
    for ((id, slot), n) in &undecided {
        if !table.contains_key(&(id.clone(), *slot)) {
            println!("{id} slot {slot}: undecided ×{n}");
        }
    }
    println!("crew session: right {crew_right}, wrong {crew_wrong}");
}
