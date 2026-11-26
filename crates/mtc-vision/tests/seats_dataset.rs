//! Seat menu on every frame labelled with one: the rows' kinds and whether each seat is taken
//! must all be right, and so must the player's own seat wherever the name could be learned.
//!
//! The HUD learns the name when the ammo bar shows the gunner's or loader's layout. Here each
//! session learns it from its first labelled frame with the player in the gunner's or loader's
//! seat (that frame's label standing in for the ammo bar), then every other frame of the session
//! is checked. In sessions with no such frame the name stays unknown and no row may be marked.

use mtc_dataset::eval::{Tally, labelled_frames};
use mtc_dataset::labels::{SeatKind as LabelKind, SeatRowLabel};
use mtc_vision::FrameView;
use mtc_vision::seats::{SeatKind, SeatMenu, SeatReader};

fn kind(k: LabelKind) -> SeatKind {
    match k {
        LabelKind::Driver => SeatKind::Driver,
        LabelKind::Gunner => SeatKind::Gunner,
        LabelKind::Commander => SeatKind::Commander,
        LabelKind::Loader => SeatKind::Loader,
        LabelKind::Passenger => SeatKind::Passenger,
    }
}

fn describe(menu: &Option<SeatMenu>) -> String {
    match menu {
        None => "no menu".into(),
        Some(m) => m.rows.iter().map(|r| format!("{:?}{}{}", r.kind, if r.taken { "+" } else { "-" }, if r.mine { "*" } else { "" })).collect::<Vec<_>>().join(" "),
    }
}

fn describe_label(rows: &[SeatRowLabel]) -> String {
    if rows.is_empty() {
        return "no menu".into();
    }
    rows.iter().map(|r| format!("{:?}{}{}", kind(r.kind), if r.occupied { "+" } else { "-" }, if r.me { "*" } else { "" })).collect::<Vec<_>>().join(" ")
}

#[test]
fn seat_menu_matches_labels() {
    if !mtc_dataset::recordings_present() {
        eprintln!("skipped: the recordings are not on this machine");
        return;
    }
    let frames: Vec<_> = labelled_frames().expect("dataset").into_iter().filter(|f| f.label.seats.is_some()).collect();
    let mut rows_ok = Tally::new("seat menu: rows, kinds and taken seats");
    let mut own_ok = Tally::new("seat menu: own seat, name learned");
    let mut own_unknown = Tally::new("seat menu: no own seat marked, name not learned");
    let mut sessions: Vec<&str> = frames.iter().map(|f| f.session.as_str()).collect();
    sessions.dedup();
    for session in sessions {
        let in_session: Vec<_> = frames.iter().filter(|f| f.session == session).collect();
        let mut reader = SeatReader::new();
        // Learn from the first frame with the player in the gunner's or loader's seat.
        let mut learned_from = None;
        for f in &in_session {
            let truth = f.label.seats.as_ref().expect("filtered");
            let Some(own) = truth.iter().find(|r| r.me && matches!(r.kind, LabelKind::Gunner | LabelKind::Loader)) else { continue };
            let image = f.image().expect("frame image");
            let view = FrameView::rgb(&image.data, image.width, image.height);
            let c = f.meta.geometry.client_in_frame();
            let client = (c.x, c.y, c.width as u32, c.height as u32);
            if let Some(menu) = reader.read(&view, client)
                && reader.learn(&view, &menu, kind(own.kind))
            {
                learned_from = Some(f.id());
                break;
            }
        }
        for f in &in_session {
            if learned_from.as_deref() == Some(f.id().as_str()) {
                continue;
            }
            let truth = f.label.seats.as_ref().expect("filtered");
            let image = f.image().expect("frame image");
            let view = FrameView::rgb(&image.data, image.width, image.height);
            let c = f.meta.geometry.client_in_frame();
            let client = (c.x, c.y, c.width as u32, c.height as u32);
            let menu = rows_ok.time(|| reader.read(&view, client));
            let read: Vec<(SeatKind, bool)> = menu.iter().flat_map(|m| m.rows.iter().map(|r| (r.kind, r.taken))).collect();
            let want: Vec<(SeatKind, bool)> = truth.iter().map(|r| (kind(r.kind), r.occupied)).collect();
            rows_ok.check(&f.id(), read == want, || format!("read {} vs {}", describe(&menu), describe_label(truth)));
            if read != want {
                continue;
            }
            let mine: Vec<bool> = menu.iter().flat_map(|m| m.rows.iter().map(|r| r.mine)).collect();
            if reader.own_known() {
                let want_mine: Vec<bool> = truth.iter().map(|r| r.me).collect();
                own_ok.check(&f.id(), mine == want_mine, || format!("read {} vs {}", describe(&menu), describe_label(truth)));
            } else {
                own_unknown.check(&f.id(), mine.iter().all(|m| !m), || format!("read {}", describe(&menu)));
            }
        }
    }
    rows_ok.require(1.0);
    own_ok.require(1.0);
    own_unknown.require(1.0);
}
