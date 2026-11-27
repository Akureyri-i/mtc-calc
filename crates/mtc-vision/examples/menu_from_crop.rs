//! Development: reads the loading menu from a crop of the loading area (a problem report's
//! `loading.png`, which covers `LOADING_REGION` of the reference geometry at any scale). The crop
//! is resampled (bilinear) to the reference size and placed where the region sits in a reference
//! frame, as the HUD maps other configurations, then `read_menu` reads it.
//!   cargo run --release -p mtc-vision --example menu_from_crop -- <loading.png> [...]
use mtc_dataset::image::RgbImage;
use mtc_vision::FrameView;
use mtc_vision::loading::{LOADING_REGION, read_menu};

fn main() {
    let (lx, ly, lw, lh) = LOADING_REGION;
    let (fw, fh) = (2560u32, 1334u32);
    for path in std::env::args().skip(1) {
        let crop = RgbImage::load_png(std::path::Path::new(&path)).expect("png");
        let mut frame = RgbImage::new(fw, fh);
        let (sx, sy) = (f64::from(crop.width) / f64::from(lw), f64::from(crop.height) / f64::from(lh));
        for y in 0..lh {
            for x in 0..lw {
                let (fx, fy) = ((f64::from(x) + 0.5) * sx - 0.5, (f64::from(y) + 0.5) * sy - 0.5);
                let (x0, y0) = (fx.floor().max(0.0) as u32, fy.floor().max(0.0) as u32);
                let (x1, y1) = ((x0 + 1).min(crop.width - 1), (y0 + 1).min(crop.height - 1));
                let (tx, ty) = (fx - f64::from(x0), fy - f64::from(y0));
                let o = (((ly + y as i32) as u32 * fw + (lx + x as i32) as u32) * 3) as usize;
                for c in 0..3 {
                    let p = |px: u32, py: u32| f64::from(crop.data[((py * crop.width + px) * 3) as usize + c]);
                    let top = p(x0, y0) * (1.0 - tx) + p(x1, y0) * tx;
                    let bottom = p(x0, y1) * (1.0 - tx) + p(x1, y1) * tx;
                    frame.data[o + c] = (top * (1.0 - ty) + bottom * ty).round() as u8;
                }
            }
        }
        let view = FrameView::rgb(&frame.data, fw, fh);
        match read_menu(&view, (0, 0)) {
            Some(menu) => {
                let charges: Vec<String> = menu.cards.iter().map(|c| format!("{:?}", c.charge)).collect();
                println!("{path}: {:?} seat, {} cards: {}", menu.seat, menu.cards.len(), charges.join(" · "));
            }
            None => println!("{path}: no menu found"),
        }
    }
}
