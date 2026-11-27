//! Development: reads the gunner's weapon label from a crop of the loading area at a display
//! configuration, as the HUD does. The crop (a problem report's `loading.png`, or the same region
//! cut from a player's screenshot) is put back where the report cut it in a client of that size,
//! resampled into the reference geometry, then compared whole with the data's labels and read
//! glyph by glyph with the templates in `<glyphs.json>` (`mtc-overlay`'s `ui_glyphs_json`; `asset`:
//! the reference configuration's).
//!   cargo run --release -p mtc-vision --example label_from_crop -- <crop.png> <glyphs.json> <shells.json> <dpi,width,height>
use mtc_ammo::AmmoData;
use mtc_dataset::image::RgbImage;
use mtc_vision::FrameView;
use mtc_vision::layout::{Placement, UiLayout};
use mtc_vision::loading::{LOADING_REGION, read_weapon_label, read_weapon_label_among, weapon_box, weapon_box_scores};
use mtc_vision::text::TextReader;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let crop = RgbImage::load_png(std::path::Path::new(&args[0])).expect("crop png");
    let reader = if args[1] == "asset" { TextReader::new() } else { TextReader::from_json(&std::fs::read_to_string(&args[1]).expect("glyphs")) };
    let labels = AmmoData::from_json(&std::fs::read_to_string(&args[2]).expect("shells.json")).expect("ammo data").label_texts();
    let n: Vec<u32> = args[3].split(',').map(|v| v.parse().expect("dpi,width,height")).collect();
    let layout = UiLayout { width: n[1], height: n[2], dpi: n[0] };
    let placement = Placement::new((0, 0), layout);
    let virtual_origin = Placement::VIRTUAL_ORIGIN;
    let (lx, ly, lw, lh) = LOADING_REGION;
    let rect = (lx + virtual_origin.0, ly + virtual_origin.1, lw, lh);
    let (sx, sy, _, _) = placement.bottom_source(rect, (layout.width, layout.height));
    let mut frame = RgbImage::new(layout.width, layout.height);
    for y in 0..crop.height {
        for x in 0..crop.width {
            let (fx, fy) = (sx + x as i32, sy + y as i32);
            if fx < 0 || fy < 0 || fx >= layout.width as i32 || fy >= layout.height as i32 {
                continue;
            }
            let (s, d) = (((y * crop.width + x) * 3) as usize, ((fy as u32 * layout.width + fx as u32) * 3) as usize);
            frame.data[d..d + 3].copy_from_slice(&crop.data[s..s + 3]);
        }
    }
    let source = FrameView::rgb(&frame.data, layout.width, layout.height);
    let remapped = placement.bottom_remap(&source, rect);
    let view = remapped.view();
    println!("crop placed at ({sx}, {sy}); weapon box {:?}", weapon_box(&view, virtual_origin));
    let scores = weapon_box_scores(&view, virtual_origin, &reader, &labels);
    for &(i, score) in scores.iter().take(5) {
        println!("  {score:.3}  {}", labels[i]);
    }
    println!("glyph by glyph: {:?}", read_weapon_label(&view, virtual_origin, &reader));
    println!("as the HUD reads it: {:?}", read_weapon_label_among(&view, virtual_origin, &reader, &labels));
}
