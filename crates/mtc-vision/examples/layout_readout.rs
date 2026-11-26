//! Development: the angle readout read on recordings of other display
//! configurations through the layout model alone (`mtc_vision::layout`): each frame's bottom
//! group mapped to the reference geometry by `UiLayout::bottom`, the reader at
//! `UiLayout::readout_text`, nothing fitted per recording. Prints per configuration the frames
//! read, the readings seen and how many frames read each.
//!   cargo run --release -p mtc-vision --example layout_readout -- <session> [<session> ...]
//! `MTC_PER_FRAME` prints every frame's reading (to diff two builds), `MTC_FILE` reads only the
//! frames whose file name contains it, `MTC_ONLY=dpi,w,h` one configuration.
//!
//! The client origin is found from the frame's pixels (a restored window's recorded geometry is
//! off): below the title bar, right of the border columns.
use std::collections::BTreeMap;

use mtc_dataset::image::RgbImage;
use mtc_dataset::{Session, datasets_dir};
use mtc_vision::FrameView;
use mtc_vision::aim::{AimReader, AimResult};
use mtc_vision::layout::{UiLayout, remap};

/// The client's top-left in the frame, from the pixels: below the title bar (rows the colour of
/// the first row), right of the border columns (the colour of the first column).
fn client_origin(img: &RgbImage, _size: (u32, u32)) -> (i32, i32) {
    let px = |x: u32, y: u32| {
        let i = ((y * img.width + x) * 3) as usize;
        [img.data[i], img.data[i + 1], img.data[i + 2]]
    };
    let near = |a: [u8; 3], b: [u8; 3]| a.iter().zip(b).all(|(p, q)| p.abs_diff(q) <= 6);
    // The title bar's colour, taken left of its buttons; a row is title while most of its samples
    // (the window title's text aside) have it.
    // A restored window has a 1 px border row above its title bar.
    let title = px(img.width / 2, 4);
    let is_title = |y: u32| (img.width / 4..img.width * 3 / 4).step_by(5).filter(|&x| near(px(x, y), title)).count() * 10 >= (img.width / 2 / 5) as usize * 9;
    let first = (0..8).find(|&y| is_title(y)).unwrap_or(0);
    let top = (first..img.height / 10).find(|&y| !is_title(y)).unwrap_or(0);
    let border = px(0, img.height / 2);
    let is_border = |x: u32| (top..img.height - 20).step_by(11).all(|y| near(px(x, y), border));
    let left = (0..8).find(|&x| !is_border(x)).unwrap_or(0);
    (left as i32, top as i32)
}

