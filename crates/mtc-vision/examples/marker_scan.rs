//! Markers in every frame of a session where a map view is shown, as CSV:
//! file,t_ms,view,zoom,count,markers (label@u:v[*incomplete] ...),arrow (u:v:heading:mounted),read_us,diamonds_us
//!   cargo run --release -p mtc-vision --example marker_scan <session-id> <map-id> [from-ms to-ms]
use mtc_dataset::image::RgbImage;
use mtc_dataset::{Session, datasets_dir};
use mtc_vision::FrameView;
use mtc_vision::map::arrow::find_player_arrow_at;
use mtc_vision::map::markers::MarkerReader;
use mtc_vision::map::zoom::V1Registrar;
use mtc_vision::map::{CueReader, MapReference, MapTransform, MapView};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let session = Session::open(datasets_dir().join("raw").join(&args[1])).expect("session");
    let info = mtc_maps::by_id(&args[2]).expect("map id");
    let img = mtc_maps::load_reference(info).expect("reference");
    let reference = MapReference::from_rgba(&info.id, info.size_m(), img.width, img.height, &img.data);
    let mut registrar = V1Registrar::new(&reference, info.grid_cell_m);
    let cues = CueReader::new();
    // V1/V2 icon scale on a remapped recording of another configuration.
    let icon_scale: f64 = std::env::var("MTC_ICON_SCALE").ok().map_or(1.0, |v| v.parse().expect("MTC_ICON_SCALE"));
    let markers = MarkerReader::with_icon_scale(icon_scale);
    let mut frames: Vec<_> = session.frames.iter().collect();
    frames.sort_by_key(|m| m.t_ms);
    let window = (args.get(3).and_then(|v| v.parse::<u64>().ok()), args.get(4).and_then(|v| v.parse::<u64>().ok()));
    if let (Some(from), Some(to)) = window {
        frames.retain(|m| (from..=to).contains(&m.t_ms));
    }
    println!("file,t_ms,view,zoom,count,markers,arrow,read_us,diamonds_us");
    for meta in frames {
        let image = RgbImage::load_png(&session.frame_path(meta)).expect("frame");
        let view = FrameView::rgb(&image.data, image.width, image.height);
        let o = meta.geometry.client_in_frame();
        let origin = (o.x, o.y);
        let Some(primary) = cues.read(&view, origin).primary() else { continue };
        let t = match primary {
            MapView::Full => {
                let cursor = meta.cursor.map(|(x, y)| (f64::from(x) + 0.5, f64::from(y) + 0.5));
                match registrar.register(&view, origin, cursor) {
                    Some(r) => r.transform,
                    None => continue,
                }
            }
            v => MapTransform::fixed(v, origin),
        };
        let started = std::time::Instant::now();
        let found = markers.read(&view, &t);
        let read_us = started.elapsed().as_micros();
        let started = std::time::Instant::now();
        let _ = markers.read_diamonds(&view, &t);
        let diamonds_us = started.elapsed().as_micros();
        let name = match primary {
            MapView::Full => "V1",
            MapView::Scope => "V2",
            MapView::Mini => "V3",
        };
        let list: Vec<String> = found
            .iter()
            .map(|m| {
                format!(
                    "{}@{:.5}:{:.5}{} (frame {:.0},{:.0})",
                    m.label.as_deref().unwrap_or("?"),
                    m.map.0,
                    m.map.1,
                    if m.complete { "" } else { "*" },
                    m.frame.0,
                    m.frame.1
                )
            })
            .collect();
        // The minimap's frame scales with the HUD: its icons keep their size.
        let arrow = find_player_arrow_at(&view, &t, if t.view == MapView::Mini { 1.0 } else { icon_scale }).map_or(String::new(), |a| {
            format!("{:.5}:{:.5}:{:.0}:{}", a.map.0, a.map.1, a.heading_deg, u8::from(a.mounted))
        });
        println!("{},{},{name},{:.1},{},{},{arrow},{read_us},{diamonds_us}", meta.file, meta.t_ms, t.zoom, found.len(), list.join(" "));
    }
}
