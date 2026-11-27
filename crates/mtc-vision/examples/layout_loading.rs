//! Development: the loading UI — ammo bar layout, loading menu, weapon label and
//! card names — read on recordings of other display configurations through the layout model
//! (`mtc_vision::layout`): the bottom group from the readout to the cards resampled to the
//! reference geometry, each recognizer at the reference's settings unless an env probe says
//! otherwise. Prints per configuration the frames and how many gave each result.
//!   cargo run --release -p mtc-vision --example layout_loading -- <session> [<session> ...]
//! `MTC_ONLY=dpi,w,h` one configuration; `MTC_BAR_SCALE=x,y` the bar headers' text scale;
//! `MTC_PER_FRAME` one line per frame; `MTC_BAR_SWEEP` instead counts, per configuration, the
//! frames whose bar layout reads at each header text scale of a grid; `MTC_TEXT_SWEEP=<weapon>`
//! counts the frames whose weapon label names that data weapon at each weapon-label text scale;
//! `MTC_GLYPHS=<dir>` reads text off the reference with the glyph files there (`MTC_GLYPHS_ALL`: at
//! the reference configuration too)
//! (`<dpi>_<width>_<height>.json`, `crates/mtc-overlay/examples/ui_glyphs_json.rs`);
//! `MTC_LABEL_CSV=<file>` appends per frame the weapon box's free reading and best two whole-label
//! matches against the data's labels; `MTC_FREE_LABEL` reads the label glyph by glyph only.
use std::collections::BTreeMap;

use mtc_dataset::image::RgbImage;
use mtc_dataset::{Session, datasets_dir};
use mtc_vision::FrameView;
use mtc_vision::aim::{AimReader, AimResult};
use mtc_vision::bar::BarReader;
use mtc_vision::layout::{Placement, UiLayout, picture_shift, shift_probes};
use mtc_ammo::AmmoData;
use mtc_vision::loading::{read_card_texts_among, read_menu, read_weapon_label, read_weapon_label_among, weapon_box, weapon_box_scores};
use mtc_vision::text::TextReader;

/// The bottom group from the readout's search area to the top of the loading cards, reference
/// client coordinates.
const REGION: (i32, i32, i32, i32) = (330, 700, 1400, 634);

