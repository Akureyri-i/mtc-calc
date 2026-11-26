//! Development: map identification on V1 frames as the live HUD would try it while no map is
//! decided — the objective layout, else one frame above the V1 thresholds — over every labelled
//! V1 frame, zoomed ones included: how many decide, and whether any decides wrong.
//!   cargo run --release -p mtc-vision --example v1_ident_check
use mtc_dataset::eval::labelled_frames;
use mtc_dataset::labels::MapView as LabelView;
use mtc_vision::FrameView;
use mtc_vision::map::{MapIdentifier, MapReference, MapTransform, MapView, Objective};

fn main() {
    let maps = mtc_maps::all();
    let mut identifier = MapIdentifier::new(&[]);
    for m in maps {
        let img = mtc_maps::load_reference(m).expect("reference");
        let layout: Vec<Objective> = mtc_maps::objectives(&m.id).iter().map(|o| Objective { letter: o.letter.chars().next().expect("letter"), u: o.u, v: o.v }).collect();
        identifier.push(&MapReference::from_rgba(&m.id, m.size_m(), img.width, img.height, &img.data), &layout);
    }
    let (mut unzoomed, mut zoomed) = ([0u32; 3], [0u32; 3]); // frames, right, wrong
    for frame in labelled_frames().expect("dataset") {
        let Some(map) = &frame.label.map else { continue };
        if map.view != LabelView::V1 {
            continue;
        }
        let image = frame.image().expect("frame");
        let view = FrameView::rgb(&image.data, image.width, image.height);
        let o = frame.meta.geometry.client_in_frame();
        let t = MapTransform::fixed(MapView::Full, (o.x, o.y));
        let tally = if map.zoom == Some(1.0) { &mut unzoomed } else { &mut zoomed };
        tally[0] += 1;
        if let Some(m) = identifier.identify(&view, &t).filter(|m| m.confident_on(MapView::Full)) {
            if maps[m.index].id == map.id {
                tally[1] += 1;
            } else {
                tally[2] += 1;
                println!("WRONG {}/{} zoom {:?}: {} (score {:.3}, margin {:.3}, layout {})", frame.session, frame.meta.file, map.zoom, maps[m.index].id, m.score, m.margin, m.by_layout);
            }
        }
    }
    println!("unzoomed V1: {} frames, decided right {}, wrong {}", unzoomed[0], unzoomed[1], unzoomed[2]);
    println!("zoomed V1: {} frames, decided right {}, wrong {}", zoomed[0], zoomed[1], zoomed[2]);
}
