//! Development: what the compositor clock reports on this machine:
//!   cargo run --release -p mtc-overlay --example compositor_clock [seconds]
fn main() {
    let seconds = std::env::args().nth(1).and_then(|s| s.parse().ok()).unwrap_or(3.0);
    println!("{}", mtc_overlay::probe_compositor_clock(seconds));
}
