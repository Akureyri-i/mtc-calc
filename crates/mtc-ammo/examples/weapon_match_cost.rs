//! What deciding the weapon from the label and the card names costs: the HUD's loading pipeline
//! runs it on every read that sees a menu (4.6 a second). Times it on the shell data the HUD
//! loads, for an artillery label with its cards and for tank labels whose cards are not in the
//! data.
//!
//! `cargo run --release -p mtc-ammo --example weapon_match_cost -- <shells.json>`

use std::time::Instant;

use mtc_ammo::AmmoData;

fn main() {
    let path = std::env::args().nth(1).expect("shells.json path");
    let ammo = AmmoData::from_json(&std::fs::read_to_string(&path).expect("read")).expect("parse");
    let cases: [(&str, &[&str]); 4] = [
        ("152.4 mm 2A64", &["3vO28", "3OF01", "3OF01 Proxy"]),
        ("155 mm M284", &["M107", "M795", "M549A1"]),
        ("120 mm L11A5", &[",Shot K23?1", "She(( L31A7", "Sl?e(l L3?A2 ?("]),
        ("120 mm M256", &["gV???P", "???P.??iiLyHHdI??"]),
    ];
    for (label, cards) in cases {
        let n = 200;
        let t = Instant::now();
        let mut found = None;
        for _ in 0..n {
            found = ammo.weapon_for_label_and_cards(label, cards);
        }
        println!("{label} ({} cards): {:.3} ms each, result {found:?}", cards.len(), t.elapsed().as_secs_f64() * 1000.0 / f64::from(n));
    }
}
