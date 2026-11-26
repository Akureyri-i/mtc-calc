//! Target bookkeeping over a session in time order; the map comes from the session's labels
//! (each map segment gets its own book). Prints events and the final targets per segment.
//!   cargo run --release -p mtc-vision --example target_scan <session-id>
use std::collections::HashMap;

use mtc_dataset::image::RgbImage;
use mtc_dataset::{Session, datasets_dir, labels};
use mtc_vision::FrameView;
use mtc_vision::map::markers::{MarkerReader, minimap_spot_clear};
use mtc_vision::map::targets::{MapFrame, TargetBook};
use mtc_vision::map::zoom::V1Registrar;
use mtc_vision::map::{CueReader, MapReference, MapTransform, MapView};

fn main() {
    let id = std::env::args().nth(1).expect("session id");
    let session = Session::open(datasets_dir().join("raw").join(&id)).expect("session");
    let labelled: HashMap<String, String> =
        labels(&id).expect("labels").into_iter().filter_map(|l| l.map.map(|m| (l.file, m.id))).collect();
    let cues = CueReader::new();
    let markers = MarkerReader::new();
    let mut registrars: HashMap<String, V1Registrar> = HashMap::new();
    let mut book = TargetBook::default();
    let mut current: Option<String> = None;
    let mut frames: Vec<_> = session.frames.iter().collect();
    frames.sort_by_key(|m| m.t_ms);
    // Map of every frame: the latest labelled map (the first one before any label).
    let first = frames.iter().find_map(|m| labelled.get(&m.file)).cloned().expect("a map label");
    let mut last = first;
    let map_of: Vec<String> = frames
        .iter()
        .map(|m| {
            if let Some(id) = labelled.get(&m.file) {
                last = id.clone();
            }
            last.clone()
        })
        .collect();
    for (meta, map_id) in frames.into_iter().zip(&map_of) {
        if current.as_ref() != Some(map_id) {
            if let Some(prev) = &current {
                println!("[{prev}] final: {:?}", book.targets().iter().map(|t| t.label.as_str()).collect::<Vec<_>>());
            }
            book.reset();
            book.set_objectives(mtc_maps::objectives(map_id).iter().map(|o| (o.u, o.v)).collect());
            current = Some(map_id.clone());
        }
        let image = RgbImage::load_png(&session.frame_path(meta)).expect("frame");
        let view = FrameView::rgb(&image.data, image.width, image.height);
        let o = meta.geometry.client_in_frame();
        let origin = (o.x, o.y);
        let c = cues.read(&view, origin);
        let Some(primary) = c.primary() else { continue };
        let cursor = meta.cursor.map(|(x, y)| (f64::from(x) + 0.5, f64::from(y) + 0.5));
        let transform = match primary {
            MapView::Full => {
                let registrar = registrars.entry(map_id.clone()).or_insert_with(|| {
                    let info = mtc_maps::by_id(map_id).expect("map");
                    let img = mtc_maps::load_reference(info).expect("reference");
                    V1Registrar::new(&MapReference::from_rgba(&info.id, info.size_m(), img.width, img.height, &img.data), info.grid_cell_m)
                });
                match registrar.register(&view, origin, cursor) {
                    Some(r) => r.transform,
                    None => continue,
                }
            }
            v => MapTransform::fixed(v, origin),
        };
        let found = markers.read(&view, &transform);
        let spot = |m: (f64, f64)| minimap_spot_clear(&view, &transform, m);
        let events = book.observe(&MapFrame {
            t_ms: meta.t_ms,
            transform,
            markers: &found,
            remove_hint: c.remove_hint(), dragging: false, right_click: None,
            cursor,
            client_origin: origin,
            spot_clear: (transform.view == MapView::Mini).then_some(&spot as &dyn Fn((f64, f64)) -> bool),
            placed: None,
        });
        for e in events {
            println!("[{map_id}] {:7.1}s {e:?}", meta.t_ms as f64 / 1000.0);
        }
    }
    if let Some(prev) = &current {
        println!("[{prev}] final: {:?}", book.targets().iter().map(|t| t.label.as_str()).collect::<Vec<_>>());
    }
}
