//! Development: the loading region of a session's frames resampled to the
//! reference geometry as the HUD reads it, with the countdown checks on it; saves each resampled
//! region as PNG.
//!   cargo run --release -p mtc-vision --example countdown_remap_check -- <session> <from_s> <to_s> <out_dir>
use mtc_dataset::image::RgbImage;
use mtc_dataset::{Session, datasets_dir};
use mtc_vision::FrameView;
use mtc_vision::layout::{Placement, UiLayout, picture_shift, shift_probes};
use mtc_vision::loading::{LOADING_REGION, countdown_progress, countdown_progress_in, countdown_shown, find_countdown};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let session = Session::open(datasets_dir().join("raw").join(&args[1])).expect("session");
    let (from, to): (f64, f64) = (args[2].parse().expect("from"), args[3].parse().expect("to"));
    let out = std::path::PathBuf::from(&args[4]);
    std::fs::create_dir_all(&out).expect("out dir");
    for meta in &session.frames {
        let t = meta.t_ms as f64 / 1000.0;
        if t < from || t > to {
            continue;
        }
        let g = &meta.geometry;
        let (cw, ch) = (g.client.width as u32, g.client.height as u32);
        let image = RgbImage::load_png(&session.frame_path(meta)).expect("frame");
        let frame = FrameView::rgb(&image.data, image.width, image.height);
        let c = g.client_in_frame();
        let shift = if image.width > cw {
            let [(bx, by, bw, bh), (rx, ry, rw, rh)] = shift_probes((image.width, image.height));
            picture_shift(&frame.sub(bx, by, bw, bh), &frame.sub(rx, ry, rw, rh)).unwrap_or((0, 0))
        } else {
            (0, 0)
        };
        let placement = Placement::new((c.x - shift.0, c.y - shift.1), UiLayout { width: cw, height: ch, dpi: g.dpi });
        let (vx, vy) = Placement::VIRTUAL_ORIGIN;
        let (x, y, w, h) = LOADING_REGION;
        let rect = (x + vx, y + vy, w, h);
        let (sx, sy, sw, sh) = placement.bottom_source(rect, (image.width, image.height));
        let r = placement.bottom_remap(&frame.sub(sx, sy, sw as u32, sh as u32), rect);
        let v = r.view();
        let started = std::time::Instant::now();
        let found = find_countdown(&v, (vx, vy));
        let us = started.elapsed().as_micros();
        println!(
            "{t:.1} {}x{} dpi {} shift {shift:?}: at the reference's place {} {:?}; found {:?} progress {:?} search {us} us",
            cw,
            ch,
            g.dpi,
            countdown_shown(&v, (vx, vy)),
            countdown_progress(&v, (vx, vy)),
            found.map(|b| b.interior),
            found.and_then(|b| countdown_progress_in(&v, (vx, vy), &b))
        );
        if std::env::var_os("MTC_NO_PNG").is_some() {
            continue;
        }
        let mut img = RgbImage::new(r.width, r.height);
        for i in 0..(r.width * r.height) as usize {
            img.data[i * 3] = r.data[i * 4 + 2];
            img.data[i * 3 + 1] = r.data[i * 4 + 1];
            img.data[i * 3 + 2] = r.data[i * 4];
        }
        img.save_png(&out.join(format!("{:06}.png", meta.t_ms / 100))).expect("save");
    }
}
