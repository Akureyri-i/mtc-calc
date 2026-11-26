//! Thumbnail scores on every unzoomed map frame of the multiplayer recordings (teammates' vehicle
//! icons all over the map), one CSV row a frame: the right map's score, the best other map's
//! score, and whether the objective layout alone decides. The labelled unzoomed frames of the
//! other (private-server) sessions follow as the uncrowded comparison.
//!   cargo run --release -p mtc-vision --example ident_public -- <datasets dir> <out.csv>
use std::io::Write;
use std::path::PathBuf;

use mtc_dataset::Session;
use mtc_dataset::eval::labelled_frames;
use mtc_dataset::image::RgbImage;
use mtc_dataset::labels::MapView as LabelView;
use mtc_vision::FrameView;
use mtc_vision::map::zoom::V1Registrar;
use mtc_vision::map::{CueReader, MapIdentifier, MapReference, MapTransform, MapView, Objective};

/// Multiplayer sessions and their map (the labels' map for the whole match).
const SESSIONS: [(&str, &str); 3] = [("20261117-222558", "fulvia_gap"), ("20261117-221940", "muddy_fields"), ("20261117-live-probe", "sokolovka")];

fn main() {
    let mut args = std::env::args().skip(1);
    let root = PathBuf::from(args.next().expect("datasets dir"));
    let out_path = PathBuf::from(args.next().expect("out csv"));
    let maps = mtc_maps::all();
    let mut identifier = MapIdentifier::new(&[]);
    for m in maps {
        let img = mtc_maps::load_reference(m).expect("cached reference");
        let layout: Vec<Objective> = mtc_maps::objectives(&m.id).iter().map(|o| Objective { letter: o.letter.chars().next().expect("letter"), u: o.u, v: o.v }).collect();
        identifier.push(&MapReference::from_rgba(&m.id, m.size_m(), img.width, img.height, &img.data), &layout);
    }
    let cues = CueReader::new();
    let mut out = std::fs::File::create(&out_path).expect("out");
    writeln!(out, "session,file,view,zoom,truth,truth_score,other_score,other,layout").expect("write");
    for (id, truth_id) in SESSIONS {
        let truth = maps.iter().position(|m| m.id == truth_id).expect("map");
        let info = &maps[truth];
        let img = mtc_maps::load_reference(info).expect("reference");
        let mut registrar = V1Registrar::new(&MapReference::from_rgba(&info.id, info.size_m(), img.width, img.height, &img.data), info.grid_cell_m);
        let session = Session::open(root.join("raw").join(id)).expect("session");
        let mut rows = 0;
        for meta in &session.frames {
            let image = RgbImage::load_png(&session.frame_path(meta)).expect("frame");
            let view = FrameView::rgb(&image.data, image.width, image.height);
            let o = meta.geometry.client_in_frame();
            let origin = (o.x, o.y);
            let Some(primary) = cues.read(&view, origin).primary() else { continue };
            let fixed = MapTransform::fixed(primary, origin);
            let zoom = if primary == MapView::Full {
                match registrar.register(&view, origin, None) {
                    Some(r) => r.transform.zoom,
                    None => continue,
                }
            } else {
                1.0
            };
            if (zoom - 1.0).abs() > 0.02 {
                // Zoomed V1: listed (to check the registrar's zoom by eye), not scored.
                writeln!(out, "{id},{},{primary:?},{zoom:.3},{truth_id},,,,", meta.file).expect("write");
                continue;
            }
            let Some(scores) = identifier.scores(&view, &fixed) else { continue };
            let (other, other_score) = scores.iter().enumerate().filter(|(i, _)| *i != truth).fold((0, f32::MIN), |b, (i, &s)| if s > b.1 { (i, s) } else { b });
            let layout = identifier.identify_by_layout(&view, &fixed).map_or("none".to_owned(), |m| maps[m.index].id.clone());
            writeln!(out, "{id},{},{primary:?},{zoom:.3},{truth_id},{:.4},{other_score:.4},{},{layout}", meta.file, scores[truth], maps[other].id).expect("write");
            rows += 1;
        }
        eprintln!("{id}: {rows} unzoomed map frames");
    }
    let mut rows = 0;
    for frame in labelled_frames().expect("dataset") {
        let Some(map) = &frame.label.map else { continue };
        if SESSIONS.iter().any(|(id, _)| *id == frame.session) {
            continue;
        }
        let view = match map.view {
            LabelView::V1 if map.zoom == Some(1.0) => MapView::Full,
            LabelView::V1 => continue,
            LabelView::V2 => MapView::Scope,
            LabelView::V3 => MapView::Mini,
        };
        let truth = maps.iter().position(|m| m.id == map.id).expect("map");
        let image = frame.image().expect("frame image");
        let o = frame.meta.geometry.client_in_frame();
        let fixed = MapTransform::fixed(view, (o.x, o.y));
        let region = FrameView::rgb(&image.data, image.width, image.height);
        let Some(scores) = identifier.scores(&region, &fixed) else { continue };
        let (other, other_score) = scores.iter().enumerate().filter(|(i, _)| *i != truth).fold((0, f32::MIN), |b, (i, &s)| if s > b.1 { (i, s) } else { b });
        let layout = identifier.identify_by_layout(&region, &fixed).map_or("none".to_owned(), |m| maps[m.index].id.clone());
        writeln!(out, "private:{},{},{view:?},1.000,{},{:.4},{other_score:.4},{},{layout}", frame.session, frame.meta.file, map.id, scores[truth], maps[other].id).expect("write");
        rows += 1;
    }
    eprintln!("private labelled: {rows} unzoomed map frames");
}