fn main() {
    let only = std::env::var("MTC_ONLY").ok();
    let per_frame = std::env::var_os("MTC_PER_FRAME").is_some();
    let bar_scale = std::env::var("MTC_BAR_SCALE").ok().map(|v| {
        let n: Vec<f32> = v.split(',').map(|x| x.parse().expect("x,y")).collect();
        (n[0], n[1])
    });
    let text = TextReader::new();
    let glyph_dir = std::env::var("MTC_GLYPHS").ok();
    let mut glyph_readers: BTreeMap<(u32, u32, u32), TextReader> = BTreeMap::new();
    let data = AmmoData::from_json(&std::fs::read_to_string("reference/wiki/production-1.0.0/20261117/shells.json").expect("ammo data snapshot")).expect("parse");
    let labels = data.label_texts();
    // `MTC_LABEL_CSV=<file>`: per frame the weapon box's free reading and its two best whole-label
    // matches, appended (`read_weapon_label_among`'s thresholds).
    let label_csv = std::env::var("MTC_LABEL_CSV").ok();
    let free_label = std::env::var_os("MTC_FREE_LABEL").is_some();
    let sweep = std::env::var_os("MTC_BAR_SWEEP").is_some();
    let grid: Vec<(f32, f32)> = (0..12).flat_map(|i| (0..9).map(move |j| (0.85 + 0.04 * i as f32, 0.78 + 0.04 * j as f32))).collect();
    let sweep_readers: Vec<BarReader> = if sweep { grid.iter().map(|&s| BarReader::with_text_scale(s)).collect() } else { Vec::new() };
    let mut sweep_counts: BTreeMap<(u32, u32, u32), (usize, Vec<usize>)> = BTreeMap::new();
    let text_sweep = std::env::var("MTC_TEXT_SWEEP").ok();
    let text_grid: Vec<(f32, f32)> = (0..11).flat_map(|i| (0..11).map(move |j| (0.80 + 0.05 * i as f32, 0.80 + 0.05 * j as f32))).collect();
    let text_readers: Vec<TextReader> = if text_sweep.is_some() { text_grid.iter().map(|&s| TextReader::with_scales([s, (1.0, 1.0), (1.0, 1.0)])).collect() } else { Vec::new() };
    for id in std::env::args().skip(1) {
        let session = Session::open(datasets_dir().join("raw").join(&id)).expect("session");
        // per configuration: frames, readout read, and counts of each result per recognizer
        let mut seen: BTreeMap<(u32, u32, u32), (usize, usize, [BTreeMap<String, usize>; 4])> = BTreeMap::new();
        let mut readers: BTreeMap<(u32, u32, u32), (AimReader, BarReader)> = BTreeMap::new();
        let mut shift = (0, 0);
        for meta in &session.frames {
            let g = &meta.geometry;
            let (cw, ch) = (g.client.width as u32, g.client.height as u32);
            let key = (g.dpi, cw, ch);
            if only.as_ref().is_some_and(|o| *o != format!("{},{},{}", key.0, key.1, key.2)) {
                continue;
            }
            let image = RgbImage::load_png(&session.frame_path(meta)).expect("frame");
            let view = FrameView::rgb(&image.data, image.width, image.height);
            let c = g.client_in_frame();
            if image.width > cw {
                let [(bx, by, bw, bh), (rx, ry, rw, rh)] = shift_probes((image.width, image.height));
                shift = picture_shift(&view.sub(bx, by, bw, bh), &view.sub(rx, ry, rw, rh)).unwrap_or(shift);
            } else {
                shift = (0, 0);
            }
            let placement = Placement::new((c.x - shift.0, c.y - shift.1), UiLayout { width: cw, height: ch, dpi: g.dpi });
            let (aim, bar) = readers.entry(key).or_insert_with(|| {
                let scale = bar_scale.unwrap_or((1.0, 1.0));
                (AimReader::with_scale(placement.layout.readout_text()), BarReader::with_text_scale(scale))
            });
            let origin = placement.bottom_origin();
            let remapped;
            let v = if placement.is_reference() {
                view
            } else {
                let (vx, vy) = Placement::VIRTUAL_ORIGIN;
                remapped = placement.bottom_remap(&view, (REGION.0 + vx, REGION.1 + vy, REGION.2, REGION.3));
                remapped.view()
            };
            let e = seen.entry(key).or_default();
            e.0 += 1;
            let label_x = match aim.read(&v, origin) {
                AimResult::Read(r) => {
                    e.1 += 1;
                    Some(r.label_x)
                }
                AimResult::Unreadable { label_x, .. } => Some(label_x),
                AimResult::Absent => None,
            };
            if let Some(want) = &text_sweep {
                let e = sweep_counts.entry(key).or_insert_with(|| (0, vec![0; text_grid.len()]));
                e.0 += 1;
                for (i, r) in text_readers.iter().enumerate() {
                    if read_weapon_label(&v, origin, r).as_deref().and_then(|t| data.weapon_for_label(t)) == Some(want.as_str()) {
                        e.1[i] += 1;
                    }
                }
                continue;
            }
            if sweep {
                if let Some(x) = label_x {
                    let e = sweep_counts.entry(key).or_insert_with(|| (0, vec![0; grid.len()]));
                    e.0 += 1;
                    for (i, r) in sweep_readers.iter().enumerate() {
                        if r.layout(&v, origin, x).is_some_and(|l| l.cells == 5) {
                            e.1[i] += 1;
                        }
                    }
                }
                continue;
            }
            let bar_text = match label_x.map(|x| bar.layout(&v, origin, x)) {
                Some(Some(l)) => format!("{} cells, {} shell{}", l.cells, l.shell_slots, if l.has_grenade { ", G" } else { "" }),
                Some(None) => "no layout".into(),
                None => "no readout".into(),
            };
            let menu = read_menu(&v, origin);
            let menu_text = menu.as_ref().map_or("no menu".into(), |m| format!("{:?} {} cards", m.seat, m.cards.len()));
            // As the HUD: the label, the cards compared with what the label's weapons can show, the
            // weapon told by label and cards.
            let text = match &glyph_dir {
                Some(dir) if !placement.is_reference() || std::env::var_os("MTC_GLYPHS_ALL").is_some() => &*glyph_readers.entry(key).or_insert_with(|| {
                    let path = std::path::Path::new(dir).join(format!("{}_{}_{}.json", key.0, key.1, key.2));
                    TextReader::from_json(&std::fs::read_to_string(&path).expect("glyph file for this configuration"))
                }),
                _ => &text,
            };
            // `MTC_FREE_LABEL`: the label read glyph by glyph only (before whole-label matching).
            let label = if free_label { read_weapon_label(&v, origin, text) } else { read_weapon_label_among(&v, origin, text, &labels) };
            if let Some(path) = &label_csv {
                use std::io::Write;
                let scored = weapon_box_scores(&v, origin, text, &labels);
                let free = read_weapon_label(&v, origin, text).unwrap_or_default();
                let at = |k: usize| scored.get(k).map_or((String::new(), f32::NAN), |&(i, sc)| (labels[i].clone(), sc));
                let ((n1, s1), (n2, s2)) = (at(0), at(1));
                // The width estimate against the ink for the label the free reading names.
                let truth = data.weapon_for_label(&free).and_then(|w| labels.iter().find(|l| data.weapon_for_label(l) == Some(w)));
                let (est, span) = weapon_box(&v, origin).zip(truth).and_then(|(rect, t)| text.width_and_span(&v, mtc_vision::text::TextStyle::Weapon, rect, 90, t)).unwrap_or((f64::NAN, f64::NAN));
                let mut f = std::fs::OpenOptions::new().create(true).append(true).open(path).expect("csv");
                writeln!(f, "{id},{},{},{},{},{free:?},{n1:?},{s1:.4},{n2:?},{s2:.4},{est:.1},{span:.1}", meta.file, key.0, key.1, key.2).expect("write");
            }
            let label_weapons = label.as_deref().map(|t| data.label_weapons(t)).unwrap_or_default();
            let texts = menu.as_ref().map(|m| read_card_texts_among(&v, origin, m, text, &|| data.card_velocities(&label_weapons), &|vel| data.card_texts(&label_weapons, vel))).unwrap_or_default();
            let names: Vec<&str> = texts.iter().filter_map(|t| t.name.as_deref()).collect();
            let found = label.as_deref().and_then(|t| if names.is_empty() { data.weapon_for_label(t) } else { data.weapon_for_label_and_cards(t, &names) });
            let weapon = format!("{} -> {}", label.as_deref().unwrap_or("-"), found.unwrap_or("?"));
            let cards = if texts.is_empty() { "-".into() } else { texts.iter().map(|t| t.name.clone().unwrap_or_else(|| "?".into())).collect::<Vec<_>>().join(" | ") };
            if per_frame {
                println!("frame {id} {} {key:?} bar {bar_text} · menu {menu_text} · weapon {weapon} · cards {cards}", meta.file);
            }
            for (i, s) in [bar_text, menu_text, weapon, cards].into_iter().enumerate() {
                *e.2[i].entry(s).or_default() += 1;
            }
        }
        println!("{id}");
        let grid = if text_sweep.is_some() { &text_grid } else { &grid };
        if let Ok(path) = std::env::var("MTC_SWEEP_CSV") {
            use std::io::Write;
            let mut f = std::fs::OpenOptions::new().create(true).append(true).open(path).expect("csv");
            for ((dpi, w, h), (n, counts)) in &sweep_counts {
                for (s, c) in grid.iter().zip(counts) {
                    writeln!(f, "{dpi},{w},{h},{n},{:.2},{:.2},{c}", s.0, s.1).expect("write");
                }
            }
        }
        for ((dpi, w, h), (n, counts)) in std::mem::take(&mut sweep_counts) {
            let best = *counts.iter().max().unwrap_or(&0);
            let good: Vec<String> = grid.iter().zip(&counts).filter(|&(_, &c)| c == best && best > 0).map(|(s, _)| format!("({:.2},{:.2})", s.0, s.1)).collect();
            let xs: Vec<f32> = grid.iter().zip(&counts).filter(|&(_, &c)| c == best && best > 0).map(|(s, _)| s.0).collect();
            let ys: Vec<f32> = grid.iter().zip(&counts).filter(|&(_, &c)| c == best && best > 0).map(|(s, _)| s.1).collect();
            let span = |v: &[f32]| v.iter().copied().fold((f32::MAX, f32::MIN), |(a, b), x| (a.min(x), b.max(x)));
            println!("  sweep {}% {w}x{h}: {best}/{n} best; x {:?} y {:?}; {} scales", dpi * 100 / 96, span(&xs), span(&ys), good.len());
        }
        for ((dpi, w, h), (n, read, tallies)) in seen {
            println!("  {}% {w}x{h}: {n} frames, readout {read}", dpi * 100 / 96);
            for (name, t) in ["bar", "menu", "weapon", "cards"].iter().zip(&tallies) {
                let mut v: Vec<(&String, &usize)> = t.iter().collect();
                v.sort_by(|a, b| b.1.cmp(a.1));
                let shown: Vec<String> = v.iter().take(4).map(|(s, c)| format!("{s} ×{c}")).collect();
                println!("    {name}: {}", shown.join(", "));
            }
        }
    }
}
