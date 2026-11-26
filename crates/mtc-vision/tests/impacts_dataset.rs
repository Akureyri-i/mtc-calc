//! Yellow X impacts on every frame with impact labels: each frame is read in all three map
//! containers. An X in its first 1.5 s (strength above ~0.25) must be found, within
//! 0.5 px; across the whole fade at least 95 %; nothing may be found where no X is labelled. An X
//! beside or under an objective icon is read as the HUD reads it (beside the icon, then from the
//! tips of its arms) and scored apart; so is an X partly covered by vehicle icons or a death X.

use std::collections::HashMap;

use mtc_dataset::eval::{Tally, labelled_frames};
use mtc_vision::FrameView;
use mtc_vision::map::impacts::{ImpactSighting, read_impact_under, read_impacts, read_impacts_among};
use mtc_vision::map::{MapTransform, MapView};

const VIEWS: [MapView; 3] = [MapView::Full, MapView::Scope, MapView::Mini];

#[test]
fn impacts_match_labels() {
    if !mtc_dataset::recordings_present() {
        eprintln!("skipped: the recordings are not on this machine");
        return;
    }
    let mut frames: Vec<_> = labelled_frames().expect("dataset").into_iter().filter(|f| f.label.impacts.is_some()).collect();
    frames.sort_by(|a, b| (a.session.as_str(), a.meta.t_ms).cmp(&(b.session.as_str(), b.meta.t_ms)));
    let mut young = Tally::new("impacts: found in the first 1.5 s");
    let mut all = Tally::new("impacts: found over the whole fade");
    let mut placed = Tally::new("impacts: within 0.5 px");
    let mut beside = Tally::new("impacts: beside or under an objective icon, found as the HUD reads it");
    let mut beside_placed = Tally::new("impacts: beside or under an objective icon, within 0.5 px");
    let mut covered = Tally::new("impacts: partly covered by vehicle icons or a death X, found");
    let mut clean = Tally::new("impacts: no false sighting");
    // When each labelled X was first seen (session, position in whole px) and last seen.
    let mut first_seen: HashMap<(String, i64, i64), (u64, u64)> = HashMap::new();
    for frame in &frames {
        let image = frame.image().expect("frame image");
        let view = FrameView::rgb(&image.data, image.width, image.height);
        let o = frame.meta.geometry.client_in_frame();
        let seen: Vec<_> = VIEWS.into_iter().flat_map(|v| clean.time(|| read_impacts(&view, &MapTransform::fixed(v, (o.x, o.y))))).collect();
        let truth = frame.label.impacts.as_ref().expect("filtered");
        for p in truth {
            let key = (frame.session.clone(), p.x.round() as i64, p.y.round() as i64);
            let t = frame.meta.t_ms;
            let entry = first_seen.entry(key).or_insert((t, t));
            if t.saturating_sub(entry.1) > 2500 {
                *entry = (t, t);
            }
            entry.1 = t;
            let age = t - entry.0;
            let nearest = |sightings: &[ImpactSighting]| sightings.iter().map(|s| ((s.frame.0 - p.x).hypot(s.frame.1 - p.y), *s)).min_by(|a, b| a.0.total_cmp(&b.0));
            let near = match p.under {
                Some(icon) => {
                    // The smallest container holding it (the minimap lies inside the scope map's).
                    let container = [MapView::Mini, MapView::Scope, MapView::Full]
                        .into_iter()
                        .map(|v| MapTransform::fixed(v, (o.x, o.y)))
                        .find(|c| c.contains_frame(p.x, p.y))
                        .expect("an X inside a map container");
                    let icons = [(icon.x, icon.y)];
                    nearest(&read_impacts_among(&view, &container, &icons))
                        .filter(|(d, _)| *d <= 3.0)
                        .or_else(|| nearest(&read_impact_under(&view, &container, icons[0], &icons).into_iter().collect::<Vec<_>>()))
                }
                None => nearest(&seen),
            };
            let found = near.is_some_and(|(d, _)| d <= 3.0);
            let detail = || format!("X at ({:.1}, {:.1}) {age} ms old: nearest {near:?}", p.x, p.y);
            if p.covered {
                covered.check(&frame.id(), found, detail);
                continue;
            }
            if p.under.is_some() {
                beside.check(&frame.id(), found, detail);
                if let Some((d, _)) = near.filter(|_| found) {
                    beside_placed.check(&frame.id(), d <= 0.5, || format!("off by {d:.2} px"));
                }
                continue;
            }
            if age <= 1500 {
                young.check(&frame.id(), found, detail);
            }
            all.check(&frame.id(), found, detail);
            if let Some((d, _)) = near.filter(|_| found) {
                placed.check(&frame.id(), d <= 0.5, || format!("off by {d:.2} px"));
            }
        }
        let stray: Vec<_> = seen.iter().filter(|s| !truth.iter().any(|p| (s.frame.0 - p.x).hypot(s.frame.1 - p.y) <= 3.0)).collect();
        clean.check(&frame.id(), stray.is_empty(), || format!("{stray:?}"));
    }
    for t in [&young, &all, &placed, &beside, &beside_placed, &covered, &clean] {
        println!("{}", t.report());
    }
    assert!(young.rate() >= 1.0 && all.rate() >= 0.95 && placed.rate() >= 1.0 && clean.rate() >= 1.0, "impacts below the bar");
    assert!(beside.rate() >= BESIDE_MIN && beside_placed.rate() >= 1.0, "impacts beside objective icons below the bar");
    assert!(covered.rate() >= COVERED_MIN, "covered impacts below the bar");
}

