//! Markers, the player arrow and target bookkeeping, run over whole sessions in time order
//! (full-map registration tracks between frames as it does in the HUD).
//!
//! Needs the reference cache (filled by the HUD's download).

use std::collections::HashMap;

use mtc_dataset::eval::Tally;
use mtc_dataset::image::RgbImage;
use mtc_dataset::labels::FrameLabel;
use mtc_dataset::{Session, datasets_dir, labels};
use mtc_vision::FrameView;
use mtc_vision::map::arrow::{ArrowSighting, find_player_arrow};
use mtc_vision::map::markers::{MarkerReader, MarkerSighting, minimap_spot_clear};
use mtc_vision::map::targets::{MapFrame, TargetBook, TargetEvent};
use mtc_vision::map::zoom::V1Registrar;
use mtc_vision::map::{CueReader, MapReference, MapTransform, MapView};

/// Position tolerance against the labels (frame px).
const POSITION_TOLERANCE: f64 = 1.0;
/// For markers placed by their label (diamond covered): label and diamond are snapped to whole
/// pixels separately, and the scope map spreads them by up to ~1 px more.
const LABEL_POSITION_TOLERANCE: f64 = 1.5;
/// Incomplete sightings (cut or partly covered outline) only refresh a target nearby, never place it.
const INCOMPLETE_POSITION_TOLERANCE: f64 = 3.0;

struct Step {
    t_ms: u64,
    file: String,
    map_id: String,
    markers: Vec<MarkerSighting>,
    arrow: Option<ArrowSighting>,
    events: Vec<TargetEvent>,
}

