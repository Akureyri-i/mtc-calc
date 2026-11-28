//! Development: over every recording, how V1's marker hint reads on frames showing the full map
//! (translucent, opaque, undecided), per session and display configuration, and the frames read
//! translucent with the cursor over the map (a marker dragged, if the reading is right).
//!
//!     cargo run --release -p mtc-vision --example hint_dim_scan [session...]

use mtc_dataset::image::RgbImage;
use mtc_dataset::{Session, datasets_dir};
use mtc_vision::FrameView;
use mtc_vision::layout::{Placement, UiLayout};
use mtc_vision::map::{CueReader, MapView};

fn main() {
    let raw = datasets_dir().join("raw");
    let wanted: Vec<String> = std::env::args().skip(1).collect();
    let mut ids: Vec<String> = std::fs::read_dir(&raw).expect("datasets").filter_map(|e| e.ok()).map(|e| e.file_name().to_string_lossy().into_owned()).collect();
    ids.sort();
    for id in ids.iter().filter(|id| wanted.is_empty() || wanted.contains(id)) {
        let Ok(session) = Session::open(&raw.join(id)) else { continue };
        let (mut dim, mut opaque, mut undecided, mut dim_on_map) = (0, 0, 0, Vec::new());
        let mut config = None;
        let mut reader: Option<(UiLayout, CueReader)> = None;
        for meta in &session.frames {
            let g = &meta.geometry;
            let layout = UiLayout { width: g.client.width as u32, height: g.client.height as u32, dpi: g.dpi };
            if reader.as_ref().is_none_or(|(l, _)| *l != layout) {
                reader = Some((layout, CueReader::with_layout(&layout)));
            }
            let (_, r) = reader.as_ref().expect("reader");
            let Ok(image) = RgbImage::load_png(&session.frame_path(meta)) else { continue };
            let c = g.client_in_frame();
            let cues = r.read(&FrameView::rgb(&image.data, image.width, image.height), (c.x, c.y));
            if cues.primary() != Some(MapView::Full) {
                continue;
            }
            config.get_or_insert(format!("{}x{} {}%", layout.width, layout.height, layout.dpi * 100 / 96));
            match cues.hint_dimmed {
                Some(true) => {
                    dim += 1;
                    let (fl, ft, fs) = Placement::new((c.x, c.y), layout).map_container(MapView::Full);
                    if let Some((x, y)) = meta.cursor {
                        let (x, y) = (f64::from(x) - f64::from(c.x), f64::from(y) - f64::from(c.y));
                        if x >= fl && x < fl + fs && y >= ft && y < ft + fs {
                            dim_on_map.push(meta.t_ms);
                        }
                    }
                }
                Some(false) => opaque += 1,
                None => undecided += 1,
            }
        }
        if dim + opaque + undecided > 0 {
            println!("{id} {}: full map {} · translucent {dim} (cursor on the map {}) · opaque {opaque} · undecided {undecided}", config.unwrap_or_default(), dim + opaque + undecided, dim_on_map.len());
            if !dim_on_map.is_empty() {
                println!("  translucent with the cursor on the map at ms {:?}", &dim_on_map[..dim_on_map.len().min(12)]);
            }
        }
    }
}
