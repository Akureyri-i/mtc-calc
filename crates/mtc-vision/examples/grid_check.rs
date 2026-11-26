//! Grid fit on one unzoomed V1 frame vs the centred-grid model:
//!   cargo run --release -p mtc-vision --example grid_check <png> <map-id>
use mtc_dataset::image::RgbImage;
use mtc_vision::FrameView;
use mtc_vision::map::zoom::{GridFit, grid_origin_m};
use mtc_vision::map::{MapTransform, MapView};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let info = mtc_maps::by_id(&args[2]).expect("map");
    let w = info.size_m();
    let image = RgbImage::load_png(std::path::Path::new(&args[1])).expect("frame");
    let view = FrameView::rgb(&image.data, image.width, image.height);
    let t = MapTransform::fixed(MapView::Full, (0, 34));
    for c in GridFit::candidates(&view, &t, w, &[info.grid_cell_m.unwrap_or(100.0)]).iter().take(6) {
        println!("  level zoom {:.1} period {:7.2} score {:.2}", c.zoom, c.period, c.score);
    }
    let Some(g) = GridFit::fit(&view, &t, w, &[info.grid_cell_m.unwrap_or(100.0)]) else {
        println!("no grid");
        return;
    };
    let cell_m = info.grid_cell_m.unwrap_or(100.0);
    let cell = cell_m / w * t.size;
    let first = t.left + grid_origin_m(w, cell_m) / w * t.size;
    let model = |line: f64, edge: f64| {
        let k = ((line - first.min(edge)) / cell).round();
        line - (first - t.left + edge + k * cell)
    };
    println!("zoom {} period {:.3} (model {:.3}) score {:.2}", g.zoom, g.period, cell, g.score);
    println!("first line x {:.2} y {:.2}", g.first_line.0, g.first_line.1);
    let off_x = model(g.first_line.0, t.left);
    let off_y = model(g.first_line.1, t.top);
    println!("observed - model: x {off_x:+.2} px ({:+.2} m), y {off_y:+.2} px ({:+.2} m)", off_x / t.size * w, off_y / t.size * w);
}
