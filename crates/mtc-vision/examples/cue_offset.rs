//! Development: how far the Ping icon and the minimap's M box are from where
//! `CueReader::with_layout` puts them, on frames of a recorded session between two times. The
//! best match within ±`radius` px is printed with its offset; a frame whose cue is not on screen
//! still prints its best (low) score.
//!   cargo run --release -p mtc-vision --example cue_offset -- <session> <from s> <to s> [radius]
use mtc_dataset::image::RgbImage;
use mtc_dataset::sessions;
use mtc_vision::FrameView;
use mtc_vision::layout::{UiLayout, picture_shift, shift_probes};
use mtc_vision::map::CueReader;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let id = &args[0];
    let (from, to): (f64, f64) = (args[1].parse().expect("from s"), args[2].parse().expect("to s"));
    let radius: i32 = args.get(3).map_or(60, |r| r.parse().expect("radius"));
    let session = sessions().expect("sessions").into_iter().find(|s| &s.info.id == id).expect("session");
    println!("file,client,dpi,ping,ping_dx,ping_dy,mkey,mkey_dx,mkey_dy");
    for meta in &session.frames {
        let t = meta.t_ms as f64 / 1000.0;
        if t < from || t > to {
            continue;
        }
        let g = &meta.geometry;
        let (cw, ch) = (g.client.width as u32, g.client.height as u32);
        let Ok(image) = RgbImage::load_png(&session.frame_path(meta)) else { continue };
        let view = FrameView::rgb(&image.data, image.width, image.height);
        let c = g.client_in_frame();
        let shift = if image.width > cw {
            let [(bx, by, bw, bh), (rx, ry, rw, rh)] = shift_probes((image.width, image.height));
            picture_shift(&view.sub(bx, by, bw, bh), &view.sub(rx, ry, rw, rh)).unwrap_or((0, 0))
        } else {
            (0, 0)
        };
        let origin = (c.x - shift.0, c.y - shift.1);
        let reader = CueReader::with_layout(&UiLayout { width: cw, height: ch, dpi: g.dpi });
        let [(p, px, py), (m, mx, my)] = reader.cue_offsets(std::slice::from_ref(&view), origin, radius);
        println!("{},{cw}x{ch},{},{p:.3},{px},{py},{m:.3},{mx},{my}", meta.file, g.dpi);
    }
}
