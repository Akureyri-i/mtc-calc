//! Weapon and shell identification on the labelled loading menus: the weapon label and every
//! card's name and VEL value are read and matched to the ammunition data snapshot
//! (`reference/wiki`); each card must come out as one ballistic choice that includes the shell
//! actually on it.
//!
//! The shell on each card (by session and position, with the card's charge as labelled) was read
//! off the recorded frames by eye. On foot the weapon strip under the cards falls below the
//! screen, so those frames have no weapon label to read; there the on-foot hotbar also covers the
//! left card's VEL row (one card in the set). The readers see only the HUD's read-back region
//! (`LOADING_REGION`). Needs the recorded frames (datasets/raw) and the data snapshot.

use std::collections::HashMap;

use mtc_ammo::{AmmoData, ChargeLevel, same_ballistics};
use mtc_dataset::eval::{Tally, labelled_frames};
use mtc_dataset::labels::{CardChargeLabel, LoadingState, Seat};
use mtc_vision::FrameView;
use mtc_vision::loading::{LOADING_REGION, read_card_texts_among, read_menu, read_weapon_label_among};
use mtc_vision::text::TextReader;

const SNAPSHOT: &str = "reference/wiki/production-1.0.0/20261117/shells.json";

/// Weapon (data name) and the shell on each card: a name without charge, or a base that takes
/// the card's charge (" Low Charge", …) appended (`+`).
fn sessions() -> HashMap<&'static str, (&'static str, Vec<&'static str>)> {
    let m284 = ("155mm M284", vec!["M107+ VT", "M107+", "M107+ PF", "M110+"]);
    HashMap::from([
        ("20261117-013111", m284.clone()),
        ("20261117-014241", m284),
        ("20261117-120718", ("152mm 2A64", vec!["3OF45 Proxy+", "3OF45+"])),
        ("20261117-121054", ("155mm L62", vec!["DM702A1", "DM121 Proxy+", "DM121+", "DM702A1+"])),
        ("20261117-121435", ("203mm 2A44", vec!["2VG11", "3VO15+", "3VOF34+"])),
        ("20261117-121745", ("227mm M269", vec!["M26A1 Low Charge", "M31A2 Low Charge"])),
        ("20261117-122016", ("122mm Grad", vec!["9M22/M21 HE-Frag"])),
        ("20261117-122332", ("60mm M2", vec!["M49A2 Medium Charge", "M49A2 Low Charge", "M49A2 High Charge"])),
        ("20261117-122601", ("122mm D-30", vec!["3BK-10", "3OF56+"])),
        // 2S33: the label shows the 2S19's "152.4 mm 2A64"; the cards tell the converted weapon.
        ("20261121-202442", ("152mm 2A64M", vec!["3VO28+", "3OF61+", "3OF61 Proxy+"])),
    ])
}

/// Menus whose weapon label a game notice covers ("Match progression bonus, +50" over most of it;
/// seen on the frames): no label to read.
const LABEL_COVERED: [(&str, &str); 3] = [
    ("20261121-202442", "frames/000244844.png"),
    ("20261121-202442", "frames/000245875.png"),
    ("20261121-202442", "frames/000246910.png"),
];

fn expected(entry: &str, charge: Option<&str>) -> Option<String> {
    match entry.split_once('+') {
        Some((base, rest)) => Some(format!("{base} {} Charge{rest}", charge?)),
        None => Some(entry.to_owned()),
    }
}

