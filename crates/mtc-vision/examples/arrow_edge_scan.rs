//! Development: every V2 and V3 frame of the multiplayer recordings, the player's arrow as found,
//! listing those fitted from a blob cut by the container's edge (`clipped`), next to the latest
//! whole-arrow sighting of the session: a white icon at the map's edge (spawn areas sit there)
//! taken for the arrow shows as a clipped sighting far from the player.
//!   cargo run --release -p mtc-vision --example arrow_edge_scan
use mtc_dataset::image::RgbImage;
use mtc_dataset::{Session, datasets_dir, sessions};
use mtc_vision::FrameView;
use mtc_vision::map::arrow::find_player_arrow;
use mtc_vision::map::{CueReader, MapTransform, MapView};

fn main() {
    let cues = CueReader::new();
    let (mut frames, mut whole, mut clipped) = (0, 0, 0);
    for info in sessions().expect("sessions") {
        let id = info.info.id.clone();
        let session = Session::open(datasets_dir().join("raw").join(&id)).expect("session");
        let mut last_whole: Option<(f64, f64)> = None;
        for meta in &session.frames {
            let image = RgbImage::load_png(&session.frame_path(meta)).expect("frame");
            let view = FrameView::rgb(&image.data, image.width, image.height);
            let o = meta.geometry.client_in_frame();
            let Some(primary) = cues.read(&view, (o.x, o.y)).primary().filter(|v| *v != MapView::Full) else { continue };
            frames += 1;
            let t = MapTransform::fixed(primary, (o.x, o.y));
            let Some(a) = find_player_arrow(&view, &t) else { continue };
            if a.clipped {
                clipped += 1;
                let away = last_whole.map(|w| format!("{:.3}", (a.map.0 - w.0).hypot(a.map.1 - w.1)));
                println!("{id},{},{primary:?},{:.1},{:.1},{:.2},{},{}", meta.file, a.frame.0, a.frame.1, a.shape, a.mounted, away.unwrap_or_default());
            } else {
                whole += 1;
                last_whole = Some(a.map);
            }
        }
    }
    eprintln!("{frames} V2/V3 frames: whole arrows {whole}, clipped {clipped}");
}
