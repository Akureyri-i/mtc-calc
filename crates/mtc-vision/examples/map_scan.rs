//! View cues and map identification on every recorded frame, as CSV:
//! session,file,t_ms,ping,m_key,scope_black,scope_lit,view,map,score,margin,layout,micros
//!   cargo run --release -p mtc-vision --example map_scan [session-id] > map_scan.csv
//!
//! `view` is the largest view the cues report; the map is identified on that view assuming it is
//! unzoomed (V1 may be zoomed, so its identification is only meaningful at zoom 1).

use std::time::Instant;

use mtc_dataset::image::RgbImage;
use mtc_dataset::sessions;
use mtc_vision::FrameView;
use mtc_vision::map::{CueReader, MapIdentifier, MapReference, MapTransform, MapView, Objective};

fn main() {
    let infos: Vec<_> = mtc_maps::all().iter().collect();
    let references: Vec<MapReference> = infos
        .iter()
        .map(|m| {
            let img = mtc_maps::load_reference(m).expect("cached reference (the HUD downloads it)");
            MapReference::from_rgba(&m.id, m.size_m(), img.width, img.height, &img.data)
        })
        .collect();
    let mut identifier = MapIdentifier::new(&[]);
    // V1/V2 icon scale on a remapped recording of another configuration.
    identifier.set_icon_scale(std::env::var("MTC_ICON_SCALE").ok().map_or(1.0, |v| v.parse().expect("MTC_ICON_SCALE")));
    for (r, m) in references.iter().zip(&infos) {
        let layout: Vec<Objective> = mtc_maps::objectives(&m.id)
            .iter()
            .map(|o| Objective { letter: o.letter.chars().next().expect("letter"), u: o.u, v: o.v })
            .collect();
        identifier.push(r, &layout);
    }
    let cues = CueReader::new();
    let only = std::env::args().nth(1);
    println!("session,file,t_ms,ping,m_key,scope_black,scope_lit,view,map,score,margin,layout,micros");
    for session in sessions().expect("sessions") {
        if only.as_deref().is_some_and(|id| id != session.info.id) {
            continue;
        }
        for meta in &session.frames {
            let image = RgbImage::load_png(&session.frame_path(meta)).expect("frame");
            let view = FrameView::rgb(&image.data, image.width, image.height);
            let origin = meta.geometry.client_in_frame();
            let origin = (origin.x, origin.y);
            let started = Instant::now();
            let c = cues.read(&view, origin);
            let primary = c.primary();
            let found = primary.and_then(|v| identifier.identify(&view, &MapTransform::fixed(v, origin)));
            let micros = started.elapsed().as_micros();
            let view_name = match primary {
                Some(MapView::Full) => "V1",
                Some(MapView::Scope) => "V2",
                Some(MapView::Mini) => "V3",
                None => "",
            };
            let (map, score, margin, layout) = found.map_or((String::new(), String::new(), String::new(), String::new()), |m| {
                (references[m.index].id.clone(), format!("{:.3}", m.score), format!("{:.3}", m.margin), m.by_layout.to_string())
            });
            println!(
                "{},{},{},{:.3},{:.3},{:.2},{:.3},{view_name},{map},{score},{margin},{layout},{micros}",
                session.info.id, meta.file, meta.t_ms, c.ping_score, c.m_key_score, c.scope_black, c.scope_lit
            );
        }
    }
}
