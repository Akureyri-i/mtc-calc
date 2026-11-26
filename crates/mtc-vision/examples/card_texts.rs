//! Development: what the card text reader makes of a session's labelled loading menus (shell names
//! and VEL values), one line per frame: glyph by glyph, and matched against the whole
//! strings the weapons the label names can show (`mtc_ammo::AmmoData::card_texts`), with the best
//! and runner-up scores.
//!   cargo run --release -p mtc-vision --example card_texts -- <session> [shells.json]
use mtc_ammo::AmmoData;
use mtc_dataset::eval::labelled_frames;
use mtc_dataset::labels::LoadingState;
use mtc_vision::FrameView;
use mtc_vision::loading::{LOADING_REGION, read_card_texts, read_card_texts_among, read_menu, read_weapon_label};
use mtc_vision::text::TextReader;

fn main() {
    let mut args = std::env::args().skip(1);
    let session = args.next().expect("session id");
    let shells = args.next().unwrap_or_else(|| "reference/wiki/production-1.0.0/20261117/shells.json".into());
    let ammo = AmmoData::from_json(&std::fs::read_to_string(&shells).expect("shells json")).expect("ammo data");
    let reader = TextReader::new();
    for frame in labelled_frames().expect("dataset").into_iter().filter(|f| f.session == session) {
        let Some(label) = &frame.label.loading else { continue };
        if label.state != LoadingState::Menu {
            continue;
        }
        let image = frame.image().expect("frame image");
        let full = FrameView::rgb(&image.data, image.width, image.height);
        let o = frame.meta.geometry.client_in_frame();
        let (rx, ry, rw, rh) = LOADING_REGION;
        let view = full.sub(o.x + rx, o.y + ry, rw as u32, rh as u32);
        let Some(menu) = read_menu(&view, (o.x, o.y)) else {
            println!("{}: no menu", frame.meta.file);
            continue;
        };
        let weapon_label = read_weapon_label(&view, (o.x, o.y), &reader).unwrap_or_default();
        let weapons = ammo.label_weapons(&weapon_label);
        let glyphs: Vec<String> = read_card_texts(&view, (o.x, o.y), &menu, &reader).iter().map(|t| t.name.clone().unwrap_or_else(|| "-".into())).collect();
        let started = std::time::Instant::now();
        let matched = read_card_texts_among(&view, (o.x, o.y), &menu, &reader, &|| ammo.card_velocities(&weapons), &|v| ammo.card_texts(&weapons, v));
        let ms = started.elapsed().as_secs_f64() * 1000.0;
        let matched: Vec<String> = matched
            .iter()
            .map(|t| match t.matched {
                Some((best, next)) => format!("{} ({best:.2}/{next:.2})", t.name.as_deref().unwrap_or("-")),
                None => format!("{} (glyphs)", t.name.as_deref().unwrap_or("-")),
            })
            .collect();
        println!("{}: \"{weapon_label}\" {weapons:?} | glyphs: {} | matched: {} | {ms:.1} ms", frame.meta.file, glyphs.join(" / "), matched.join(" / "));
    }
}
