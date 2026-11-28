//! Composites the V1 plan panel (sample targets) onto a recorded V1 frame where it docks live,
//! to check it against the game's UI: plan_composite <frame.png> <out.png>. Also writes the
//! terrain section's states beside it (`<out>_<state>.png`).
use std::path::Path;

use mtc_dataset::image::RgbImage;
use mtc_overlay::{PanelLoad, PlanArc, PlanChargeBox, PlanDetail, PlanNext, PlanRow, PlanState, PlanTerrain, ProfileArc, TerrainNote, render_plan};

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

/// Sample ground along a 3467 m line: a slow rise to the target and a ridge `ridge` m high just
/// short of it.
fn ground(x: f32, ridge: f32) -> f32 {
    let d = 3467.0;
    18.4 * x / d + ridge * (-((x - 3250.0) / 120.0).powi(2)).exp() + 3.0 * (x / 97.0).sin()
}

/// A sample arc to (3467 m, 18.4 m) climbing `apex` m, cut where it meets the ground past 100 m
/// (`stop`) or at `end`.
fn arc(apex: f32, end: f32, charge: u8, used: bool, high: bool, ridge: f32) -> ProfileArc {
    let d = 3467.0;
    let mut points = Vec::new();
    let mut stopped = None;
    for i in 0..=400 {
        let x = end * i as f32 / 400.0;
        let u = x / d;
        let y = 4.0 * apex * u * (1.0 - u) + 18.4 * u;
        points.push((x, y));
        if x > 100.0 && y < ground(x, ridge) - 2.5 && x < d - 70.0 {
            stopped = Some((x, ground(x, ridge)));
            break;
        }
    }
    ProfileArc { points, high, charge: Some(charge), used, stopped }
}

