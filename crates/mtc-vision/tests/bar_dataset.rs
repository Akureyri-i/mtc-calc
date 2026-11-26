//! Ammo bar layout on every recorded gunner frame, against the known vehicle of each session.

use mtc_dataset::image::RgbImage;
use mtc_dataset::eval::Tally;
use mtc_dataset::sessions;
use mtc_vision::FrameView;
use mtc_vision::aim::{AimReader, AimResult};
use mtc_vision::bar::BarReader;

/// (session, vehicle, cells, has_grenade) from the recording log; one session can show two
/// layouts when an addon changes the ammo (PzH 2000 with and without SMArt 155).
const EXPECTED: &[(&str, &str, &[(usize, bool)])] = &[
    ("20261117-013111", "M109A6", &[(5, false)]),
    ("20261117-014241", "M109A6", &[(5, false)]),
    ("20261117-120718", "2S19", &[(4, true)]),
    ("20261117-121054", "PzH 2000", &[(6, true)]),
    ("20261117-121435", "2S7", &[(4, false)]),
    ("20261117-121745", "M270", &[(3, false)]),
    ("20261117-122016", "BM-21", &[(2, false)]),
    ("20261117-122332", "60 mm mortar", &[(4, false)]),
    ("20261117-122601", "D-30", &[(3, false)]),
    ("20261117-125220", "PzH 2000 (debug map)", &[(5, true)]),
];

#[test]
#[ignore = "decodes every frame of its sessions; run with --ignored"]
fn layout_matches_vehicle() {
    if !mtc_dataset::recordings_present() {
        eprintln!("skipped: the recordings are not on this machine");
        return;
    }
    let mut aim = AimReader::new();
    let bar = BarReader::new();
    // The bar collapses to a thin strip while the full map is open; no layout is reported
    // then. A layout that disagrees with the vehicle is a failure.
    let mut wrong = Tally::new("bar layout: no wrong layout");
    let mut found = Tally::new("bar layout: found (collapsed bar excluded)");
    for session in sessions().expect("sessions") {
        let Some(&(_, vehicle, layouts)) = EXPECTED.iter().find(|e| e.0 == session.info.id) else {
            continue;
        };
        // Every frame: these sessions keep only every third frame (and the labelled ones).
        for meta in &session.frames {
            let image = RgbImage::load_png(&session.frame_path(meta)).expect("frame");
            let view = FrameView::rgb(&image.data, image.width, image.height);
            let origin = meta.geometry.client_in_frame();
            let origin = (origin.x, origin.y);
            let label_x = match aim.read(&view, origin) {
                AimResult::Read(r) => r.label_x,
                AimResult::Unreadable { label_x, .. } => label_x,
                AimResult::Absent => continue,
            };
            let layout = found.time(|| bar.layout(&view, origin, label_x));
            let case = format!("{}/{}", session.info.id, meta.file);
            let ok = layout.is_none_or(|l| layouts.contains(&(l.cells, l.has_grenade)));
            wrong.check(&case, ok, || format!("{vehicle}: {layout:?}"));
            found.check(&case, layout.is_some(), || "no layout".into());
        }
    }
    found.require(0.0);
    wrong.require(1.0);
}
