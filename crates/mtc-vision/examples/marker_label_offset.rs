//! Where a marker's label ink sits relative to the marked point, measured on complete labelled
//! sightings of the labelled sessions (V1 at every zoom, V2): offset of the ink bounding box's
//! left and top edges, in frame px.
//!   cargo run --release -p mtc-vision --example marker_label_offset
use std::collections::HashMap;

use mtc_dataset::image::RgbImage;
use mtc_dataset::labels::FrameLabel;
use mtc_dataset::{Session, datasets_dir, labels};
use mtc_vision::FrameView;
use mtc_vision::map::markers::MarkerReader;
use mtc_vision::map::zoom::V1Registrar;
use mtc_vision::map::{CueReader, MapReference, MapTransform, MapView};

const SESSIONS: [&str; 2] = ["20261117-014241", "20261117-160854"];

fn main() {
    let cues = CueReader::new();
    let reader = MarkerReader::new();
    let mut offsets: HashMap<String, Vec<(f64, f64)>> = HashMap::new();
    for id in SESSIONS {
        let session = Session::open(datasets_dir().join("raw").join(id)).expect("session");
        let labels: HashMap<String, FrameLabel> = labels(id).expect("labels").into_iter().map(|l| (l.file.clone(), l)).collect();
        let mut frames: Vec<_> = session.frames.iter().collect();
        frames.sort_by_key(|m| m.t_ms);
        let mut map_id = String::new();
        let mut registrars: HashMap<String, V1Registrar> = HashMap::new();
        for meta in frames {
            if let Some(m) = labels.get(&meta.file).and_then(|l| l.map.as_ref()) {
                map_id = m.id.clone();
            }
            if map_id.is_empty() {
                continue;
            }
            let image = RgbImage::load_png(&session.frame_path(meta)).expect("frame");
            let view = FrameView::rgb(&image.data, image.width, image.height);
            let o = meta.geometry.client_in_frame();
            let origin = (o.x, o.y);
            let Some(primary) = cues.read(&view, origin).primary() else { continue };
            let t = match primary {
                MapView::Full => {
                    let registrar = registrars.entry(map_id.clone()).or_insert_with(|| {
                        let info = mtc_maps::by_id(&map_id).expect("map");
                        let img = mtc_maps::load_reference(info).expect("reference");
                        V1Registrar::new(&MapReference::from_rgba(&info.id, info.size_m(), img.width, img.height, &img.data), info.grid_cell_m)
                    });
                    match registrar.register(&view, origin, None) {
                        Some(r) => r.transform,
                        None => continue,
                    }
                }
                MapView::Scope => MapTransform::fixed(MapView::Scope, origin),
                MapView::Mini => continue,
            };
            for m in reader.read(&view, &t) {
                if !m.complete || m.label.is_none() {
                    continue;
                }
                if let Some((x0, y0, _, _)) = reader.label_ink_box(&view, m.frame) {
                    let key = format!("{primary:?} {:.1}", t.zoom);
                    offsets.entry(key).or_default().push((f64::from(x0) - m.frame.0, f64::from(y0) - m.frame.1));
                }
                // Per glyph: the letter's box, and the first digit's box per digit value.
                let boxes = reader.label_glyph_boxes(&view, m.frame);
                let label = m.label.clone().unwrap_or_default();
                if boxes.len() == label.len() {
                    let a = boxes[0];
                    offsets.entry("glyph A width".into()).or_default().push((f64::from(a.2), f64::from(a.3)));
                    let d = boxes[1];
                    let digit = &label[1..2];
                    offsets
                        .entry(format!("digit {digit} left/top"))
                        .or_default()
                        .push((f64::from(d.0) - m.frame.0, f64::from(d.1) - m.frame.1));
                    offsets.entry(format!("digit {digit} w/h")).or_default().push((f64::from(d.2), f64::from(d.3)));
                }
            }
        }
    }
    // Squad labels (a labelled recording with A, B and C series): the first digit by letter, each
    // frame read with the fixed V1 transform (frame positions need no registration).
    let squad = "20261120-000720";
    if let Ok(session) = Session::open(datasets_dir().join("raw").join(squad)) {
        for label in labels(squad).expect("labels") {
            if label.markers.is_none() {
                continue;
            }
            let meta = session.frames.iter().find(|m| m.file == label.file).expect("frame");
            let image = RgbImage::load_png(&session.frame_path(meta)).expect("frame");
            let view = FrameView::rgb(&image.data, image.width, image.height);
            let o = meta.geometry.client_in_frame();
            for m in reader.read(&view, &MapTransform::fixed(MapView::Full, (o.x, o.y))) {
                let Some(l) = m.label.clone().filter(|_| m.complete && !m.from_label) else { continue };
                let boxes = reader.label_glyph_boxes(&view, m.frame);
                if boxes.len() != l.len() || l.starts_with('?') {
                    continue;
                }
                let d = boxes[1];
                offsets.entry(format!("squad {} first digit left/top", &l[..1])).or_default().push((f64::from(d.0) - m.frame.0, f64::from(d.1) - m.frame.1));
                offsets.entry(format!("squad {} letter left/width", &l[..1])).or_default().push((f64::from(boxes[0].0) - m.frame.0, f64::from(boxes[0].2)));
            }
        }
    }
    let mut all = Vec::new();
    let mut keys: Vec<_> = offsets.keys().cloned().collect();
    keys.sort();
    for k in keys {
        let v = &offsets[&k];
        let n = v.len() as f64;
        let (mx, my) = (v.iter().map(|o| o.0).sum::<f64>() / n, v.iter().map(|o| o.1).sum::<f64>() / n);
        let sx = (v.iter().map(|o| (o.0 - mx).powi(2)).sum::<f64>() / n).sqrt();
        let sy = (v.iter().map(|o| (o.1 - my).powi(2)).sum::<f64>() / n).sqrt();
        println!("{k:>10}: n {:4}  left {mx:+.2} ± {sx:.2}  top {my:+.2} ± {sy:.2}", v.len());
        all.extend_from_slice(v);
    }
    let n = all.len() as f64;
    let (mx, my) = (all.iter().map(|o| o.0).sum::<f64>() / n, all.iter().map(|o| o.1).sum::<f64>() / n);
    let sx = (all.iter().map(|o| (o.0 - mx).powi(2)).sum::<f64>() / n).sqrt();
    let sy = (all.iter().map(|o| (o.1 - my).powi(2)).sum::<f64>() / n).sqrt();
    println!("       all: n {:4}  left {mx:+.2} ± {sx:.2}  top {my:+.2} ± {sy:.2}", all.len());
}