fn composite(image: &mut RgbImage, layer: &mtc_overlay::Layer) {
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

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut image = RgbImage::load_png(Path::new(&args[0])).expect("frame");
    // Sample data from the panel's design mockup.
    let row = |label: &str, objective: bool, d: f64, charge: Option<(&str, u8)>, t: Option<f64>, precision: Option<(f32, bool)>| PlanRow {
        label: label.into(),
        objective,
        distance_m: Some(d),
        charge: charge.map(|(n, i)| (n.into(), i)),
        time_s: t,
        high_arc: false,
        reachable: t.is_some(),
        precision,
    };
    let (low, medium, high) = (("Low", 0u8), ("Medium", 1u8), ("High", 2u8));
    // Picked by hand in the shell list: the M110 smoke shell.
    let manual = matches!(args.iter().position(|a| a == "--menu").and_then(|i| args.get(i + 1)).map(String::as_str), Some("manual" | "manual-open"));
    let boxed = |name: &str, index: u8, band: (f32, f32), t: Option<f64>, loaded: bool, planned: bool| PlanChargeBox { name: name.into(), index, band_km: Some(band), time_s: t, loaded, planned };
    // Docked as live: 64 px right of the full map (client x 839 + 882), top on its top (y 226).
    let state = PlanState {
        at: (839 + 882 + 64, 34 + 226),
        rows: vec![
            row("A1", false, 3936.0, Some(high), Some(13.5), Some((0.9, false))),
            row("A2", false, 2156.0, Some(low), Some(14.3), Some((0.9, false))),
            row("A3", false, 3467.0, Some(medium), Some(15.3), Some((6.4, true))),
            row("A4", false, 3564.0, Some(medium), Some(15.7), Some((1.1, false))),
            row("A5", false, 3236.0, Some(medium), Some(14.4), Some((1.4, false))),
            row("A", true, 1204.0, Some(low), Some(8.5), None),
            row("B", true, 1374.0, Some(low), Some(9.5), None),
            row("C", true, 1987.0, Some(low), Some(13.2), None),
            row("D", true, 2425.0, Some(medium), Some(11.1), None),
            row("E", true, 3413.0, Some(medium), Some(15.1), None),
            row("F", true, 3484.0, Some(medium), Some(15.4), None),
        ],
        selected: Some("A3".into()),
        hover: None,
        shell: Some(if manual { "M110" } else { "M107" }.into()),
        loaded_charge: Some(("Low".into(), 0)),
        shell_kind: Some(if manual { "SMOKE" } else { "HE" }.into()),
        menu: menu_state(args.iter().position(|a| a == "--menu").and_then(|i| args.get(i + 1)).map(String::as_str)),
        charge_manual: args.iter().any(|a| a == "--menu").then(|| args.iter().any(|a| a == "--charge-manual")),
        load: PanelLoad::Loaded,
        next: PlanNext::Load("Medium".into(), 1),
        detail: Some(PlanDetail {
            label: "A3".into(),
            height_m: 18.4,
            azimuth: 194.47,
            precision: Some((6.4, true)),
            blast_m: Some((49.7, 20.7)),
            low: Some(PlanArc { elevation: 24.61, time_s: 14.2, beyond_reach: false, blocked_m: None, hint_m: None }),
            high: Some(PlanArc { elevation: 66.93, time_s: 33.8, beyond_reach: false, blocked_m: None, hint_m: None }),
            high_chosen: false,
            auto: true,
            charges: vec![
                boxed("Low", 0, (0.3, 2.4), None, true, false),
                boxed("Medium", 1, (0.9, 3.6), Some(14.2), false, true),
                boxed("High", 2, (1.6, 5.0), Some(10.9), false, false),
            ],
            terrain: None,
        }),
        gun_set: true,
        gun_source: Some(format!("{} · ±0.9 m", mtc_overlay::lang::t("gun.full_map"))),
        notice: None,
        terrain_open: false,
        settings_open: false,
        targets_open: true,
        squad: None,
        scale: 1.0,
    };
    let fonts = roblox_font_dir();
    let frame = image.clone();
    let layer = render_plan(&fonts, &state).expect("render");
    composite(&mut image, &layer);
    image.save_png(Path::new(&args[1])).expect("save");
    if args.iter().any(|a| a == "--menu") {
        return;
    }

    // The terrain section's states, on the same frame.
    let d = 3467.0;
    let end = d * 1.06;
    let line = |ridge: f32| -> Vec<(f32, f32)> { (0..=160).map(|i| end * i as f32 / 160.0).map(|x| (x, ground(x, ridge))).collect() };
    const RIDGE: f32 = 110.0;
    let terrain = |arcs: Vec<ProfileArc>, note: TerrainNote| PlanTerrain { distance_m: d, target_m: 18.4, ground: line(RIDGE), arcs, skims: None, note };
    let open_ground = |arcs: Vec<ProfileArc>, note: TerrainNote| PlanTerrain { distance_m: d, target_m: 18.4, ground: line(12.0), arcs, skims: None, note };
    let with = |high_chosen: bool, auto: bool, low_blocked: Option<f32>, high_blocked: Option<f32>, terrain: PlanTerrain, open: bool| {
        let mut s = state.clone();
        let detail = s.detail.as_mut().expect("detail");
        detail.high_chosen = high_chosen;
        detail.auto = auto;
        detail.low.as_mut().expect("low").blocked_m = low_blocked;
        detail.high.as_mut().expect("high").blocked_m = high_blocked;
        detail.terrain = Some(terrain);
        s.terrain_open = open;
        s
    };
    let low_stopped = arc(400.0, end, 1, false, false, RIDGE);
    let low_at = low_stopped.stopped.expect("the sample ridge stops the low arc").0;
    let states = [
        ("targets_folded", PlanState { targets_open: false, settings_open: true, ..state.clone() }),
        ("clear_closed", with(false, true, None, None, open_ground(vec![arc(400.0, d, 1, true, false, 12.0)], TerrainNote::Clear), false)),
        ("clear_open", with(false, true, None, None, open_ground(vec![arc(400.0, d, 1, true, false, 12.0)], TerrainNote::Clear), true)),
        ("other_arc", with(true, true, Some(low_at), None, terrain(vec![arc(1900.0, d, 1, true, true, RIDGE), low_stopped.clone()], TerrainNote::OtherArc { high: false, at_m: low_at, auto: true, hard: true }), true)),
        ("skims", {
            // A crest 95 m high: the low arc passes 1.3 m under it, within the barrel's height.
            let skim = PlanTerrain { distance_m: d, target_m: 18.4, ground: line(95.0), arcs: vec![arc(400.0, d, 1, true, false, 95.0)], skims: Some((3250.0, ground(3250.0, 95.0))), note: TerrainNote::Skims { high: false, at_m: 3250.0 } };
            with(false, true, None, None, skim, true)
        }),
        ("loaded_charge", {
            let flat = arc(60.0, end, 2, false, false, RIDGE);
            let at = flat.stopped.map_or(1200.0, |s| s.0);
            // High is the loaded charge here (the note says so).
            let mut s = with(true, true, Some(low_at), None, terrain(vec![arc(1900.0, d, 1, true, true, RIDGE), flat, low_stopped.clone()], TerrainNote::LoadedCharge { name: "High".into(), index: 2, at_m: at }), true);
            s.loaded_charge = Some(("High".into(), 2));
            s
        }),
        ("used_chosen", with(false, false, Some(low_at), None, terrain(vec![low_stopped.clone()].into_iter().map(|mut a| { a.used = true; a }).collect(), TerrainNote::Used { high: false, at_m: low_at, auto: false, hard: true }), true)),
        ("used_auto", with(false, true, Some(low_at), Some(3200.0), terrain(vec![low_stopped.clone()].into_iter().map(|mut a| { a.used = true; a }).collect(), TerrainNote::Used { high: false, at_m: low_at, auto: true, hard: true }), true)),
        ("hint", {
            // A tree the low arc passes through: only a hint, the low arc stays chosen.
            let mut s = with(false, true, None, None, terrain(vec![low_stopped.clone()].into_iter().map(|mut a| { a.used = true; a }).collect(), TerrainNote::Used { high: false, at_m: low_at, auto: false, hard: false }), true);
            s.detail.as_mut().expect("detail").low.as_mut().expect("low").hint_m = Some(low_at);
            s
        }),
        ("many_rows", {
            let mut s = with(true, true, Some(low_at), None, terrain(vec![arc(1900.0, d, 1, true, true, RIDGE), low_stopped.clone()], TerrainNote::OtherArc { high: false, at_m: low_at, auto: true, hard: true }), true);
            let extra: Vec<PlanRow> = (6..=12).map(|k| row(&format!("A{k}"), false, 2000.0 + 100.0 * k as f64, Some(medium), Some(12.0), Some((1.0, false)))).collect();
            s.rows.splice(5..5, extra);
            s
        }),
    ];
    let stem = Path::new(&args[1]).with_extension("");
    for (name, st) in states {
        let mut image = frame.clone();
        let layer = render_plan(&fonts, &st).expect("render");
        composite(&mut image, &layer);
        image.save_png(Path::new(&format!("{}_{name}.png", stem.display()))).expect("save");
    }
}

