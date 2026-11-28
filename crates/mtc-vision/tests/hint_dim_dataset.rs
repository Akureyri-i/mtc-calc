//! Whether V1's "Add marker" is drawn translucent, on recorded frames: translucent while a marker is
//! dragged over the map and while the cursor is off the map, opaque while the map is panned. And
//! on a player's screen recording (1920 x 1080 at 100 %): translucent while they drag a marker,
//! opaque while they press on their own vehicle icon over a marker the game does not pick up.

use mtc_dataset::image::RgbImage;
use mtc_dataset::{Session, datasets_dir};
use mtc_vision::FrameView;
use mtc_vision::layout::UiLayout;
use mtc_vision::map::CueReader;

/// (session, frame file stems, translucent expected).
const RECORDED: &[(&str, &[&str], bool)] = &[
    // A marker dragged over the map (the marker and its owner's name at the cursor).
    ("20261124-042740", &["000422866", "000423515", "000447500", "000448016", "000448755", "000449268", "000451054", "000451698", "000452338", "000454719"], true),
    ("20261121-202442", &["000577359", "000577984", "000578616", "000579144", "000579772", "000581855"], true),
    // The cursor off the map, right of it.
    ("20261117-014241", &["000593509", "000596091", "000598160", "000600536"], true),
    // The map panned: its picture moved between frames under the held cursor.
    ("20261124-042740", &["000478563", "000554349", "000904964", "000906121", "001984261", "002529036"], false),
    ("20261125-050633", &["000347847", "000534858", "000547437"], false),
    ("20261125-071543", &["000274563", "000285819", "000420517", "000429983", "001711704", "001760332", "001814715", "001818865"], false),
];

#[test]
fn recorded_hint_reads_translucent_only_when_no_marker_can_be_added() {
    let raw = datasets_dir().join("raw");
    let (mut right, mut wrong, mut undecided) = (0, Vec::new(), Vec::new());
    for &(id, stems, translucent) in RECORDED {
        let Ok(session) = Session::open(&raw.join(id)) else {
            eprintln!("skipped: recording {id} is not on this machine");
            return;
        };
        for stem in stems {
            let meta = session.frames.iter().find(|m| m.file.ends_with(&format!("{stem}.png"))).expect("frame in the recording");
            let g = &meta.geometry;
            let reader = CueReader::with_layout(&UiLayout { width: g.client.width as u32, height: g.client.height as u32, dpi: g.dpi });
            let image = RgbImage::load_png(&session.frame_path(meta)).expect("frame");
            let c = g.client_in_frame();
            match reader.read(&FrameView::rgb(&image.data, image.width, image.height), (c.x, c.y)).hint_dimmed {
                Some(d) if d == translucent => right += 1,
                Some(_) => wrong.push(format!("{id}/{stem}")),
                None => undecided.push(format!("{id}/{stem}")),
            }
        }
    }
    println!("hint translucency on recorded frames: {right} right, wrong {wrong:?}, undecided {undecided:?}");
    assert!(wrong.is_empty() && undecided.is_empty());
}

#[test]
fn a_press_the_game_did_not_take_reads_opaque() {
    let dir = datasets_dir().join("raw").join("feedback-39-marker-under-icon");
    if !dir.exists() {
        eprintln!("skipped: the player's frames are not on this machine");
        return;
    }
    // The recording is the whole 1920 x 1080 client at 100 % (full screen, no title bar).
    let reader = CueReader::with_layout(&UiLayout { width: 1920, height: 1080, dpi: 96 });
    let mut wrong = Vec::new();
    for (name, translucent) in [("idle-2.0", false), ("drag-7.0", true), ("drag-7.5", true), ("drag-8.0", true), ("stuck-13.0", false), ("stuck-14.0", false), ("stuck-15.0", false), ("stuck-15.8", false)] {
        let image = RgbImage::load_png(&dir.join(format!("{name}.png"))).expect("frame");
        let read = reader.read(&FrameView::rgb(&image.data, image.width, image.height), (0, 0)).hint_dimmed;
        if read != Some(translucent) {
            wrong.push(format!("{name}: {read:?}"));
        }
    }
    assert!(wrong.is_empty(), "{wrong:?}");
}
