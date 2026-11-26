//! What one loading UI read costs, step by step: in a real match a read took 0.6 ms with no menu
//! on screen and up to 24 ms with one. Runs the steps the HUD's loading pipeline runs on every
//! read (menu, loader bar, gunner bar, countdown) and the occasional ones (card texts at most
//! once a second, weapon name), on the labelled frames' read-back region.
//!
//! `cargo run --release -p mtc-vision --example loading_cost [-- <session> [--every N]]`: the labelled
//! frames, or every Nth frame of a recording (driving, tanks: not in the label set).

use std::time::Instant;

use mtc_dataset::eval::labelled_frames;
use mtc_vision::FrameView;
use mtc_vision::aim::{AimReader, AimResult};
use mtc_vision::bar::BarReader;
use mtc_vision::loading::{LOADING_REGION, countdown_progress, countdown_shown, gunner_bar_highlight, read_card_texts, read_card_texts_among, read_loader_bar, read_menu, read_weapon_label};
use mtc_vision::text::TextReader;

#[derive(Default)]
struct Step {
    with_menu: Vec<f64>,
    without: Vec<f64>,
}

fn time<T>(f: impl FnOnce() -> T) -> (T, f64) {
    let t = Instant::now();
    let out = f();
    (out, t.elapsed().as_secs_f64() * 1000.0)
}

fn median(v: &[f64]) -> f64 {
    if v.is_empty() {
        return f64::NAN;
    }
    let mut v = v.to_vec();
    v.sort_by(f64::total_cmp);
    v[v.len() / 2]
}

fn main() {
    let mut aim = AimReader::new();
    let bar = BarReader::new();
    let reader = TextReader::new();
    let names = ["menu", "loader's ammo bar", "gunner's bar layout", "gunner's bar lit cell", "countdown", "countdown progress", "card text (at most once a second)", "weapon name (now and then)", "card text, whole strings (as the HUD reads)", "card text, letter by letter (old reader)"];
    // The ammunition data the HUD matches card names against (the snapshot the tests use).
    let ammo = mtc_ammo::AmmoData::from_json(&std::fs::read_to_string("reference/wiki/production-1.0.0/20261117/shells.json").expect("shells snapshot")).expect("ammo data");
    let mut steps: Vec<Step> = names.iter().map(|_| Step::default()).collect();
    let mut frames = (0, 0);
    let args: Vec<String> = std::env::args().skip(1).collect();
    let every: usize = args.iter().position(|a| a == "--every").and_then(|i| args.get(i + 1)).and_then(|v| v.parse().ok()).unwrap_or(1);
    // Streamed: a recording's frames are loaded one at a time.
    let inputs: Box<dyn Iterator<Item = (mtc_dataset::image::RgbImage, mtc_dataset::FrameMeta, String)>> = match args.first().filter(|a| !a.starts_with("--")) {
        Some(id) => {
            let session = mtc_dataset::Session::open(mtc_dataset::datasets_dir().join("raw").join(id)).expect("session");
            let mut metas: Vec<_> = session.frames.to_vec();
            metas.sort_by_key(|m| m.t_ms);
            Box::new(metas.into_iter().step_by(every).map(move |m| (mtc_dataset::image::RgbImage::load_png(&session.frame_path(&m)).expect("frame"), m.clone(), m.file.clone())))
        }
        None => Box::new(labelled_frames().expect("dataset").into_iter().filter(|f| f.label.loading.is_some()).map(|f| (f.image().expect("frame image"), f.meta.clone(), f.id()))),
    };
    let mut slow: Vec<(f64, String, bool)> = Vec::new();
    for (image, meta, id) in inputs {
        let full = FrameView::rgb(&image.data, image.width, image.height);
        let o = meta.geometry.client_in_frame();
        let origin = (o.x, o.y);
        let (rx, ry, rw, rh) = LOADING_REGION;
        let view = full.sub(origin.0 + rx, origin.1 + ry, rw as u32, rh as u32);
        let label_x = match aim.read(&full, origin) {
            AimResult::Read(r) => Some(r.label_x),
            AimResult::Unreadable { label_x, .. } => Some(label_x),
            AimResult::Absent => None,
        };
        let (menu, t0) = time(|| read_menu(&view, origin));
        let (_, t1) = time(|| read_loader_bar(&view, origin));
        let (layout, t2) = time(|| label_x.and_then(|x| bar.layout(&view, origin, x)));
        let (_, t3) = time(|| layout.as_ref().and_then(|l| gunner_bar_highlight(&view, origin, l)));
        let (countdown, t4) = time(|| countdown_shown(&view, origin));
        let (_, t5) = time(|| countdown.then(|| countdown_progress(&view, origin)));
        let (label, t7) = time(|| read_weapon_label(&view, origin, &reader));
        let weapons = label.as_deref().map(|l| ammo.label_weapons(l)).unwrap_or_default();
        let t8 = menu.as_ref().map(|m| time(|| read_card_texts_among(&view, origin, m, &reader, &|| ammo.card_velocities(&weapons), &|v| ammo.card_texts(&weapons, v))).1);
        let t9 = menu.as_ref().map(|m| time(|| read_card_texts(&view, origin, m, &reader)).1);
        let t6 = t8;
        let shown = menu.is_some();
        slow.push((t0 + t1 + t2 + t3 + t4 + t5 + t6.unwrap_or(0.0), id, shown));
        if shown { frames.0 += 1 } else { frames.1 += 1 }
        for (i, t) in [Some(t0), Some(t1), Some(t2), Some(t3), Some(t4), Some(t5), t6, Some(t7), t8, t9].into_iter().enumerate() {
            if let Some(t) = t {
                if shown { steps[i].with_menu.push(t) } else { steps[i].without.push(t) }
            }
        }
    }
    println!("{} frames with the menu, {} without; median ms per step:", frames.0, frames.1);
    slow.sort_by(|a, b| b.0.total_cmp(&a.0));
    for (ms, id, shown) in slow.iter().take(8) {
        println!("  slowest: {id} {ms:.1} ms (menu recognised {shown})");
    }
    for (name, s) in names.iter().zip(&steps) {
        println!("  {name}: with the menu {:.2} ms, without {:.2} ms", median(&s.with_menu), median(&s.without));
    }
}
