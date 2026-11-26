//! Development: the gunner's bar over a recording, one line per frame in time order: whether the
//! card menu or the countdown shows, the highlighted (loaded) slot and its charge bags, and the
//! weapon label, and the readout label's client x.
//!   cargo run --release -p mtc-vision --example bar_timeline -- <session>
use mtc_dataset::image::RgbImage;
use mtc_dataset::{Session, datasets_dir};
use mtc_vision::FrameView;
use mtc_vision::aim::{AimReader, AimResult, TextScale};
use mtc_vision::bar::BarReader;
use mtc_vision::loading::{countdown_shown, gunner_bar_bags, gunner_bar_highlight, read_menu, read_weapon_label};
use mtc_vision::text::TextReader;

fn main() {
    let id = std::env::args().nth(1).expect("session");
    let session = Session::open(datasets_dir().join("raw").join(&id)).expect("session");
    // The readout's text scale on a remapped recording of another configuration (`aim_scan`).
    let mut aim = AimReader::with_scale(std::env::var("MTC_AIM_SCALE").ok().map_or(TextScale::REFERENCE, |v| v.parse().expect("MTC_AIM_SCALE")));
    // The bar headers' text scale likewise ("sx,sy").
    let bar = BarReader::with_text_scale(std::env::var("MTC_BAR_SCALE").ok().map_or((1.0, 1.0), |v| {
        let f: Vec<f32> = v.split(',').map(|x| x.trim().parse().expect("MTC_BAR_SCALE")).collect();
        (f[0], f[1])
    }));
    let text = TextReader::new();
    let mut metas: Vec<_> = session.frames.iter().collect();
    metas.sort_by_key(|m| m.t_ms);
    for meta in metas {
        let image = RgbImage::load_png(&session.frame_path(meta)).expect("frame");
        let view = FrameView::rgb(&image.data, image.width, image.height);
        let o = meta.geometry.client_in_frame();
        let origin = (o.x, o.y);
        let menu = read_menu(&view, origin).map(|m| format!("menu {:?} {}", m.seat, m.cards.len())).unwrap_or_default();
        let countdown = if countdown_shown(&view, origin) { "countdown" } else { "" };
        let label_x = match aim.read(&view, origin) {
            AimResult::Read(r) => Some(r.label_x),
            AimResult::Unreadable { label_x, .. } => Some(label_x),
            AimResult::Absent => None,
        };
        let layout = label_x.and_then(|x| bar.layout(&view, origin, x));
        let (slots, lit) = match &layout {
            Some(l) => {
                let lit = gunner_bar_highlight(&view, origin, l);
                (l.shell_slots.to_string(), lit.map(|s| format!("slot {} bags {}", s + 1, gunner_bar_bags(&view, origin, l, s))).unwrap_or_else(|| "none lit".into()))
            }
            None => (String::new(), String::new()),
        };
        let weapon = read_weapon_label(&view, origin, &text).unwrap_or_default();
        let label = label_x.map(|x| (x - origin.0).to_string()).unwrap_or_default();
        println!("{:8.1},{},{menu},{countdown},{slots},{lit},{weapon},{label}", meta.t_ms as f64 / 1000.0, meta.file);
    }
}
