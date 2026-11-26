//! Development: charge bags read on the gunner's bar (the loaded slot's capsule) against the
//! charge that slot showed in the latest card menu of the same session (1 bag Low, 2 Medium,
//! 3 High), over every recorded frame in time order.
//!   cargo run --release -p mtc-vision --example bar_bags_eval
use std::collections::HashMap;

use mtc_dataset::image::RgbImage;
use mtc_dataset::{Session, datasets_dir, sessions};
use mtc_vision::FrameView;
use mtc_vision::aim::{AimReader, AimResult};
use mtc_vision::bar::BarReader;
use mtc_vision::loading::{CardCharge, Charge, MenuSeat, gunner_bar_bags, gunner_bar_highlight, read_menu};

fn main() {
    let mut aim = AimReader::new();
    let bar = BarReader::new();
    let (mut agree, mut disagree, mut unknown, mut zero) = (0, 0, 0, 0);
    let mut seen: HashMap<u32, u32> = HashMap::new();
    for info in sessions().expect("sessions") {
        let id = info.info.id.clone();
        let session = Session::open(datasets_dir().join("raw").join(&id)).expect("session");
        let mut metas: Vec<_> = session.frames.iter().collect();
        metas.sort_by_key(|m| m.t_ms);
        let mut charges: HashMap<usize, Charge> = HashMap::new();
        for meta in metas {
            let image = RgbImage::load_png(&session.frame_path(meta)).expect("frame");
            let view = FrameView::rgb(&image.data, image.width, image.height);
            let o = meta.geometry.client_in_frame();
            let origin = (o.x, o.y);
            if let Some(menu) = read_menu(&view, origin).filter(|m| m.seat == MenuSeat::Gunner) {
                for (slot, card) in menu.cards.iter().enumerate() {
                    if let CardCharge::Selected(c) = card.charge {
                        charges.insert(slot, c);
                    }
                }
                continue;
            }
            let label_x = match aim.read(&view, origin) {
                AimResult::Read(r) => r.label_x,
                AimResult::Unreadable { label_x, .. } => label_x,
                AimResult::Absent => continue,
            };
            let Some(layout) = bar.layout(&view, origin, label_x) else { continue };
            let Some(slot) = gunner_bar_highlight(&view, origin, &layout) else { continue };
            let bags = gunner_bar_bags(&view, origin, &layout, slot);
            *seen.entry(bags).or_default() += 1;
            let read = match bags {
                1 => Some(Charge::Low),
                2 => Some(Charge::Medium),
                3 => Some(Charge::High),
                _ => None,
            };
            match (read, charges.get(&slot)) {
                (None, _) => {
                    zero += 1;
                    println!("{id},{},slot {},bags {bags},menu {:?}", meta.file, slot + 1, charges.get(&slot));
                }
                (Some(_), None) => unknown += 1,
                (Some(r), Some(&m)) if r == m => agree += 1,
                (Some(r), Some(&m)) => {
                    disagree += 1;
                    println!("{id},{},slot {},bar {r:?},menu {m:?}", meta.file, slot + 1);
                }
            }
        }
    }
    eprintln!("highlighted bar slots: bags agree with the menu {agree}, disagree {disagree}, no menu seen before {unknown}, no bags read {zero}; counts {seen:?}");
}
