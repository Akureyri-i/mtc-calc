//! V1 registration over a session in time order, as CSV:
//! file,t_ms,v1,method,zoom,u0,v0,score,grid_zoom,grid_period,phase_x,phase_y,micros
//!   cargo run --release -p mtc-vision --example zoom_scan <session-id> <map-id> > zoom.csv

use std::time::Instant;

use mtc_dataset::image::RgbImage;
use mtc_dataset::{Session, datasets_dir};
use mtc_vision::FrameView;
use mtc_vision::map::zoom::{Method, V1Registrar};
use mtc_vision::map::{CueReader, MapReference, MapView};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let session = Session::open(datasets_dir().join("raw").join(&args[1])).expect("session");
    let info = mtc_maps::by_id(&args[2]).expect("map id");
    let img = mtc_maps::load_reference(info).expect("reference");
    let reference = MapReference::from_rgba(&info.id, info.size_m(), img.width, img.height, &img.data);
    let mut registrar = V1Registrar::new(&reference, info.grid_cell_m);
    let cues = CueReader::new();
    let mut frames: Vec<_> = session.frames.iter().collect();
    frames.sort_by_key(|m| m.t_ms);
    println!("file,t_ms,v1,method,zoom,u0,v0,score,grid_zoom,grid_period,phase_x,phase_y,micros");
    for meta in frames {
        let image = RgbImage::load_png(&session.frame_path(meta)).expect("frame");
        let view = FrameView::rgb(&image.data, image.width, image.height);
        let origin = meta.geometry.client_in_frame();
        let origin = (origin.x, origin.y);
        if !cues.read(&view, origin).shows(MapView::Full) {
            println!("{},{},0,,,,,,,,,,", meta.file, meta.t_ms);
            continue;
        }
        let cursor = meta.cursor.map(|(x, y)| (f64::from(x) + 0.5, f64::from(y) + 0.5));
        let started = Instant::now();
        let r = registrar.register(&view, origin, cursor);
        let micros = started.elapsed().as_micros();
        match r {
            None => println!("{},{},1,none,,,,,,,,,{micros}", meta.file, meta.t_ms),
            Some(r) => {
                let method = match r.method {
                    Method::Reference => "ref",
                    Method::GridLabels => "grid_labels",
                    Method::GridReference => "grid_ref",
                    Method::GridTracked => "grid_track",
                };
                let t = r.transform;
                let g = r.grid.map_or(String::from(",,,"), |g| {
                    format!("{:.1},{:.2},{:.2},{:.2}", g.zoom, g.period, g.phase.0, g.phase.1)
                });
                println!(
                    "{},{},1,{method},{:.1},{:.5},{:.5},{:.3},{g},{micros}",
                    meta.file, meta.t_ms, t.zoom, t.origin.0, t.origin.1, r.score
                );
            }
        }
    }
}