/// The shell list in one of its states (`--menu closed|open|manual-open|manual`), as the gunner's
/// panel shows it: the M109's shells, the slot read as M107, or picked by hand as M110.
fn menu_state(which: Option<&str>) -> Option<mtc_overlay::PanelMenu> {
    use mtc_overlay::{MenuItem, MenuPick, PanelMenu};
    let shells = |manual: bool| {
        let mut items = vec![MenuItem { label: "Auto".into(), kind: None, detail: Some("read M107".into()), current: !manual, pick: MenuPick::Auto }];
        for (name, kind) in [("M107", "HE"), ("M107 VT", "TIME FUZE"), ("M107 PF", "PROXIMITY FUZE"), ("M110", "SMOKE")] {
            items.push(MenuItem { label: name.into(), kind: Some(kind.into()), detail: None, current: manual && name == "M110", pick: MenuPick::Shell(name.into()) });
        }
        items
    };
    Some(match which? {
        "closed" => PanelMenu { open: false, items: shells(false), manual: false },
        "open" => PanelMenu { open: true, items: shells(false), manual: false },
        "manual-open" => PanelMenu { open: true, items: shells(true), manual: true },
        "manual" => PanelMenu { open: false, items: shells(true), manual: true },
        other => panic!("--menu {other}: closed, open, manual-open or manual"),
    })
}
