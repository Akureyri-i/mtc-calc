//! Composites the HUD (panel and aim strip, sample values) onto a recorded frame where it would
//! show live, to check how it sits with the game's UI in states not on screen right now:
//!   hud_composite <frame.png> <out.png> [full|empty|compact] [--background r,g,b] [--settings [impact]] [--hover x,y]
//! `--settings` opens the settings window over the frame (`impact`: tolerance by impact, a
//! least-move charge rule and two map label fields on); `--hover` highlights what is under that
//! window point.
//! `--background` repaints a black backdrop (a scope screenshot) in that colour first, rebuilding
//! the game's text over it with the game's drop shadow (2 px right and down, 35 % black).
//! The frame must be a full 2560×1368 window frame (title bar included) showing the angle readout
//! (gunner seat: panel and strip) or the loader's menu or bar (loader seat: panel only).
//! `--layout dpi,width,height,x,y` takes a frame of another display configuration (client size,
//! DPI, client origin in the frame; gunner seat only): read and drawn through its placement at its
//! scales, the loading menu as the HUD reads it there.
//! `--aim elevation,azimuth` fixes the selected target's angles (the strip and the panel's low arc),
//! so a run of frames shows the gun being laid on one target.
use std::path::Path;

use mtc_dataset::image::RgbImage;
use mtc_overlay::{
    MenuItem, MenuPick, PanelMenu, AimRow, AimStrip, Axis, PanelArc, PanelLoad, PanelSolution, PanelState, PanelTarget, hud_right, loader_panel_anchor, panel_anchor, render_offscreen,
};
use mtc_vision::loading::{LOADER_STRIP_BOTTOM, MenuSeat, menu_left, read_loader_bar, read_menu};
use mtc_vision::FrameView;
use mtc_vision::aim::{AimReader, AimResult};
use mtc_vision::layout::{Placement, UiLayout};

