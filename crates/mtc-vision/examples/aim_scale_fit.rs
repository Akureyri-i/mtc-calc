//! Development: measures a configuration's text scale on its recorded frames by how
//! well the templates fit, never at run time. `label`: each (x, y) label scale's mean best-candidate
//! score and how many frames put the best candidate at the expected place; `digits`: each digit
//! scale's frames read (both lines decoded under the label at the given place) and mean cost.
//!   cargo run --release -p mtc-vision --example aim_scale_fit -- <session> label <label_x>
//!   cargo run --release -p mtc-vision --example aim_scale_fit -- <session> digits <label_x> <label_y> <lx> <ly>
use mtc_dataset::image::RgbImage;
use mtc_dataset::{Session, datasets_dir};
use mtc_vision::FrameView;
use mtc_vision::aim::{AimReader, TextScale};

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let session = Session::open(datasets_dir().join("raw").join(&a[0])).expect("session");
    let frames: Vec<(RgbImage, (i32, i32))> = session
        .frames
        .iter()
        .map(|m| {
            let c = m.geometry.client_in_frame();
            (RgbImage::load_png(&session.frame_path(m)).expect("frame"), (c.x, c.y))
        })
        .collect();
    let steps = |lo: f32, hi: f32, step: f32| -> Vec<f32> { (0..).map(|i| lo + i as f32 * step).take_while(|v| *v <= hi + 1e-4).collect() };
    match a[1].as_str() {
        "label" => {
            let expect_x: i32 = a[2].parse().expect("label x");
            println!("sx,sy,mean_score,at_place,mean_az,mean_y");
            for sy in steps(0.90, 1.24, 0.02) {
                for sx in steps(0.90, 1.16, 0.02) {
                    let reader = AimReader::with_scale(TextScale { label: (sx, sy), digits: (1.0, 1.0) });
                    let (mut sum, mut at, mut az, mut n, mut ys) = (0.0f32, 0, 0.0f32, 0, 0i64);
                    for (img, o) in &frames {
                        let view = FrameView::rgb(&img.data, img.width, img.height);
                        n += 1;
                        if let Some((s, x, y, z)) = reader.probe_label(&view, *o) {
                            if (x - expect_x).abs() <= 4 {
                                at += 1;
                                sum += s;
                                az += z;
                                ys += i64::from(y);
                            }
                        }
                    }
                    println!("{sx:.2},{sy:.2},{:.3},{at}/{n},{:.3},{}", sum / at.max(1) as f32, az / at.max(1) as f32, ys / i64::from(at.max(1)));
                }
            }
        }
        "digits" => {
            let (x, y): (i32, i32) = (a[2].parse().expect("label x"), a[3].parse().expect("label y"));
            let label = (a[4].parse().expect("label sx"), a[5].parse().expect("label sy"));
            println!("dx,dy,read,mean_cost");
            for dy in steps(0.84, 1.16, 0.01) {
                for dx in steps(0.90, 1.12, 0.01) {
                    let reader = AimReader::with_scale(TextScale { label, digits: (dx, dy) });
                    let (mut read, mut cost, mut n) = (0, 0u32, 0);
                    for (img, o) in &frames {
                        let view = FrameView::rgb(&img.data, img.width, img.height);
                        n += 1;
                        if let Some((_, _, c)) = reader.probe_lines(&view, *o, x, y) {
                            read += 1;
                            cost += c;
                        }
                    }
                    println!("{dx:.2},{dy:.2},{read}/{n},{:.1}", cost as f32 / read.max(1) as f32);
                }
            }
        }
        other => panic!("unknown mode {other}"),
    }
}
