//! Development: where the angle readout's read-back area sits on a client of the given size and
//! scaling (frame px, client at the frame's origin) — the rectangle a problem report's readout.png
//! is cut from.
//!   cargo run --release -p mtc-vision --example readout_rect -- <width> <height> <dpi>
use mtc_vision::aim::AimReader;
use mtc_vision::layout::{Placement, UiLayout};

fn main() {
    let a: Vec<u32> = std::env::args().skip(1).map(|v| v.parse().expect("number")).collect();
    let layout = UiLayout { width: a[0], height: a[1], dpi: a[2] };
    let placement = Placement::new((0, 0), layout);
    let reader = AimReader::with_scale(layout.readout_text());
    let rect = placement.bottom_source(reader.wanted_region(placement.bottom_origin()), (a[0], a[1]));
    println!("{} {} {} {}", rect.0, rect.1, rect.2, rect.3);
}
