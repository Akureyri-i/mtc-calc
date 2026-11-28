//! The view cues with Roblox's text size one step up: the recording's one-step span read with the
//! text factor the HUD measures there (the readout fits 1.25). Every frame shows a map view: the
//! minimap (also in the sight), the full map with "Add marker" or with "Remove marker" (the hint
//! row pushed left), or the scope map. Counts from the span, checked frame by frame against where
//! the Ping icon sits and whether the sight's black surround shows.

use mtc_dataset::image::RgbImage;
use mtc_dataset::{Session, datasets_dir};
use mtc_vision::FrameView;
use mtc_vision::layout::UiLayout;
use mtc_vision::map::{CueReader, MapView};

const SESSION: &str = "20261127-221541";
const ONE_STEP_MS: (u64, u64) = (258_000, 370_000);

#[test]
fn views_show_with_the_text_size_one_step_up() {
    let dir = datasets_dir().join("raw").join(SESSION);
    let Ok(session) = Session::open(&dir) else {
        eprintln!("skipped: the recording is not on this machine");
        return;
    };
    let (mut full, mut remove, mut mini, mut scope, mut none) = (0, 0, 0, 0, Vec::new());
    let mut reader: Option<CueReader> = None;
    for meta in session.frames.iter().filter(|m| (ONE_STEP_MS.0..=ONE_STEP_MS.1).contains(&m.t_ms)) {
        let g = &meta.geometry;
        let reader = reader.get_or_insert_with(|| {
            let mut r = CueReader::with_layout(&UiLayout { width: g.client.width as u32, height: g.client.height as u32, dpi: g.dpi });
            r.set_text_factor(1.25);
            r
        });
        let image = RgbImage::load_png(&session.frame_path(meta)).expect("frame");
        let c = g.client_in_frame();
        let cues = reader.read(&FrameView::rgb(&image.data, image.width, image.height), (c.x, c.y));
        match cues.primary() {
            Some(MapView::Full) => {
                full += 1;
                remove += usize::from(cues.remove_hint());
            }
            Some(MapView::Mini) => mini += 1,
            Some(MapView::Scope) => scope += 1,
            None => none.push(meta.t_ms),
        }
    }
    println!("text size one step up: full map {full} (Remove marker {remove}), minimap {mini}, scope map {scope}, none {}", none.len());
    assert!(none.is_empty(), "no view at {none:?}");
    assert_eq!((full, remove, mini, scope), (22, 15, 200, 5));
}
