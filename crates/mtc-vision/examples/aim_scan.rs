//! Reads the angle readout on every recorded frame and prints CSV:
//! session,file,t_ms,status,elevation,azimuth,cost,micros
//!   cargo run --release -p mtc-vision --example aim_scan [session-id] > aim_scan.csv
//! `MTC_AIM_SCALE="lx,ly,dx,dy"` gives the text scale of the recording's configuration (label x, y,
//! digits x, y; default the reference).

use std::time::Instant;

use mtc_dataset::image::RgbImage;
use mtc_dataset::sessions;
use mtc_vision::FrameView;
use mtc_vision::aim::{AimReader, AimResult, TextScale};

fn main() {
    let scale = std::env::var("MTC_AIM_SCALE").ok().map_or(TextScale::REFERENCE, |v| v.parse().expect("MTC_AIM_SCALE"));
    let mut reader = AimReader::with_scale(scale);
    println!("session,file,t_ms,status,elevation,azimuth,cost,micros");
    let only = std::env::args().nth(1);
    for session in sessions().expect("sessions") {
        if only.as_deref().is_some_and(|id| id != session.info.id) {
            continue;
        }
        for meta in &session.frames {
            let image = RgbImage::load_png(&session.frame_path(meta)).expect("frame");
            let view = FrameView::rgb(&image.data, image.width, image.height);
            let origin = meta.geometry.client_in_frame();
            let started = Instant::now();
            let result = reader.read(&view, (origin.x, origin.y));
            let micros = started.elapsed().as_micros();
            let (status, e, a, c) = match result {
                AimResult::Absent => ("absent", String::new(), String::new(), 0),
                AimResult::Unreadable { .. } => ("unreadable", String::new(), String::new(), 0),
                AimResult::Read(r) => ("read", r.elevation_text, r.azimuth_text, r.cost),
            };
            println!("{},{},{},{status},{e},{a},{c},{micros}", session.info.id, meta.file, meta.t_ms);
        }
    }
}
