//! What a seat menu read costs on recorded frames: in a real tank match the HUD's reads took
//! 19 ms, against 7.1 ms on the labelled set. Each frame's seat
//! region is copied out as BGRA, as the live read-back hands it over, and read; the time is set
//! against how much of the region is near-white (the reader looks for glyphs only where a window
//! has a near-white pixel).
//!
//! `cargo run --release -p mtc-vision --example seat_cost -- <session> [<session>…] [--every N]`

use std::time::Instant;

use mtc_vision::seats::SeatReader;
use mtc_vision::{FrameView, PixelFormat};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let every: usize = args.iter().position(|a| a == "--every").and_then(|i| args.get(i + 1)).and_then(|v| v.parse().ok()).unwrap_or(1);
    let sessions: Vec<&String> = args.iter().enumerate().filter(|(i, a)| !a.starts_with("--") && (*i == 0 || args[i - 1] != "--every")).map(|(_, a)| a).collect();
    let reader = SeatReader::new();
    let mut all: Vec<(f64, f64, bool)> = Vec::new();
    for id in sessions {
        let session = mtc_dataset::Session::open(mtc_dataset::datasets_dir().join("raw").join(id)).expect("session");
        let mut frames: Vec<_> = session.frames.iter().collect();
        frames.sort_by_key(|m| m.t_ms);
        let mut rows: Vec<(f64, f64, bool)> = Vec::new();
        for meta in frames.into_iter().step_by(every) {
            let image = mtc_dataset::image::RgbImage::load_png(&session.frame_path(meta)).expect("frame");
            let c = meta.geometry.client_in_frame();
            let client = (c.x, c.y, c.width as u32, c.height as u32);
            let (rx, ry, rw, rh) = SeatReader::region(client);
            // The region as BGRA, like the live read-back.
            let mut data = Vec::with_capacity(rw as usize * rh as usize * 4);
            let mut bright = 0usize;
            for y in ry..ry + rh as i32 {
                for x in rx..rx + rw as i32 {
                    let i = (y as usize * image.width as usize + x as usize) * 3;
                    let (r, g, b) = (image.data[i], image.data[i + 1], image.data[i + 2]);
                    data.extend_from_slice(&[b, g, r, 255]);
                    if 0.299 * f32::from(r) + 0.587 * f32::from(g) + 0.114 * f32::from(b) >= 200.0 {
                        bright += 1;
                    }
                }
            }
            let view = FrameView { data: &data, format: PixelFormat::Bgra, origin: (rx, ry), width: rw, height: rh, stride: rw as usize * 4 };
            let t = Instant::now();
            let menu = reader.read(&view, client);
            let ms = t.elapsed().as_secs_f64() * 1000.0;
            rows.push((ms, bright as f64 / (rw as f64 * rh as f64), menu.is_some()));
        }
        report(id, &rows);
        all.extend(rows);
    }
    report("all", &all);
    // Time by how near-white the region is.
    for (lo, hi) in [(0.0, 0.005), (0.005, 0.02), (0.02, 0.05), (0.05, 0.1), (0.1, 0.2), (0.2, 1.01)] {
        let sel: Vec<f64> = all.iter().filter(|r| r.1 >= lo && r.1 < hi).map(|r| r.0).collect();
        if !sel.is_empty() {
            println!("  near-white pixels {:>4.1}–{:>5.1}%: {:4} frames, median {:.1} ms", lo * 100.0, hi * 100.0, sel.len(), median(sel));
        }
    }
}

fn median(mut v: Vec<f64>) -> f64 {
    v.sort_by(f64::total_cmp);
    v[v.len() / 2]
}

fn report(name: &str, rows: &[(f64, f64, bool)]) {
    if rows.is_empty() {
        return;
    }
    let mut ms: Vec<f64> = rows.iter().map(|r| r.0).collect();
    ms.sort_by(f64::total_cmp);
    let found = rows.iter().filter(|r| r.2).count();
    let bright = median(rows.iter().map(|r| r.1).collect());
    println!(
        "{name}: {} frames, menu on {found}; per read median {:.1} ms, p90 {:.1} ms, longest {:.1} ms; near-white pixels median {:.1}%",
        rows.len(),
        ms[ms.len() / 2],
        ms[ms.len() * 9 / 10],
        ms[ms.len() - 1],
        bright * 100.0
    );
}
