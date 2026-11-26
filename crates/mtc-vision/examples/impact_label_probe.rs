//! Development (labelling yellow X): what the X readers make of given frames when the
//! objective icons of a map are known, as the HUD reads them. Reads lines `session file map_id`
//! from stdin; for each, the minimap sightings beside the icons (`read_impacts_among`) and, for
//! every icon, the X read from its tips (`read_impact_under`). Icon centres are the objective
//! table's on the fixed minimap.
//!   impact_label_probe < frames.txt
//! Output: `session file among T ms, under T ms; icons=[L(x,y)…] among=[(x,y) a.. arms..] under=[L:(x,y) arms..]`
use std::io::BufRead;

use mtc_dataset::{Session, datasets_dir, image::RgbImage};
use mtc_vision::FrameView;
use mtc_vision::map::impacts::{read_impact_under, read_impacts_among};
use mtc_vision::map::{MapTransform, MapView};

fn main() {
    let raw = datasets_dir().join("raw");
    let mut open: Option<(String, Session)> = None;
    for line in std::io::stdin().lock().lines() {
        let line = line.expect("stdin");
        let parts: Vec<&str> = line.split_whitespace().collect();
        let [id, file, map] = parts[..] else { continue };
        if open.as_ref().is_none_or(|(o, _)| o != id) {
            open = Some((id.to_owned(), Session::open(raw.join(id)).expect("session")));
        }
        let session = &open.as_ref().expect("opened").1;
        let meta = session.frames.iter().find(|m| m.file == file).expect("frame");
        let image = RgbImage::load_png(&session.frame_path(meta)).expect("png");
        let view = FrameView::rgb(&image.data, image.width, image.height);
        let o = meta.geometry.client_in_frame();
        let t = MapTransform::fixed(MapView::Mini, (o.x, o.y));
        let objectives = mtc_maps::objectives(map);
        let icons: Vec<(f64, f64)> = objectives.iter().map(|ob| t.to_frame(ob.u, ob.v)).collect();
        let started = std::time::Instant::now();
        let among = read_impacts_among(&view, &t, &icons);
        let among_ms = started.elapsed().as_secs_f64() * 1000.0;
        let started = std::time::Instant::now();
        let under: Vec<String> = objectives
            .iter()
            .zip(&icons)
            .filter_map(|(ob, &icon)| read_impact_under(&view, &t, icon, &icons).map(|s| format!("{}:({:.2},{:.2}) arms{:.2}", ob.letter, s.frame.0, s.frame.1, s.arms)))
            .collect();
        let under_ms = started.elapsed().as_secs_f64() * 1000.0;
        let icons_s: Vec<String> = objectives.iter().zip(&icons).map(|(ob, i)| format!("{}({:.1},{:.1})", ob.letter, i.0, i.1)).collect();
        let among_s: Vec<String> = among.iter().map(|s| format!("({:.2},{:.2}) a{:.2} arms{:.2}", s.frame.0, s.frame.1, s.alpha, s.arms)).collect();
        println!("{id} {file} among {among_ms:.1} ms, under {under_ms:.1} ms; icons=[{}] among=[{}] under=[{}]", icons_s.join(" "), among_s.join(" "), under.join(" "));
    }
}
