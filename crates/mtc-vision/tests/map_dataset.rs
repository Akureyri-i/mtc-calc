//! Map views, map identification and full-map zoom registration against the labels.
//!
//! Needs the reference cache (filled by the HUD's download).

use std::collections::HashMap;

use mtc_dataset::eval::{LabelledFrame, Tally, labelled_frames};
use mtc_dataset::image::RgbImage;
use mtc_dataset::labels::MapView as LabelView;
use mtc_dataset::{Session, datasets_dir};
use mtc_vision::{FrameView, PixelFormat};
use mtc_vision::map::zoom::V1Registrar;
use mtc_vision::map::{CueReader, MapIdentifier, MapReference, MapTransform, MapView, Objective};

/// Origin tolerance in map units (≈1.4 m on Fulvia Gap, 0.16 px at 3×).
const ORIGIN_TOLERANCE: f64 = 0.0005;

fn references() -> Vec<MapReference> {
    mtc_maps::all()
        .iter()
        .map(|m| {
            let img = mtc_maps::load_reference(m).expect("cached reference");
            MapReference::from_rgba(&m.id, m.size_m(), img.width, img.height, &img.data)
        })
        .collect()
}

/// Identifier over every map with its objective table (`data/objectives.json`).
fn identifier(refs: &[MapReference]) -> MapIdentifier {
    let mut identifier = MapIdentifier::new(&[]);
    for (r, m) in refs.iter().zip(mtc_maps::all()) {
        let layout: Vec<Objective> = mtc_maps::objectives(&m.id)
            .iter()
            .map(|o| Objective { letter: o.letter.chars().next().expect("letter"), u: o.u, v: o.v })
            .collect();
        identifier.push(r, &layout);
    }
    identifier
}

fn view_of(label: LabelView) -> MapView {
    match label {
        LabelView::V1 => MapView::Full,
        LabelView::V2 => MapView::Scope,
        LabelView::V3 => MapView::Mini,
    }
}

fn client_origin(frame: &LabelledFrame) -> (i32, i32) {
    let o = frame.meta.geometry.client_in_frame();
    (o.x, o.y)
}

#[test]
fn views_and_maps_match_labels() {
    if !mtc_dataset::recordings_present() {
        eprintln!("skipped: the recordings are not on this machine");
        return;
    }
    let refs = references();
    let identifier = identifier(&refs);
    let cues = CueReader::new();
    let mut views = Tally::new("map view cues");
    let mut ids = Tally::new("map id (single frame, confident and right)");
    for frame in labelled_frames().expect("dataset") {
        // Labelled as showing no map (black and loading screens): no view.
        if frame.label.map.is_none() && frame.label.scene.as_ref().is_some_and(|s| s.map_view.is_none()) {
            let image = frame.image().expect("frame image");
            let view = FrameView::rgb(&image.data, image.width, image.height);
            let primary = views.time(|| cues.read(&view, client_origin(&frame)).primary());
            views.check(&frame.id(), primary.is_none(), || format!("cues say {primary:?}, label: no map"));
            continue;
        }
        let Some(map) = &frame.label.map else { continue };
        let image = frame.image().expect("frame image");
        let view = FrameView::rgb(&image.data, image.width, image.height);
        let origin = client_origin(&frame);
        let expected = view_of(map.view);
        let primary = views.time(|| cues.read(&view, origin).primary());
        views.check(&frame.id(), primary == Some(expected), || format!("cues say {primary:?}, label {expected:?}"));
        if expected == MapView::Full && map.zoom != Some(1.0) {
            continue;
        }
        // Identify on exactly the container rectangle the live HUD reads back, not the whole frame.
        let (rx, ry, rw, rh) = expected.frame_rect(origin);
        let start = (ry as usize * image.width as usize + rx as usize) * 3;
        let region = FrameView {
            data: &image.data[start..],
            format: PixelFormat::Rgb,
            origin: (rx, ry),
            width: rw as u32,
            height: rh as u32,
            stride: image.width as usize * 3,
        };
        let found = ids.time(|| identifier.identify(&region, &MapTransform::fixed(expected, origin)));
        let ok = found.is_some_and(|m| m.confident_on(expected) && refs[m.index].id == map.id);
        ids.check(&frame.id(), ok, || {
            found.map_or("not covered".into(), |m| format!("{} score {:.3} margin {:.3}, label {}", refs[m.index].id, m.score, m.margin, map.id))
        });
    }
    views.require(1.0);
    ids.require(1.0);
}

