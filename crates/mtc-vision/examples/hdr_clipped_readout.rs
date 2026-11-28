//! What an 8-bit capture of an HDR display does to the angle readout: every labelled readout frame
//! is brightened as Windows brightens SDR content on an HDR desktop (linear light times the SDR
//! white level over 80 nits) and clipped to 8 bits, then read.
//!   cargo run --release -p mtc-vision --example hdr_clipped_readout
use mtc_dataset::eval::labelled_frames;
use mtc_vision::FrameView;
use mtc_vision::aim::{AimReader, AimResult};

fn srgb_decode(v: u8) -> f32 {
    let v = f32::from(v) / 255.0;
    if v <= 0.040_45 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
}

fn srgb_encode(l: f32) -> u8 {
    let l = l.clamp(0.0, 1.0);
    let v = if l <= 0.003_130_8 { 12.92 * l } else { 1.055 * l.powf(1.0 / 2.4) - 0.055 };
    (v * 255.0).round() as u8
}

fn main() {
    let frames = labelled_frames().expect("dataset");
    let frames: Vec<_> = frames.iter().filter(|f| f.label.aim.is_some()).collect();
    for white in [1.0f32, 178.0 / 80.0, 280.0 / 80.0, 480.0 / 80.0] {
        let table: Vec<u8> = (0..=255u8).map(|v| srgb_encode(srgb_decode(v) * white)).collect();
        let mut reader = AimReader::new();
        let (mut read, mut right) = (0, 0);
        for frame in &frames {
            let truth = frame.label.aim.as_ref().expect("filtered");
            let image = frame.image().expect("frame image");
            let data: Vec<u8> = image.data.iter().map(|&v| table[usize::from(v)]).collect();
            let view = FrameView::rgb(&data, image.width, image.height);
            let origin = frame.meta.geometry.client_in_frame();
            if let AimResult::Read(r) = reader.read(&view, (origin.x, origin.y)) {
                read += 1;
                right += usize::from(r.elevation_text == truth.elevation && r.azimuth_text == truth.azimuth);
            }
        }
        println!("white {:.0} nits: readout read {read}/{} ({right} right)", white * 80.0, frames.len());
    }
}
