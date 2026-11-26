//! Loading UI on labelled frames: state (menu, countdown, neither), the menu's layout (gunner,
//! loader, on foot), each card's charge and charge bags, and the loaded slot (gunner's bar header,
//! loader's simplified bar). Frames labelled `bar` count as neither menu nor countdown; the
//! gunner's bar collapsed under the full map is not scored.
//!
//! Known misses: in the half second after the loading key, the fading menu dims the gunner's
//! highlighted header and the loader's bar is still sliding in (5 and 4 frames), and the menu
//! over the countdown hides it (2 frames). Needs the recorded frames (datasets/raw).
//!
//! The loading readers see only the HUD's read-back region (`LOADING_REGION`); the angle readout
//! (for the gunner's bar layout) is read on the whole frame.

use mtc_dataset::eval::{Tally, labelled_frames};
use mtc_dataset::labels::{CardChargeLabel, LoadingState, Seat};
use mtc_vision::FrameView;
use mtc_vision::aim::{AimReader, AimResult};
use mtc_vision::bar::BarReader;
use mtc_vision::loading::{CardCharge, Charge, LOADING_REGION, MenuSeat, ShellIcons, countdown_shown, gunner_bar_bags, gunner_bar_highlight, read_loader_bar, read_menu};

#[test]
fn loading_ui_matches_labels() {
    if !mtc_dataset::recordings_present() {
        eprintln!("skipped: the recordings are not on this machine");
        return;
    }
    let mut state = Tally::new("loading UI state (menu and layout, countdown, neither)");
    let mut cards = Tally::new("card charges (cards in correctly found menus)");
    let mut bags = Tally::new("card charge bags (cards in correctly found menus)");
    let mut gunner_slot = Tally::new("loaded slot in the gunner's bar");
    let mut loader_bar = Tally::new("loader's bar and its loaded slot");
    let mut bar_shell = Tally::new("loaded slot's charge bags and shell icon on the gunner's bar");
    let mut aim = AimReader::new();
    let bar = BarReader::new();
    let icons = ShellIcons::new();
    for frame in labelled_frames().expect("dataset") {
        let Some(truth) = &frame.label.loading else { continue };
        let image = frame.image().expect("frame image");
        let full = FrameView::rgb(&image.data, image.width, image.height);
        let o = frame.meta.geometry.client_in_frame();
        let origin = (o.x, o.y);
        let (rx, ry, rw, rh) = LOADING_REGION;
        let view = full.sub(origin.0 + rx, origin.1 + ry, rw as u32, rh as u32);
        let menu = read_menu(&view, origin);
        let countdown = countdown_shown(&view, origin);
        let case = frame.id();
        if truth.collapsed_bar_slot.is_none() {
            let label_x = match aim.read(&full, origin) {
                AimResult::Read(r) => Some(r.label_x),
                AimResult::Unreadable { label_x, .. } => Some(label_x),
                AimResult::Absent => None,
            };
            let layout = label_x.and_then(|x| bar.layout(&view, origin, x));
            let found = layout.as_ref().and_then(|l| gunner_bar_highlight(&view, origin, l)).map(|i| i as u32 + 1);
            gunner_slot.check(&case, found == truth.bar_slot, || format!("found {found:?}, label {:?}", truth.bar_slot));
            if let (Some(l), Some(slot), true) = (&layout, found, truth.bar_bags.is_some() || truth.bar_icon.is_some()) {
                let slot = slot as usize - 1;
                let bags = gunner_bar_bags(&view, origin, l, slot);
                let icon = icons.read(&view, origin, l, slot).map(|m| m.icon);
                let ok = truth.bar_bags.is_none_or(|b| b == bags) && truth.bar_icon.as_ref().is_none_or(|i| icon.as_ref() == Some(i));
                bar_shell.check(&case, ok, || format!("found {bags} bags, icon {icon:?}; label {:?} bags, icon {:?}", truth.bar_bags, truth.bar_icon));
            }
        }
        let found = read_loader_bar(&view, origin).map(|b| (b.slots as u32, b.loaded.map(|i| i as u32 + 1)));
        let want = truth.loader_bar.as_ref().map(|b| (b.slots, b.loaded));
        loader_bar.check(&case, found == want, || format!("found {found:?}, label {want:?}"));
        let seat = menu.as_ref().map(|m| match m.seat {
            MenuSeat::Gunner => Seat::Gunner,
            MenuSeat::Loader => Seat::Loader,
            MenuSeat::Dismounted => Seat::Dismounted,
        });
        let ok = match truth.state {
            LoadingState::None | LoadingState::Bar => menu.is_none() && !countdown,
            LoadingState::Countdown => menu.is_none() && countdown,
            LoadingState::Menu => !countdown && seat == truth.menu_seat && menu.as_ref().map(|m| m.cards.len()) == truth.cards.as_ref().map(Vec::len),
        };
        state.check(&case, ok, || format!("found menu {seat:?} with {:?} cards, countdown {countdown}; label {truth:?}", menu.as_ref().map(|m| m.cards.len())));
        if !ok || truth.state != LoadingState::Menu {
            continue;
        }
        let (Some(menu), Some(want)) = (&menu, &truth.cards) else { continue };
        if let Some(want_bags) = &truth.bags {
            for (i, (card, &want)) in menu.cards.iter().zip(want_bags).enumerate() {
                bags.check(&format!("{case} card {}", i + 1), card.bags == want, || format!("found {}, label {want}", card.bags));
            }
        }
        for (i, (card, want)) in menu.cards.iter().zip(want).enumerate() {
            // A card the label could not decide is not scored.
            let good = match (want, card.charge) {
                (CardChargeLabel::Hidden, _) => continue,
                (CardChargeLabel::None, CardCharge::None) => true,
                (CardChargeLabel::Low, CardCharge::Selected(Charge::Low)) => true,
                (CardChargeLabel::Medium, CardCharge::Selected(Charge::Medium)) => true,
                (CardChargeLabel::High, CardCharge::Selected(Charge::High)) => true,
                _ => false,
            };
            cards.check(&format!("{case} card {}", i + 1), good, || format!("found {:?}, label {want:?}", card.charge));
        }
    }
    for t in [&state, &cards, &bags, &gunner_slot, &loader_bar] {
        println!("{}", t.report());
    }
    state.require(0.98);
    cards.require(1.0);
    bags.require(1.0);
    gunner_slot.require(0.96);
    bar_shell.require(1.0);
    loader_bar.require(0.97);
}