#[test]
fn unknown_map_is_never_identified() {
    if !mtc_dataset::recordings_present() {
        eprintln!("skipped: the recordings are not on this machine");
        return;
    }
    let refs = references();
    let identifier = identifier(&refs);
    let cues = CueReader::new();
    let mut tally = Tally::new("debug map (not in the database): no confident id");
    for id in ["20261117-125220", "20261117-135421"] {
        let session = Session::open(datasets_dir().join("raw").join(id)).expect("session");
        for meta in session.frames.iter().step_by(4) {
            let image = RgbImage::load_png(&session.frame_path(meta)).expect("frame");
            let view = FrameView::rgb(&image.data, image.width, image.height);
            let o = meta.geometry.client_in_frame();
            let Some(primary) = cues.read(&view, (o.x, o.y)).primary() else { continue };
            let found = tally.time(|| identifier.identify(&view, &MapTransform::fixed(primary, (o.x, o.y))));
            let ok = found.is_none_or(|m| !m.confident_on(primary));
            tally.check(&format!("{id}/{}", meta.file), ok, || {
                let m = found.expect("failed only when found");
                format!("{} score {:.3} margin {:.3}", refs[m.index].id, m.score, m.margin)
            });
        }
    }
    tally.require(1.0);
}

/// Runs the registrar over every full-map frame of each session in time order (so tracking between
/// frames is exercised) and checks the labelled ones.
#[test]
fn v1_registration_matches_labels() {
    if !mtc_dataset::recordings_present() {
        eprintln!("skipped: the recordings are not on this machine");
        return;
    }
    let refs = references();
    let cues = CueReader::new();
    let mut labelled: HashMap<String, Vec<LabelledFrame>> = HashMap::new();
    for frame in labelled_frames().expect("dataset") {
        if frame.label.map.as_ref().is_some_and(|m| m.view == LabelView::V1 && m.zoom.is_some()) {
            labelled.entry(frame.session.clone()).or_default().push(frame);
        }
    }
    let mut tally = Tally::new("V1 registration: zoom exact, origin within 0.0005");
    for (session_id, frames) in labelled {
        let session = Session::open(datasets_dir().join("raw").join(&session_id)).expect("session");
        let by_file: HashMap<&str, &LabelledFrame> = frames.iter().map(|f| (f.meta.file.as_str(), f)).collect();
        let mut registrars: HashMap<String, V1Registrar> = HashMap::new();
        let map_of_session = frames[0].label.map.as_ref().map(|m| m.id.clone()).expect("map label");
        let mut metas: Vec<_> = session.frames.iter().collect();
        metas.sort_by_key(|m| m.t_ms);
        for meta in metas {
            let label = by_file.get(meta.file.as_str()).and_then(|f| f.label.map.as_ref());
            // Sessions with one map are registered frame by frame; others only on labelled frames.
            let map_id = match label {
                Some(m) => m.id.clone(),
                None if frames.iter().all(|f| f.label.map.as_ref().is_some_and(|m| m.id == map_of_session)) => map_of_session.clone(),
                None => continue,
            };
            let image = RgbImage::load_png(&session.frame_path(meta)).expect("frame");
            let view = FrameView::rgb(&image.data, image.width, image.height);
            let o = meta.geometry.client_in_frame();
            if !cues.read(&view, (o.x, o.y)).shows(MapView::Full) {
                continue;
            }
            let registrar = registrars.entry(map_id.clone()).or_insert_with(|| {
                V1Registrar::new(
                    refs.iter().find(|r| r.id == map_id).expect("reference"),
                    mtc_maps::by_id(&map_id).and_then(|m| m.grid_cell_m),
                )
            });
            let cursor = meta.cursor.map(|(x, y)| (f64::from(x) + 0.5, f64::from(y) + 0.5));
            let result = tally.time(|| registrar.register(&view, (o.x, o.y), cursor));
            let Some(truth) = label else { continue };
            let zoom = truth.zoom.expect("filtered");
            let ok = result.is_some_and(|r| {
                (r.transform.zoom - zoom).abs() < 1e-6
                    && truth.origin.is_none_or(|(u, v)| {
                        (r.transform.origin.0 - u).abs() <= ORIGIN_TOLERANCE && (r.transform.origin.1 - v).abs() <= ORIGIN_TOLERANCE
                    })
            });
            tally.check(&format!("{session_id}/{}", meta.file), ok, || {
                format!(
                    "got {:?}, label zoom {zoom} origin {:?}",
                    result.map(|r| (r.method, r.transform.zoom, r.transform.origin)),
                    truth.origin
                )
            });
        }
    }
    tally.require(1.0);
}

