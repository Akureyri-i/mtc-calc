//! Development: which weapon names read glyph by glyph (a file of "<count>\t<text>" lines, as the
//! stats records collect them) the normalised match takes for a known weapon's label.
//!   cargo run --release -p mtc-vision --example label_rescue -- <shells.json> <labels.txt>
use mtc_ammo::AmmoData;
use mtc_vision::loading::label_by_normalised;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let ammo = AmmoData::from_json(&std::fs::read_to_string(&args[0]).expect("shells.json")).expect("ammo data");
    let labels = ammo.label_texts();
    let (mut rescued, mut total) = (0u64, 0u64);
    for line in std::fs::read_to_string(&args[1]).expect("labels").lines() {
        let Some((count, text)) = line.split_once('\t') else { continue };
        let count: u64 = count.parse().unwrap_or(0);
        total += count;
        match label_by_normalised(text, &labels) {
            Some(label) => {
                rescued += count;
                println!("{count:>5}  {text:?} -> {label:?}");
            }
            None => println!("{count:>5}  {text:?} -> none"),
        }
    }
    println!("{rescued} of {total} reads taken for a known label ({} labels known)", labels.len());
}
