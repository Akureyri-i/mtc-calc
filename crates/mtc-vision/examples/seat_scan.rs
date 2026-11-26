//! The seat menu read on every frame of a session, one line per frame in time order:
//! `t_ms,file,layout,rows` with each row as kind/taken/mine (`-` when no menu is read).
//!   cargo run --release -p mtc-vision --example seat_scan -- <session>
use mtc_dataset::image::RgbImage;
use mtc_dataset::{Session, datasets_dir};
use mtc_vision::FrameView;
use mtc_vision::seats::SeatReader;

fn main() {
    let id = std::env::args().nth(1).expect("session");
    let session = Session::open(datasets_dir().join("raw").join(&id)).expect("session");
    let reader = SeatReader::new();
    let mut frames: Vec<_> = session.frames.iter().collect();
    frames.sort_by_key(|m| m.t_ms);
    println!("t_ms,file,layout,rows");
    for meta in frames {
        let image = RgbImage::load_png(&session.frame_path(meta)).expect("frame");
        let c = meta.geometry.client_in_frame();
        let view = FrameView::rgb(&image.data, image.width, image.height);
        match reader.read(&view, (c.x, c.y, c.width as u32, c.height as u32)) {
            Some(menu) => {
                let rows: Vec<String> = menu
                    .rows
                    .iter()
                    .map(|r| {
                        format!(
                            "{:?}/{}/{}",
                            r.kind,
                            if r.taken { "taken" } else { "empty" },
                            if r.mine { "mine" } else { "-" }
                        )
                    })
                    .collect();
                println!(
                    "{},{},{:?},{}",
                    meta.t_ms,
                    meta.file,
                    menu.layout,
                    rows.join(" ")
                );
            }
            None => println!("{},{},-,", meta.t_ms, meta.file),
        }
    }
}
