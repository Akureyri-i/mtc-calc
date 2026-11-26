//! Loading UI over a session's frames, as CSV:
//! file,t_ms,reason,menu,cards,bags,countdown,gunner_slot,loader_bar,micros
//! (`gunner_slot`: highlighted shell slot in the gunner's bar, 1-based; `loader_bar`: slots/loaded).
//! (`menu` gunner/loader/-, `cards` one letter per card: L/M/H selected charge, - no charges,
//! ? hidden under the hover detail box).
//!   cargo run --release -p mtc-vision --example loading_scan <session-id>
use mtc_dataset::image::RgbImage;
use mtc_dataset::{Session, datasets_dir};
use mtc_vision::FrameView;
use mtc_vision::aim::{AimReader, AimResult, TextScale};
use mtc_vision::bar::BarReader;
use mtc_vision::loading::{CardCharge, Charge, MenuSeat, countdown_shown, gunner_bar_highlight, read_loader_bar, read_menu};

fn main() {
    let id = std::env::args().nth(1).expect("session id");
    let session = Session::open(datasets_dir().join("raw").join(&id)).expect("session");
    let mut frames: Vec<_> = session.frames.iter().collect();
    frames.sort_by_key(|m| m.t_ms);
    println!("file,t_ms,reason,menu,cards,bags,countdown,gunner_slot,loader_bar,micros");
    // The readout's text scale on a remapped recording of another configuration (`aim_scan`).
    let mut aim = AimReader::with_scale(std::env::var("MTC_AIM_SCALE").ok().map_or(TextScale::REFERENCE, |v| v.parse().expect("MTC_AIM_SCALE")));
    // The bar headers' text scale likewise ("sx,sy").
    let bar = BarReader::with_text_scale(std::env::var("MTC_BAR_SCALE").ok().map_or((1.0, 1.0), |v| {
        let f: Vec<f32> = v.split(',').map(|x| x.trim().parse().expect("MTC_BAR_SCALE")).collect();
        (f[0], f[1])
    }));
    for meta in frames {
        let image = RgbImage::load_png(&session.frame_path(meta)).expect("frame");
        let view = FrameView::rgb(&image.data, image.width, image.height);
        let o = meta.geometry.client_in_frame();
        let origin = (o.x, o.y);
        let started = std::time::Instant::now();
        let menu = read_menu(&view, origin);
        let countdown = countdown_shown(&view, origin);
        let loader_bar = read_loader_bar(&view, origin);
        let micros = started.elapsed().as_micros();
        let label_x = match aim.read(&view, origin) {
            AimResult::Read(r) => Some(r.label_x),
            AimResult::Unreadable { label_x, .. } => Some(label_x),
            _ => None,
        };
        let gunner_slot = label_x.and_then(|x| bar.layout(&view, origin, x)).and_then(|l| gunner_bar_highlight(&view, origin, &l));
        let gunner_slot = gunner_slot.map_or("-".to_string(), |i| (i + 1).to_string());
        let loader_bar = loader_bar.map_or("-".to_string(), |b| format!("{}/{}", b.slots, b.loaded.map_or("-".to_string(), |i| (i + 1).to_string())));
        let bags: String = menu.as_ref().map_or(String::new(), |m| m.cards.iter().map(|c| c.bags.to_string()).collect());
        let (seat, cards) = match &menu {
            Some(m) => (
                match m.seat {
                    MenuSeat::Gunner => "gunner",
                    MenuSeat::Loader => "loader",
                    MenuSeat::Dismounted => "foot",
                },
                m.cards
                    .iter()
                    .map(|c| match c.charge {
                        CardCharge::Selected(Charge::Low) => 'L',
                        CardCharge::Selected(Charge::Medium) => 'M',
                        CardCharge::Selected(Charge::High) => 'H',
                        CardCharge::Hidden => '?',
                        CardCharge::None => '-',
                    })
                    .collect::<String>(),
            ),
            None => ("-", String::new()),
        };
        println!("{},{},{:?},{seat},{cards},{bags},{countdown},{gunner_slot},{loader_bar},{micros}", meta.file, meta.t_ms, meta.reason);
    }
}