fn main() {
    for id in std::env::args().skip(1) {
        let session = Session::open(datasets_dir().join("raw").join(&id)).expect("session");
        let mut by_config: BTreeMap<(u32, u32, u32), (usize, usize, BTreeMap<String, usize>)> = BTreeMap::new();
        let mut readers: BTreeMap<(u32, u32, u32), AimReader> = BTreeMap::new();
        // The readout's search area in the reference client.
        let (ax, ay, aw, ah) = AimReader::new().wanted_region((0, 0));
        let virtual_origin = (0, 34);
        for meta in &session.frames {
            let g = &meta.geometry;
            let (cw, ch) = (g.client.width as u32, g.client.height as u32);
            let layout = UiLayout { width: cw, height: ch, dpi: g.dpi };
            let key = (g.dpi, cw, ch);
            if let Ok(only) = std::env::var("MTC_ONLY")
                && only != format!("{},{},{}", key.0, key.1, key.2)
            {
                continue;
            }
            if let Ok(file) = std::env::var("MTC_FILE")
                && !meta.file.contains(&file)
            {
                continue;
            }
            let image = RgbImage::load_png(&session.frame_path(meta)).expect("frame");
            let view = FrameView::rgb(&image.data, image.width, image.height);
            // Maximised (no side border): the recorded client is right; restored: from the pixels.
            let origin = if image.width == cw {
                let c = g.client_in_frame();
                (c.x, c.y)
            } else {
                client_origin(&image, (cw, ch))
            };
            // Right after a resize the recorder can store the new geometry with a frame of the old
            // size (in a height sweep, a 1920x920 picture recorded as 1920x960); not a reading.
            // (The sweeps step the height by 40 px; a restored window's bottom border is a few px.)
            let below = image.height as i32 - origin.1 - ch as i32;
            if !(0..20).contains(&below) {
                *by_config.entry(key).or_default().2.entry("frame of another size".into()).or_default() += 1;
                continue;
            }
            let remapped = remap(&view, origin, &layout.bottom(), (ax, ay, aw, ah), virtual_origin);
            let reader = readers.entry(key).or_insert_with(|| {
                let mut scale = layout.readout_text();
                // Development probes of which part of the text scale matters.
                if std::env::var_os("MTC_REFERENCE_SCALE").is_some() {
                    scale = mtc_vision::aim::TextScale::REFERENCE;
                }
                if let Some(v) = std::env::var("MTC_DIGITS").ok().and_then(|v| v.parse::<f32>().ok()) {
                    scale.digits = (v / layout.hud_scale() as f32, v / layout.hud_scale() as f32);
                }
                if std::env::var_os("MTC_LABEL_Y_EQ_X").is_some() {
                    scale.label.1 = scale.label.0;
                }
                if let Some(v) = std::env::var("MTC_LABEL_X").ok().and_then(|v| v.parse::<f32>().ok()) {
                    scale.label.0 = v / layout.hud_scale() as f32;
                }
                if let Some(v) = std::env::var("MTC_LABEL_Y").ok().and_then(|v| v.parse::<f32>().ok()) {
                    scale.label.1 = v / layout.hud_scale() as f32;
                }
                AimReader::with_scale(scale)
            });
            let e = by_config.entry(key).or_default();
            e.0 += 1;
            if std::env::var_os("MTC_DIRECT").is_some() && layout.hud_scale() > 0.999 {
                // At scale 1 the readout can be read on the frame itself, at the client origin moved so
                // that the centre line falls where the reference has it.
                let shifted = (origin.0 + (cw as i32 - 2560) / 2, origin.1 + ch as i32 - 1334);
                println!("  direct {:?} origin {origin:?}: {:?}", key, match reader.read(&view, shifted) { AimResult::Read(r) => format!("{} / {}", r.elevation_text, r.azimuth_text), AimResult::Unreadable { .. } => "unreadable".into(), AimResult::Absent => "absent".into() });
            }
            if std::env::var_os("MTC_LINE_OFFSETS").is_some() {
                // Which vertical offset of the value lines decodes (and how cheaply), with the label
                // where the reader finds it.
                let v = remapped.view();
                if let Some((_, lx, ly, _)) = reader.probe_label(&v, virtual_origin) {
                    let costs: Vec<String> = (-3..=3)
                        .map(|dy| match reader.probe_lines(&v, (virtual_origin.0, virtual_origin.1 + dy), lx, ly) {
                            Some((e, a, c)) => format!("{dy:+}:{c}{}", if e == "+11.32" || e == "+5.35" { "" } else { "!" }).replace("!", &format!("({e}/{a})")),
                            None => format!("{dy:+}:-"),
                        })
                        .collect();
                    println!("  lines {key:?} label y {}: {}", ly - virtual_origin.1, costs.join(" "));
                }
            }
            let shown = match reader.read(&remapped.view(), virtual_origin) {
                AimResult::Read(r) => {
                    e.1 += 1;
                    format!("{} / {} (label x {})", r.elevation_text, r.azimuth_text, r.label_x - virtual_origin.0)
                }
                AimResult::Unreadable { label_x, .. } => format!("unreadable at label x {}", label_x - virtual_origin.0),
                AimResult::Absent => "absent".into(),
            };
            if std::env::var_os("MTC_PER_FRAME").is_some() {
                // One line per frame, for comparing two builds of the reader.
                println!("frame {id} {} {key:?} {shown}", meta.file);
            }
            *e.2.entry(shown).or_default() += 1;
            if std::env::var_os("MTC_LAYOUT_DUMP").is_some() && e.0 == 3 {
                let path = std::env::temp_dir().join(format!("layout-{}-{}x{}.png", key.0, key.1, key.2));
                let v = remapped.view();
                let mut rgb = Vec::with_capacity((v.width * v.height * 3) as usize);
                for y in 0..v.height as i32 {
                    for x in 0..v.width as i32 {
                        rgb.extend_from_slice(&v.px(v.origin.0 + x, v.origin.1 + y));
                    }
                }
                RgbImage { width: v.width, height: v.height, data: rgb }.save_png(&path).expect("dump");
            }
        }
        println!("{id}");
        for ((dpi, w, h), (n, read, values)) in by_config {
            let mut v: Vec<(String, usize)> = values.into_iter().collect();
            v.sort_by(|a, b| b.1.cmp(&a.1));
            let shown: Vec<String> = v.iter().take(3).map(|(s, c)| format!("{s} ×{c}")).collect();
            println!("  {dpi:3} {w}x{h}: {read}/{n} read  {}", shown.join(", "));
        }
    }
}