/// Runs cues → transform → markers/arrow → target book over a session. The map of each frame
/// is the latest map label (each map segment gets a fresh book).
fn run(session_id: &str) -> (Vec<Step>, HashMap<String, FrameLabel>) {
    let session = Session::open(datasets_dir().join("raw").join(session_id)).expect("session");
    let labels: HashMap<String, FrameLabel> = labels(session_id).expect("labels").into_iter().map(|l| (l.file.clone(), l)).collect();
    let cues = CueReader::new();
    let reader = MarkerReader::new();
    let mut registrars: HashMap<String, V1Registrar> = HashMap::new();
    let mut book = TargetBook::default();
    let mut frames: Vec<_> = session.frames.iter().collect();
    frames.sort_by_key(|m| m.t_ms);
    let mut map_id = frames.iter().find_map(|m| labels.get(&m.file).and_then(|l| l.map.as_ref()).map(|m| m.id.clone())).expect("map label");
    let mut book_map = map_id.clone();
    let objectives_of = |id: &str| mtc_maps::objectives(id).iter().map(|o| (o.u, o.v)).collect::<Vec<_>>();
    book.set_objectives(objectives_of(&map_id));
    let mut steps = Vec::new();
    for meta in frames {
        if let Some(m) = labels.get(&meta.file).and_then(|l| l.map.as_ref()) {
            map_id = m.id.clone();
        }
        if map_id != book_map {
            book.reset();
            book.set_objectives(objectives_of(&map_id));
            book_map = map_id.clone();
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
                    let info = mtc_maps::by_id(&map_id).expect("map");
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
        let markers = reader.read(&view, &transform);
        let arrow = find_player_arrow(&view, &transform);
        let spot = |m: (f64, f64)| minimap_spot_clear(&view, &transform, m);
        let events = book.observe(&MapFrame {
            t_ms: meta.t_ms,
            transform,
            markers: &markers,
            remove_hint: c.remove_hint(), dragging: false, right_click: None,
            cursor,
            client_origin: origin,
            spot_clear: (transform.view == MapView::Mini).then_some(&spot as &dyn Fn((f64, f64)) -> bool),
            placed: None,
        });
        steps.push(Step {
            t_ms: meta.t_ms,
            file: meta.file.clone(),
            map_id: map_id.clone(),
            markers,
            arrow,
            events,
        });
    }
    (steps, labels)
}

fn dist(a: (f64, f64), b: (f64, f64)) -> f64 {
    ((a.0 - b.0).powi(2) + (a.1 - b.1).powi(2)).sqrt()
}

fn check_session(session_id: &str, expected: &[(&str, &[&str])], markers: &mut Tally, player: &mut Tally, events: &mut Tally) {
    let (steps, labels) = run(session_id);
    for step in &steps {
        let Some(label) = labels.get(&step.file) else { continue };
        let case = format!("{session_id}/{}", step.file);
        if let Some(truth) = &label.markers {
            let mut ok = truth.len() == step.markers.len();
            for t in truth {
                ok &= step
                    .markers
                    .iter()
                    .any(|m| {
                        let tolerance = if m.from_label {
                            LABEL_POSITION_TOLERANCE
                        } else if m.complete {
                            POSITION_TOLERANCE
                        } else {
                            INCOMPLETE_POSITION_TOLERANCE
                        };
                        m.label == t.id && dist(m.frame, (t.x, t.y)) <= tolerance
                    });
            }
            markers.check(&case, ok, || {
                format!(
                    "found {:?}, label {:?}",
                    step.markers.iter().map(|m| (m.label.clone(), m.frame)).collect::<Vec<_>>(),
                    truth.iter().map(|t| (t.id.clone(), (t.x, t.y))).collect::<Vec<_>>()
                )
            });
        }
        if let Some(truth) = label.player {
            let ok = step.arrow.is_some_and(|a| dist(a.frame, (truth.x, truth.y)) <= POSITION_TOLERANCE);
            player.check(&case, ok, || format!("found {:?}, label {truth:?}", step.arrow.map(|a| a.frame)));
        }
    }
    if std::env::var("MTC_TRACE").is_ok_and(|v| v == session_id) {
        for s in &steps {
            eprintln!("TRACE {} {} {} {:?} {:?}", s.t_ms, s.file, s.map_id, s.markers.iter().map(|m| (m.label.clone(), (m.frame.0 as i32, m.frame.1 as i32), m.complete, m.from_label)).collect::<Vec<_>>(), s.events);
        }
    }
    for (map_id, want) in expected {
        let got: Vec<String> = steps
            .iter()
            .filter(|s| s.map_id == *map_id)
            .flat_map(|s| s.events.iter().map(move |e| (s.t_ms, e)))
            .map(|(_, e)| match e {
                TargetEvent::Added(l) => format!("+{l}"),
                TargetEvent::Removed(l) => format!("-{l}"),
                TargetEvent::Moved(l) => format!("~{l}"),
                TargetEvent::Cleared => "clear".into(),
                TargetEvent::Series(c) => format!("series {c}"),
                TargetEvent::Follow(c) => format!("follow {c}"),
            })
            .collect();
        events.check(&format!("{session_id}/{map_id}"), got == *want, || format!("events {got:?}, expected {want:?}"));
    }
}

#[test]
fn markers_player_and_targets_match_labels() {
    if !mtc_dataset::recordings_present() {
        eprintln!("skipped: the recordings are not on this machine");
        return;
    }
    let mut markers = Tally::new("markers: set, labels and positions within 1 px (1.5 px placed by label, 3 px incomplete)");
    let mut player = Tally::new("player arrow pivot within 1 px");
    let mut events = Tally::new("target events in recorded order");
    check_session(
        "20261117-014241",
        &[("fulvia_gap", &["+A1", "+A2", "+A3", "+A4", "+A5", "-A3", "+A6", "clear"])],
        &mut markers,
        &mut player,
        &mut events,
    );
    check_session(
        "20261117-160854",
        &[
            ("rohkstov", &["+A1", "+A2", "+A3", "clear"]),
            ("sokolovka", &["+A1", "+A2", "+A3", "clear"]),
            ("normandy", &["+A1", "+A2", "+A3", "clear"]),
        ],
        &mut markers,
        &mut player,
        &mut events,
    );
    // Public match: A1 on objective A and A2 next to B, deleted one by one; A1–A5 again with A4
    // on B and A5 on A (both covered), Clear all; much later A1, deleted.
    check_session(
        "20261117-222558",
        &[("fulvia_gap", &["+A1", "+A2", "clear", "+A1", "+A2", "+A3", "+A4", "+A5", "clear", "+A1", "clear"])],
        &mut markers,
        &mut player,
        &mut events,
    );
    markers.require(1.0);
    player.require(1.0);
    events.require(1.0);
}

/// Squad markers on the labelled full-map frames of a squad recording (206–318 s, Roinburg; the
/// recording player joined third and is C, the others A and B, up to 16 markers at once): each
/// frame read on its own (positions in frame px need no registration, zoomed or not). All
/// markers: the set, labels and positions as in `check_session`; labels drawn over another
/// marker's diamond, under a vehicle icon or a player's name are misread (measured). The player's
/// own series: every C marker found with its label and nothing else read as C.
#[test]
fn squad_markers_match_labels() {
    if !mtc_dataset::recordings_present() {
        eprintln!("skipped: the recordings are not on this machine");
        return;
    }
    let id = "20261120-000720";
    let Ok(session) = Session::open(datasets_dir().join("raw").join(id)) else {
        eprintln!("squad markers: recording {id} not here, skipped");
        return;
    };
    let reader = MarkerReader::new();
    let mut all = Tally::new("squad markers: set, labels and positions");
    let mut own = Tally::new("squad markers: the player's own series (C)");
    for label in labels(id).expect("labels") {
        let Some(truth) = &label.markers else { continue };
        let meta = session.frames.iter().find(|m| m.file == label.file).expect("frame");
        let image = RgbImage::load_png(&session.frame_path(meta)).expect("frame");
        let view = FrameView::rgb(&image.data, image.width, image.height);
        let o = meta.geometry.client_in_frame();
        let found = all.time(|| reader.read(&view, &MapTransform::fixed(MapView::Full, (o.x, o.y))));
        let matches = |t: &mtc_dataset::labels::Marker| {
            found.iter().any(|m| {
                let tolerance = if m.from_label {
                    LABEL_POSITION_TOLERANCE
                } else if m.complete {
                    POSITION_TOLERANCE
                } else {
                    INCOMPLETE_POSITION_TOLERANCE
                };
                m.label == t.id && dist(m.frame, (t.x, t.y)) <= tolerance
            })
        };
        let detail = || format!("found {:?}", found.iter().map(|m| (m.label.clone(), m.frame)).collect::<Vec<_>>());
        all.check(&label.file, truth.len() == found.len() && truth.iter().all(matches), detail);
        let is_c = |l: Option<&str>| l.is_some_and(|l| l.starts_with('C'));
        let own_truth: Vec<_> = truth.iter().filter(|t| is_c(t.id.as_deref())).collect();
        let own_found = found.iter().filter(|m| is_c(m.label.as_deref())).count();
        own.check(&label.file, own_truth.len() == own_found && own_truth.iter().all(|t| matches(t)), detail);
    }
    println!("{}", own.report());
    all.require(SQUAD_ALL_MIN);
    assert_eq!(own.rate(), 1.0, "the player's own markers");
}

/// Measured on the labels: 11/21 frames.
const SQUAD_ALL_MIN: f64 = 0.5;

/// Following each member of a squad recording (Roinburg, 206–318 s: A11–A16, B21–B26 and the
/// player's C1–C4 at the end), the labelled full-map frames in time order: the book follows A
/// (the player's letter not learned: no input in the recording), then B from 289 s, then C from
/// 293 s (new labels count from their second frame; the recording has no full-map frame between
/// 238 and 289 s). Each letter is counted with its markers; after a switch the targets are that letter's
/// markers and no other.
#[test]
fn each_squad_member_can_be_followed() {
    if !mtc_dataset::recordings_present() {
        eprintln!("skipped: the recordings are not on this machine");
        return;
    }
    let id = "20261120-000720";
    let Ok(session) = Session::open(datasets_dir().join("raw").join(id)) else {
        eprintln!("squad follow: recording {id} not here, skipped");
        return;
    };
    let info = mtc_maps::by_id("roinburg").expect("map");
    let Ok(img) = mtc_maps::load_reference(info) else {
        eprintln!("squad follow: no Roinburg reference in the cache, skipped");
        return;
    };
    let mut registrar = V1Registrar::new(&MapReference::from_rgba(&info.id, info.size_m(), img.width, img.height, &img.data), info.grid_cell_m);
    let reader = MarkerReader::new();
    let mut book = TargetBook::default();
    book.set_objectives(mtc_maps::objectives(&info.id).iter().map(|o| (o.u, o.v)).collect());
    let mut labelled: Vec<_> = labels(id).expect("labels").into_iter().filter(|l| l.markers.is_some()).collect();
    labelled.sort_by(|a, b| a.file.cmp(&b.file));
    let mut phases: Vec<(char, Vec<String>)> = Vec::new();
    let mut counted = None;
    for label in &labelled {
        let meta = session.frames.iter().find(|m| m.file == label.file).expect("frame");
        let follow = if meta.t_ms >= 293_000 { 'C' } else if meta.t_ms >= 280_000 { 'B' } else { 'A' };
        if book.series() != follow {
            phases.push((book.series(), targets(&book)));
            let events = book.follow(follow);
            assert_eq!(events.last(), Some(&TargetEvent::Follow(follow)));
            assert!(book.targets().is_empty());
        }
        let image = RgbImage::load_png(&session.frame_path(meta)).expect("frame");
        let view = FrameView::rgb(&image.data, image.width, image.height);
        let o = meta.geometry.client_in_frame();
        let cursor = meta.cursor.map(|(x, y)| (f64::from(x) + 0.5, f64::from(y) + 0.5));
        let Some(r) = registrar.register(&view, (o.x, o.y), cursor) else { continue };
        let markers = reader.read(&view, &r.transform);
        book.observe(&MapFrame { t_ms: meta.t_ms, transform: r.transform, markers: &markers, remove_hint: false, dragging: false, right_click: None, cursor, client_origin: (o.x, o.y), spot_clear: None, placed: None });
        if meta.t_ms == 293_854 {
            counted = Some(book.letters_seen());
        }
    }
    phases.push((book.series(), targets(&book)));
    println!("squad follow: {phases:?}; letters at 293.9 s {counted:?}");
    let six = |letter: char, first: u32| (first..first + 6).map(|n| format!("{letter}{n}")).collect::<Vec<_>>();
    assert_eq!(phases[0], ('A', six('A', 11)));
    assert_eq!(phases[1], ('B', six('B', 21)));
    assert_eq!(phases[2], ('C', vec!["C1".to_owned(), "C2".into(), "C3".into(), "C4".into()]));
    assert_eq!(counted, Some(vec![('A', 6), ('B', 6), ('C', 4)]));
}

fn targets(book: &TargetBook) -> Vec<String> {
    let mut l: Vec<String> = book.targets().iter().map(|t| t.label.clone()).collect();
    l.sort_by_key(|l| l[1..].parse::<u32>().unwrap_or(0));
    l
}