fn roblox_font_dir() -> std::path::PathBuf {
    let versions = std::path::PathBuf::from(std::env::var("LOCALAPPDATA").expect("LOCALAPPDATA")).join("Roblox/Versions");
    std::fs::read_dir(versions)
        .expect("Roblox versions")
        .filter_map(Result::ok)
        .map(|e| e.path().join("content/fonts"))
        .filter(|p| p.join("Montserrat-Medium.ttf").is_file())
        .max_by_key(|p| std::fs::metadata(p).and_then(|m| m.modified()).ok())
        .expect("Montserrat in a Roblox install")
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut image = RgbImage::load_png(Path::new(&args[0])).expect("frame");
    let full = args.get(2).is_none_or(|s| s == "full" || s == "compact" || s.starts_with("--"));
    let compact = args.get(2).is_some_and(|s| s == "compact");
    let aim: Option<(f64, f64)> = args.iter().position(|a| a == "--aim").and_then(|i| args.get(i + 1)).map(|v| {
        let (e, a) = v.split_once(',').expect("--aim elevation,azimuth");
        (e.trim().parse().expect("elevation"), a.trim().parse().expect("azimuth"))
    });
    let background = args.iter().position(|a| a == "--background").and_then(|i| args.get(i + 1)).map(|v| {
        let c: Vec<f32> = v.split(',').map(|x| x.parse().expect("r,g,b")).collect();
        [c[0], c[1], c[2]]
    });
    let placement = match args.iter().position(|a| a == "--layout").and_then(|i| args.get(i + 1)) {
        Some(v) => {
            let n: Vec<i32> = v.split(',').map(|x| x.parse().expect("dpi,width,height,x,y")).collect();
            Placement::new((n[3], n[4]), UiLayout { width: n[1] as u32, height: n[2] as u32, dpi: n[0] as u32 })
        }
        None => Placement::new((0, 34), UiLayout::REFERENCE),
    };
    let origin = placement.origin;
    let (strip_scale, hud_scale) = placement.layout.overlay_scales();
    let view = FrameView::rgb(&image.data, image.width, image.height);
    let mut reader = AimReader::with_scale(placement.layout.readout_text());
    let result = if placement.is_reference() {
        reader.read(&view, origin)
    } else {
        let area = AimReader::new().wanted_region(Placement::VIRTUAL_ORIGIN);
        reader.read(&placement.bottom_remap(&view, area).view(), Placement::VIRTUAL_ORIGIN)
    };
    let reading = match result {
        AimResult::Read(r) => Some(r),
        _ => None,
    };
    let (anchor_x, [(el, ev), (al, av)]) = placement.readout_rows(reading.as_ref().map_or(0, |r| r.anchor_x));
    // The loading menu read where the HUD reads it: off the reference from the region resampled to
    // the reference geometry, at the bottom origin.
    let bottom = placement.bottom_origin();
    let loading_area = {
        let (x, y, w, h) = mtc_vision::loading::LOADING_REGION;
        (bottom.0 + x, bottom.1 + y, w, h)
    };
    let remapped = (!placement.is_reference()).then(|| placement.bottom_remap(&view, loading_area));
    let loading_view = remapped.as_ref().map_or(view, |r| r.view());
    let menu = read_menu(&loading_view, bottom);
    let loader_slots = match (menu.clone(), if placement.is_reference() { read_loader_bar(&view, origin) } else { None }) {
        (Some(m), _) if m.seat != MenuSeat::Gunner => Some(m.cards.len()),
        (_, Some(bar)) => Some(bar.slots),
        _ => None,
    };
    let centre = origin.0 + placement.layout.width as i32 / 2;
    let right = reading.as_ref().map(|_| hud_right(anchor_x, centre, strip_scale, hud_scale));
    let anchor = match (right, loader_slots) {
        (Some(right), _) => panel_anchor(right, el, hud_scale),
        (None, Some(n)) => loader_panel_anchor(origin.0 + menu_left(n) as i32, origin.1 + LOADER_STRIP_BOTTOM),
        (None, None) => panic!("neither the angle readout nor the loader's menu or bar in the frame"),
    };
    let target = |label: &str, objective: bool, reachable: bool| PanelTarget { label: label.into(), objective, reachable };
    let panel = PanelState {
        anchor,
        targets: if full {
            vec![target("A1", false, true), target("A2", false, true), target("A3", false, false), target("A", true, true), target("B", true, true), target("C", true, true)]
        } else {
            vec![target("A", true, true), target("B", true, true), target("C", true, true)]
        },
        selected: full.then(|| "A2".into()),
        solution: full.then_some(PanelSolution {
            distance_m: 2069.4,
            height_m: 42.7,
            azimuth: aim.map_or(297.77, |(_, a)| a),
            low: Some(PanelArc { elevation: aim.map_or(12.98, |(e, _)| e), time_of_flight: 9.1, active: true, beyond_reach: false }),
            high: Some(PanelArc { elevation: 71.40, time_of_flight: 44.6, active: false, beyond_reach: true }),
        }),
        shell: full.then(|| "M107".into()),
        load: if full { PanelLoad::Loaded } else { PanelLoad::Empty },
        recommended: full.then(|| "Medium".into()),
        charge: full.then(|| "Low".into()),
        gun_set: true,
        notice: None,
        precision: Some((3.6, true)),
        compact,
        locked: !args.iter().any(|a| a == "--unlocked"),
        settings_open: args.iter().any(|a| a == "--settings"),
        scale: hud_scale,
        menu: menu_state(args.iter().position(|a| a == "--menu").and_then(|i| args.get(i + 1)).map(String::as_str)),
        shell_kind: full.then(|| "HE".into()),
        charge_menu: full.then(|| charge_state(args.iter().position(|a| a == "--charge").and_then(|i| args.get(i + 1)).map(String::as_str))),
    };
    // The shell's name as the list makes it: picked by hand shows the pick; no gun, unknown.
    let panel = match args.iter().position(|a| a == "--menu").and_then(|i| args.get(i + 1)).map(String::as_str) {
        Some("manual" | "manual-open") => PanelState { shell: Some("M110".into()), shell_kind: Some("SMOKE".into()), ..panel },
        Some("guns") => PanelState { shell: None, shell_kind: None, charge: None, recommended: None, solution: None, charge_menu: None, ..panel },
        _ => panel,
    };
    let strip = reading.as_ref().zip(right).map(|(r, right)| AimStrip {
        right,
        rows: [
            AimRow { axis: Axis::Elevation, current: r.elevation, target: aim.map_or(r.elevation + 0.64, |(e, _)| e), tolerance: 0.05, label_y: el, value_y: ev, beyond_reach: false },
            AimRow { axis: Axis::Azimuth, current: r.azimuth, target: aim.map_or(r.azimuth + 0.02, |(_, a)| a), tolerance: 0.05, label_y: al, value_y: av, beyond_reach: false },
        ],
        mirror: args.iter().any(|a| a == "--mirror"),
        scale: strip_scale,
    });
    if let Some(bg) = background {
        repaint(&mut image, bg);
    }
    let mut layers = render_offscreen(&roblox_font_dir(), strip.as_ref().filter(|_| full), Some(&panel), None).expect("render");
    // The settings window and the startup question stay above the other windows live.
    let mut on_top = Vec::new();
    if panel.settings_open {
        let impact = args.iter().position(|a| a == "--settings").and_then(|i| args.get(i + 1)).is_some_and(|a| a == "impact");
        let hover = args.iter().position(|a| a == "--hover").and_then(|i| args.get(i + 1)).map(|v| {
            let c: Vec<i32> = v.split(',').map(|x| x.parse().expect("x,y")).collect();
            (c[0], c[1])
        });
        let settings = mtc_overlay::SettingsState {
            centre: (image.width as i32 / 2, (image.height as i32 + origin.1) / 2),
            pred: !impact,
            mode: if impact { mtc_overlay::ToleranceMode::Impact } else { mtc_overlay::ToleranceMode::Angle },
            angle: (0.05, 0.05),
            impact: (5.0, 3.0),
            rule: if impact { mtc_overlay::ChargeRule::LeastMove } else { mtc_overlay::ChargeRule::NearestCharge },
            labels: mtc_overlay::LabelFields { time: impact, precision: impact, ..Default::default() },
            drag_offset_ms: if impact { -4 } else { 0 },
            limits: [(false, false), (false, false), (false, false)],
            share: impact,
            report_shots: impact,
            debug_map: impact,
            report_note: None,
            scale: (placement.layout.dpi as f32 / 144.0).min(1.0),
        };
        on_top.push(mtc_overlay::render_settings(&roblox_font_dir(), &settings, hover).expect("render settings"));
    }
    // The startup question about sending statistics (`--consent [ticked]`).
    if let Some(i) = args.iter().position(|a| a == "--consent") {
        let consent = mtc_overlay::ConsentState {
            centre: (image.width as i32 / 2, (image.height as i32 + origin.1) / 2),
            dont_ask: args.get(i + 1).is_some_and(|a| a == "ticked"),
            scale: (placement.layout.dpi as f32 / 144.0).min(1.0),
        };
        on_top.push(mtc_overlay::render_consent(&roblox_font_dir(), &consent, None).expect("render consent"));
    }
    // Charge marks on an open menu: reachable up to Medium on even cards, only High on odd ones.
    if let Some(menu) = menu.filter(|_| full) {
        use mtc_vision::loading::{CARD_HALF, Charge, charge_row_centre};
        let mut dots = Vec::new();
        for (i, card) in menu.cards.iter().enumerate() {
            for (k, charge) in [Charge::Low, Charge::Medium, Charge::High].into_iter().enumerate() {
                let reaches = if i % 2 == 0 { k <= 1 } else { k == 2 };
                let (x, y) = placement.bottom_to_frame((card.centre_x + CARD_HALF - 14.0, f64::from(bottom.1) + charge_row_centre(menu.seat, charge)));
                dots.push(mtc_overlay::ChargeDot { x: x as f32, y: y as f32, reaches });
            }
        }
        let (left, top) = (dots.iter().map(|d| d.x).fold(f32::MAX, f32::min) as i32 - 10, dots.iter().map(|d| d.y).fold(f32::MAX, f32::min) as i32 - 10);
        let right = dots.iter().map(|d| d.x).fold(f32::MIN, f32::max) as i32 + 11;
        let bottom = dots.iter().map(|d| d.y).fold(f32::MIN, f32::max) as i32 + 11;
        let marks = mtc_overlay::ChargeMarks { left, top, width: right - left, height: bottom - top, dots, scale: hud_scale };
        layers.push(mtc_overlay::render_charge_marks(&roblox_font_dir(), &marks).expect("render charge marks"));
    }
    layers.append(&mut on_top);
    for layer in &layers {
        for y in 0..layer.height as i32 {
            for x in 0..layer.width as i32 {
                let (fx, fy) = (layer.x + x, layer.y + y);
                if fx < 0 || fy < 0 || fx >= image.width as i32 || fy >= image.height as i32 {
                    continue;
                }
                let i = (y as usize * layer.width as usize + x as usize) * 4;
                let (b, g, r, a) = (layer.bgra[i], layer.bgra[i + 1], layer.bgra[i + 2], layer.bgra[i + 3]);
                let j = (fy as usize * image.width as usize + fx as usize) * 3;
                let keep = 255 - u32::from(a);
                for (k, src) in [r, g, b].into_iter().enumerate() {
                    image.data[j + k] = (u32::from(src) + (u32::from(image.data[j + k]) * keep + 127) / 255).min(255) as u8;
                }
            }
        }
    }
    image.save_png(Path::new(&args[1])).expect("save");
    match (&reading, loader_slots) {
        (Some(r), _) => println!("gunner seat: readout anchor x {}, elevation {:.2} azimuth {:.2}, {} layers", r.anchor_x, r.elevation, r.azimuth, layers.len()),
        (None, n) => println!("loader seat: {n:?} slots, {} layers", layers.len()),
    }
}