#[test]
fn weapons_and_shells_match_the_data() {
    if !mtc_dataset::recordings_present() {
        eprintln!("skipped: the recordings are not on this machine");
        return;
    }
    let root = mtc_dataset::datasets_dir().parent().expect("repo").join(SNAPSHOT);
    let data = AmmoData::from_json(&std::fs::read_to_string(&root).expect("ammo data snapshot")).expect("parse");
    let truth = sessions();
    let reader = TextReader::new();
    let labels = data.label_texts();
    let mut weapons = Tally::new("weapon label to data weapon (menu frames)");
    let mut shells = Tally::new("card to one ballistic choice including its shell");
    let mut micros = Vec::new();
    for frame in labelled_frames().expect("dataset") {
        let Some(label) = &frame.label.loading else { continue };
        if label.state != LoadingState::Menu {
            continue;
        }
        let Some((weapon, cards)) = truth.get(frame.session.as_str()) else { continue };
        let image = frame.image().expect("frame image");
        let full = FrameView::rgb(&image.data, image.width, image.height);
        let o = frame.meta.geometry.client_in_frame();
        let origin = (o.x, o.y);
        let (rx, ry, rw, rh) = LOADING_REGION;
        let view = full.sub(origin.0 + rx, origin.1 + ry, rw as u32, rh as u32);
        let Some(menu) = read_menu(&view, origin) else { continue };
        let case = frame.id();
        let started = std::time::Instant::now();
        let text = read_weapon_label_among(&view, origin, &reader, &labels);
        // As the HUD before it has decided the weapon: the card names compared with what the
        // cards of the weapons the label names can show, the weapon told by label and cards.
        let label_weapons = text.as_deref().map(|t| data.label_weapons(t)).unwrap_or_default();
        let texts = read_card_texts_among(&view, origin, &menu, &reader, &|| data.card_velocities(&label_weapons), &|v| data.card_texts(&label_weapons, v));
        micros.push(started.elapsed().as_micros());
        let names: Vec<&str> = texts.iter().filter_map(|t| t.name.as_deref()).collect();
        let found = text.as_deref().and_then(|t| data.weapon_for_label_and_cards(t, &names));
        if label.menu_seat != Some(Seat::Dismounted) && !LABEL_COVERED.contains(&(frame.session.as_str(), frame.meta.file.as_str())) {
            weapons.check(&case, found == Some(*weapon), || format!("read {text:?} -> {found:?}"));
        }
        let (Some(labels), Some(bags)) = (&label.cards, &label.bags) else { continue };
        for (i, ((t, want_charge), entry)) in texts.iter().zip(labels).zip(cards).enumerate() {
            let charge = match want_charge {
                CardChargeLabel::Low => Some("Low"),
                CardChargeLabel::Medium => Some("Medium"),
                CardChargeLabel::High => Some("High"),
                CardChargeLabel::None | CardChargeLabel::Hidden => match bags[i] {
                    1 => Some("Low"),
                    2 => Some("Medium"),
                    3 => Some("High"),
                    _ => None,
                },
            };
            let level = match charge {
                Some("Low") => Some(ChargeLevel::Low),
                Some("Medium") => Some(ChargeLevel::Medium),
                Some("High") => Some(ChargeLevel::High),
                _ => None,
            };
            // A card labelled without charges: identified without one (rockets show one bag).
            let level = if *want_charge == CardChargeLabel::None && !entry.contains('+') && !entry.contains("Charge") { None } else { level };
            let Some(want) = expected(entry, charge) else { continue };
            let candidates = data.card_candidates(weapon, t.name.as_deref().unwrap_or(""), t.velocity, level);
            let ok = !candidates.is_empty() && same_ballistics(&candidates) && candidates.iter().any(|s| s.name == want);
            shells.check(&format!("{case} card {}", i + 1), ok, || {
                format!("read {:?} / {:?} -> {:?}, want {want}", t.name, t.velocity, candidates.iter().map(|s| &s.name).collect::<Vec<_>>())
            });
        }
    }
    micros.sort_unstable();
    println!("{}", weapons.report());
    println!("{}", shells.report());
    println!("text reading per menu: median {} us, max {} us", micros.get(micros.len() / 2).copied().unwrap_or(0), micros.last().copied().unwrap_or(0));
    weapons.require(1.0);
    shells.require(0.99);
}
