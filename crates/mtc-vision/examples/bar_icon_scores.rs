//! Development: the loaded slot's icon scores (top four) on frames of one session and slot.
//!   cargo run --release -p mtc-vision --example bar_icon_scores -- <session> <slot> [frames]
use mtc_dataset::image::RgbImage;
use mtc_dataset::{Session, datasets_dir};
use mtc_vision::FrameView;
use mtc_vision::aim::{AimReader, AimResult};
use mtc_vision::bar::BarReader;
use mtc_vision::loading::{ShellIcons, gunner_bar_highlight};

fn main() {
    let mut args = std::env::args().skip(1);
    let id = args.next().expect("session");
    let want: usize = args.next().expect("slot").parse().expect("slot");
    let limit: usize = args.next().map_or(5, |n| n.parse().expect("frames"));
    let session = Session::open(datasets_dir().join("raw").join(&id)).expect("session");
    let (mut aim, bar, icons) = (AimReader::new(), BarReader::new(), ShellIcons::new());
    let mut shown = 0;
    for meta in &session.frames {
        let image = RgbImage::load_png(&session.frame_path(meta)).expect("frame");
        let view = FrameView::rgb(&image.data, image.width, image.height);
        let o = meta.geometry.client_in_frame();
        let label_x = match aim.read(&view, (o.x, o.y)) {
            AimResult::Read(r) => r.label_x,
            AimResult::Unreadable { label_x, .. } => label_x,
            AimResult::Absent => continue,
        };
        let Some(layout) = bar.layout(&view, (o.x, o.y), label_x) else { continue };
        if gunner_bar_highlight(&view, (o.x, o.y), &layout) != Some(want - 1) {
            continue;
        }
        if let Some(s) = icons.scores(&view, (o.x, o.y), &layout, want - 1) {
            println!("{} {:?}", meta.file, &s[..4].iter().map(|(v, n)| format!("{n} {v:.2}")).collect::<Vec<_>>());
            shown += 1;
            if shown == limit {
                break;
            }
        }
    }
}