/// Repaints a black backdrop in `bg`: each pixel's brightness over black is the game text's
/// coverage; the text keeps its colour over `bg` and casts the game's shadow.
fn repaint(image: &mut RgbImage, bg: [f32; 3]) {
    let (w, h) = (image.width as usize, image.height as usize);
    let cov: Vec<f32> = image.data.chunks_exact(3).map(|p| f32::from(p[0].max(p[1]).max(p[2])) / 255.0).collect();
    let source = image.data.clone();
    for y in 0..h {
        for x in 0..w {
            let i = y * w + x;
            let c = cov[i];
            let shade = if x >= 2 && y >= 2 { 0.35 * cov[i - 2 * w - 2] } else { 0.0 };
            for k in 0..3 {
                let under = bg[k] * (1.0 - shade);
                image.data[i * 3 + k] = (f32::from(source[i * 3 + k]) + under * (1.0 - c)).min(255.0) as u8;
            }
        }
    }
}

/// The shell list in one of its states (`--menu closed|open|manual|manual-open|guns`): the M109's
/// shells, the slot read as M107; picked by hand as M110; or, the gun not recognised, the 155 mm guns.
fn menu_state(which: Option<&str>) -> Option<PanelMenu> {
    let item = |label: &str, detail: Option<&str>, current: bool, pick: MenuPick| MenuItem { label: label.into(), kind: None, detail: detail.map(str::to_owned), current, pick };
    let shells = |current: &str, manual: bool| {
        let mut items = vec![item("Auto", Some("read M107"), !manual, MenuPick::Auto)];
        for (name, kind) in [("M107", "HE"), ("M107 VT", "TIME FUZE"), ("M107 PF", "PROXIMITY FUZE"), ("M110", "SMOKE")] {
            items.push(MenuItem { kind: Some(kind.into()), ..item(name, None, manual && name == current, MenuPick::Shell(name.into())) });
        }
        items
    };
    Some(match which? {
        "closed" => PanelMenu { open: false, items: shells("M107", false), manual: false },
        "open" => PanelMenu { open: true, items: shells("M107", false), manual: false },
        "manual" => PanelMenu { open: false, items: shells("M110", true), manual: true },
        "manual-open" => PanelMenu { open: true, items: shells("M110", true), manual: true },
        "guns" => PanelMenu {
            open: true,
            items: vec![
                item("155mm M185", Some("M109"), false, MenuPick::Gun("155mm M185".into())),
                item("155mm M284", Some("M109A6"), false, MenuPick::Gun("155mm M284".into())),
                item("155mm L62", Some("PzH 2000"), false, MenuPick::Gun("155mm L62".into())),
                item("Other guns", None, false, MenuPick::More(3)),
            ],
            manual: false,
        },
        other => panic!("--menu {other}: closed, open, manual, manual-open or guns"),
    })
}

/// The charge list (`--charge closed|open|manual`): read Low; or set to High by hand.
fn charge_state(which: Option<&str>) -> PanelMenu {
    let manual = which == Some("manual");
    let mut items = vec![MenuItem { label: "Auto".into(), kind: None, detail: Some("read Low".into()), current: !manual, pick: MenuPick::Charge(None) }];
    for (i, name) in ["Low", "Medium", "High"].into_iter().enumerate() {
        items.push(MenuItem { label: name.into(), kind: None, detail: None, current: manual && i == 2, pick: MenuPick::Charge(Some(i as u8)) });
    }
    PanelMenu { open: which == Some("open"), items, manual }
}