/// The search around a canvas prior (`register_with_prior`, as the HUD calls it with the canvas
/// the wheel and drags led to): every labelled zoomed full-map frame, registered afresh (no
/// previous registration) with the labelled view moved 15 screen px as the prior, reads the
/// labelled view as the plain search does, at a fraction of its time.
#[test]
fn v1_registration_from_a_canvas_prior() {
    if !mtc_dataset::recordings_present() {
        eprintln!("skipped: the recordings are not on this machine");
        return;
    }
    let refs = references();
    let mut plain = Tally::new("V1 registration afresh, whole search");
    let mut prior = Tally::new("V1 registration afresh, around a prior 15 px off");
    let mut by_map: HashMap<String, V1Registrar> = HashMap::new();
    for frame in labelled_frames().expect("dataset") {
        let Some(truth) = frame.label.map.as_ref().filter(|m| m.view == LabelView::V1 && m.zoom.is_some_and(|z| z > 1.0 + 1e-6) && m.origin.is_some()) else { continue };
        let (zoom, origin) = (truth.zoom.expect("filtered"), truth.origin.expect("filtered"));
        let image = frame.image().expect("frame");
        let view = FrameView::rgb(&image.data, image.width, image.height);
        let o = frame.meta.geometry.client_in_frame();
        let registrar = by_map.entry(truth.id.clone()).or_insert_with(|| {
            V1Registrar::new(refs.iter().find(|r| r.id == truth.id).expect("reference"), mtc_maps::by_id(&truth.id).and_then(|m| m.grid_cell_m))
        });
        let cursor = frame.meta.cursor.map(|(x, y)| (f64::from(x) + 0.5, f64::from(y) + 0.5));
        let right = |r: Option<mtc_vision::map::zoom::Registration>| {
            r.is_some_and(|r| (r.transform.zoom - zoom).abs() < 1e-6 && (r.transform.origin.0 - origin.0).abs() <= ORIGIN_TOLERANCE && (r.transform.origin.1 - origin.1).abs() <= ORIGIN_TOLERANCE)
        };
        registrar.forget();
        let r = plain.time(|| registrar.register(&view, (o.x, o.y), cursor));
        plain.check(&frame.id(), right(r), || format!("{:?}", r.map(|r| (r.method, r.transform.zoom, r.transform.origin))));
        let base = MapTransform::fixed(MapView::Full, (o.x, o.y));
        let shift = 15.0 / (base.size * zoom);
        let p = MapTransform::zoomed(MapView::Full, (o.x, o.y), zoom, (origin.0 + shift, origin.1 - shift));
        registrar.forget();
        let r = prior.time(|| registrar.register_with_prior(&view, (o.x, o.y), cursor, Some(p)));
        prior.check(&frame.id(), right(r), || format!("{:?}", r.map(|r| (r.method, r.transform.zoom, r.transform.origin))));
    }
    plain.require(0.0);
    prior.require(plain.rate());
}
