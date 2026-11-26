//! Looks for the yellow X in the minimap container of every recorded frame, whatever its
//! size: blobs of the X's colour (about (210, 180, 5)) and what the X reader
//! (`read_impacts`, 30 px in every view) makes of the frame. `minimap_x_scan [--view=full|scope]
//! [session…]` (the minimap by default); prints each frame with a blob, its box, and the reader's
//! sightings, then a count per session.

use mtc_dataset::{Session, datasets_dir, image::RgbImage};
use mtc_vision::FrameView;
use mtc_vision::map::impacts::read_impacts;
use mtc_vision::map::{MapTransform, MapView};

fn x_colour(r: u8, g: u8, b: u8) -> bool {
    let (r, g, b) = (i32::from(r), i32::from(g), i32::from(b));
    (r - 210).abs() <= 30 && (g - 180).abs() <= 30 && b <= 40 && (15..=55).contains(&(r - g))
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let view = match args.iter().find_map(|a| a.strip_prefix("--view=")) {
        Some("full") => MapView::Full,
        Some("scope") => MapView::Scope,
        _ => MapView::Mini,
    };
    let wanted: Vec<String> = args.into_iter().filter(|a| !a.starts_with("--")).collect();
    let raw = datasets_dir().join("raw");
    let mut sessions: Vec<_> = std::fs::read_dir(&raw).expect("raw").filter_map(|e| e.ok()).map(|e| e.file_name().to_string_lossy().into_owned()).collect();
    sessions.sort();
    for id in sessions.iter().filter(|s| wanted.is_empty() || wanted.contains(s)) {
        let Ok(session) = Session::open(raw.join(id)) else { continue };
        let (mut frames, mut with_blob, mut with_reader) = (0, 0, 0);
        for meta in &session.frames {
            let Ok(image) = RgbImage::load_png(&session.frame_path(meta)) else { continue };
            frames += 1;
            let o = meta.geometry.client_in_frame();
            let t = MapTransform::fixed(view, (o.x, o.y));
            let (x0, y0, side) = (t.left.round() as i64, t.top.round() as i64, t.size.round() as i64);
            let (w, h) = (i64::from(image.width), i64::from(image.height));
            let px = |x: i64, y: i64| -> Option<(u8, u8, u8)> {
                (x >= 0 && y >= 0 && x < w && y < h).then(|| {
                    let i = ((y * w + x) * 3) as usize;
                    (image.data[i], image.data[i + 1], image.data[i + 2])
                })
            };
            // Blobs of the X's colour (4-connected) in the container.
            let mut seen = vec![false; (side * side).max(0) as usize];
            let mut blobs = Vec::new();
            for sy in 0..side {
                for sx in 0..side {
                    let k = (sy * side + sx) as usize;
                    if seen[k] || !px(x0 + sx, y0 + sy).is_some_and(|(r, g, b)| x_colour(r, g, b)) {
                        continue;
                    }
                    let (mut stack, mut n, mut bx) = (vec![(sx, sy)], 0, (sx, sy, sx, sy));
                    seen[k] = true;
                    while let Some((cx, cy)) = stack.pop() {
                        n += 1;
                        bx = (bx.0.min(cx), bx.1.min(cy), bx.2.max(cx), bx.3.max(cy));
                        for (nx, ny) in [(cx + 1, cy), (cx - 1, cy), (cx, cy + 1), (cx, cy - 1)] {
                            if nx < 0 || ny < 0 || nx >= side || ny >= side {
                                continue;
                            }
                            let nk = (ny * side + nx) as usize;
                            if !seen[nk] && px(x0 + nx, y0 + ny).is_some_and(|(r, g, b)| x_colour(r, g, b)) {
                                seen[nk] = true;
                                stack.push((nx, ny));
                            }
                        }
                    }
                    if n >= 6 {
                        blobs.push((n, bx));
                    }
                }
            }
            let frame = FrameView::rgb(&image.data, image.width, image.height);
            let reader = read_impacts(&frame, &t);
            if !blobs.is_empty() || !reader.is_empty() {
                with_blob += usize::from(!blobs.is_empty());
                with_reader += usize::from(!reader.is_empty());
                let boxes: Vec<String> = blobs.iter().map(|(n, b)| format!("{n}px@({},{})+{}x{}", x0 + b.0, y0 + b.1, b.2 - b.0 + 1, b.3 - b.1 + 1)).collect();
                let read: Vec<String> = reader.iter().map(|s| format!("({:.1},{:.1}) a{:.2}", s.frame.0, s.frame.1, s.alpha)).collect();
                println!("{id}/{} [{}] reader [{}]", meta.file, boxes.join(" "), read.join(" "));
            }
        }
        println!("{id}: {frames} frames, {with_blob} with an X-coloured blob in {view:?}, {with_reader} with a reader sighting");
    }
}
