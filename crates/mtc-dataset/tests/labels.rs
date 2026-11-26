//! Consistency of the committed labels with the local recordings.

use mtc_dataset::eval::{Tally, labelled_frames};

fn is_angle_text(text: &str, signed: bool) -> bool {
    let body = match text.as_bytes().first() {
        Some(b'+' | b'-') if signed => &text[1..],
        _ if signed => return false,
        _ => text,
    };
    let Some((int, frac)) = body.split_once('.') else {
        return false;
    };
    !int.is_empty() && int.bytes().all(|b| b.is_ascii_digit()) && frac.len() == 2 && frac.bytes().all(|b| b.is_ascii_digit())
}

#[test]
fn labels_point_at_existing_frames_and_are_well_formed() {
    if !mtc_dataset::recordings_present() {
        eprintln!("skipped: the recordings are not on this machine");
        return;
    }
    let frames = labelled_frames().expect("labels reference recorded frames");
    let mut files = Tally::new("label files exist");
    let mut aims = Tally::new("aim label format");
    for frame in &frames {
        files.check(&frame.id(), frame.path.is_file(), || "missing image".into());
        if let Some(aim) = &frame.label.aim {
            aims.check(
                &frame.id(),
                is_angle_text(&aim.elevation, true) && is_angle_text(&aim.azimuth, false),
                || format!("{aim:?}"),
            );
        }
    }
    files.require(1.0);
    aims.require(1.0);
}