/// Measured on the labels; raise as the readers improve. Beside an icon: 54/55, a faint X beside
/// D on the full map is read without the icon given but not with it. Covered by vehicle icons or a
/// death X: 14/16 (the two left: two red tanks over the middle leave a third of the arms in view;
/// an objective icon and a tank leave only the upper two tips).
const BESIDE_MIN: f64 = 0.98;
const COVERED_MIN: f64 = 0.85;

/// The X under an objective icon on the minimap, read from the tips of its arms (a match on
/// Rohkshort; icon centres measured off the frames): shots at objective B fired at the same angles
/// (the full map read three of their Xs within 0.7 m of each other; the minimap readings below
/// agree with them within 0.4 px in the replay), shots beside A, one X right of B.
/// Where vehicle icons hide one diagonal's tips, nothing is read. Between 460 s and 740 s no shell
/// landed (the replay's whole-match pass, every icon on every frame, found none): nothing is read
/// at either icon on any frame. Skipped where the recording is not on this machine.
#[test]
fn x_under_objective_icons() {
    if !mtc_dataset::recordings_present() {
        eprintln!("skipped: the recordings are not on this machine");
        return;
    }
    use mtc_vision::map::impacts::read_impact_under;
    let Ok(session) = mtc_dataset::Session::open(mtc_dataset::datasets_dir().join("raw").join("20261121-202442")) else {
        eprintln!("x under icons: recording 20261121-202442 not here, skipped");
        return;
    };
    let (a, b) = ((2234.0, 258.5), (2332.5, 382.5));
    let read = |t_ms: u64, icon: (f64, f64)| {
        let meta = session.frames.iter().find(|m| m.t_ms == t_ms).expect("frame");
        let image = mtc_dataset::image::RgbImage::load_png(&session.frame_path(meta)).expect("png");
        let view = FrameView::rgb(&image.data, image.width, image.height);
        let o = meta.geometry.client_in_frame();
        read_impact_under(&view, &MapTransform::fixed(MapView::Mini, (o.x, o.y)), icon, &[icon])
    };
    let cases: [(u64, (f64, f64), Option<(f64, f64)>); 12] = [
        (126_063, b, Some((2325.0, 384.2))),
        (744_212, b, Some((2324.9, 384.0))),
        (757_646, b, Some((2324.9, 384.1))),
        (931_572, b, Some((2325.0, 384.1))),
        (916_946, b, Some((2337.2, 381.5))),
        (840_087, b, Some((2354.0, 384.7))),
        (268_926, a, Some((2233.5, 265.3))),
        (427_557, a, Some((2233.6, 265.3))),
        (454_570, a, Some((2233.7, 265.3))),
        (202_284, b, None),
        (881_645, b, None),
        (888_892, b, None),
    ];
    let mut placed = Tally::new("x under icons: read where it is (0.5 px)");
    for (t_ms, icon, want) in cases {
        let got = placed.time(|| read(t_ms, icon));
        let ok = match (want, got) {
            (Some(w), Some(g)) => (g.frame.0 - w.0).hypot(g.frame.1 - w.1) <= 0.5,
            (None, None) => true,
            _ => false,
        };
        placed.check(&t_ms.to_string(), ok, || format!("want {want:?}, got {got:?}"));
    }
    let mut clean = Tally::new("x under icons: nothing where no shell landed");
    for meta in session.frames.iter().filter(|m| (460_000..740_000).contains(&m.t_ms)) {
        for icon in [a, b] {
            let got = read(meta.t_ms, icon);
            clean.check(&meta.t_ms.to_string(), got.is_none(), || format!("{got:?}"));
        }
    }
    println!("{}", placed.report());
    println!("{}", clean.report());
    assert_eq!(placed.rate(), 1.0);
    assert_eq!(clean.rate(), 1.0);
}
