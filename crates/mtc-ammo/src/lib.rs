//! Ammunition data from the MTC data service (`shells.json`) and the
//! matching of what the loading UI shows against it.
//!
//! - Weapon: the label ("155 mm M284", "227mm M269") names calibre and model; the data names
//!   them "155mm M284". Matched on the calibre's whole number and the model.
//! - Shell: a card shows a name prefix (often cut, "M107 High..."), the muzzle velocity at the
//!   selected charge and whether it has charges. Candidates are the weapon's shells starting with
//!   the prefix, with that velocity, and with the selected charge in their name ("High Charge") —
//!   or, for a card without charges, without one. Shells left over that fly alike (HE, VT and PF
//!   fuzes) are one ballistic choice.
//!
//! A shell's physics convert to the ballistics model's `Projectile` as the upstream calculator's
//! `fetchGuns.ts` does (explosive mass rounded to 4 decimals, zero drag left out).
//!
//! Text from the screen is read imperfectly: look-alike glyphs (5/S, 1/I, 0/O, 8/B, 2/Z) count as
//! equal, `?` (unread) matches anything, and names are matched by edit distance: the closest
//! candidates win, within about one error per four glyphs (`MAX_ERROR_SHARE`).

use std::collections::BTreeMap;

pub mod fetch;

use serde::Deserialize;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChargeLevel {
    Low,
    Medium,
    High,
}

impl ChargeLevel {
    pub const ALL: [ChargeLevel; 3] = [Self::Low, Self::Medium, Self::High];

    pub fn word(self) -> &'static str {
        match self {
            Self::Low => "Low Charge",
            Self::Medium => "Medium Charge",
            Self::High => "High Charge",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct Ballistics {
    pub gravity: f64,
    pub lifetime: f64,
    #[serde(rename = "dragConstant", default)]
    pub drag: f64,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct Explosive {
    /// kg.
    pub mass: f64,
    /// Blast reach (m): the explosion damages within it. The upstream calculator draws it in
    /// metres, and its formula from `mass` gives the same value (M107: 49.7 m).
    #[serde(default)]
    pub radius: f64,
    /// Within this (m) the blast kills.
    #[serde(rename = "killRadius", default)]
    pub kill_radius: f64,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Cluster {
    pub submunitions: f64,
    #[serde(default)]
    pub deploy_factor: f64,
    /// Full spread angle, degrees.
    #[serde(default)]
    pub dispersion: f64,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct Shell {
    pub name: String,
    /// Muzzle velocity, m/s.
    pub velocity: f64,
    #[serde(rename = "type", default)]
    pub kind: String,
    pub ballistics: Ballistics,
    #[serde(default)]
    pub explosive: Option<Explosive>,
    #[serde(default)]
    pub cluster: Option<Cluster>,
    /// Spread of a rocket (or an automatic grenade launcher's grenade); howitzer and mortar
    /// shells have none. A weapon with it loads a whole pod or belt at once and, emptied, needs
    /// resupply.
    #[serde(default)]
    pub dispersion: Option<f64>,
}

impl Shell {
    /// Fired in salvos from a pod loaded at once (rockets): emptied, the weapon needs resupply,
    /// not another loading.
    pub fn salvo(&self) -> bool {
        self.dispersion.is_some_and(|d| d > 0.0)
    }

    /// The charge named in the shell's name, if any.
    pub fn charge(&self) -> Option<ChargeLevel> {
        ChargeLevel::ALL.into_iter().find(|c| self.name.contains(c.word()))
    }

    /// The name without its charge ("M107 High Charge VT" → "M107 VT").
    pub fn base_name(&self) -> String {
        match self.charge() {
            Some(c) => self.name.replacen(c.word(), "", 1).split_whitespace().collect::<Vec<_>>().join(" "),
            None => self.name.clone(),
        }
    }

    /// The physics for the ballistics model, as the upstream calculator builds it.
    pub fn projectile(&self) -> mtc_ballistics::Projectile {
        let nonzero = |v: f64| (v != 0.0).then_some(v);
        mtc_ballistics::Projectile {
            velocity: self.velocity,
            gravity: Some(self.ballistics.gravity),
            drag_constant: nonzero(self.ballistics.drag),
            lifetime: Some(self.ballistics.lifetime),
            submunitions: self.cluster.as_ref().map(|c| c.submunitions),
            submunition_angular_dispersion: self.cluster.as_ref().and_then(|c| nonzero(c.dispersion)),
            submunition_deploy_factor: self.cluster.as_ref().and_then(|c| nonzero(c.deploy_factor)),
            explosive_mass: self.explosive.as_ref().map(|e| (e.mass * 1e4).round() / 1e4),
        }
    }
}

#[derive(Deserialize)]
struct File {
    data: BTreeMap<String, Game>,
}

#[derive(Deserialize)]
struct Game {
    data: BTreeMap<String, Vec<Shell>>,
}

pub struct AmmoData {
    /// Weapon data name ("155mm M284") to its shells.
    weapons: BTreeMap<String, Vec<Shell>>,
}

/// Look-alike glyphs read from the screen.
fn glyph_class(c: char) -> char {
    match c {
        '5' | 'S' | 's' => 'S',
        '1' | 'I' | 'l' | 'i' | '|' => 'I',
        '0' | 'O' | 'o' => 'O',
        '8' | 'B' => 'B',
        '2' | 'Z' | 'z' => 'Z',
        c => c,
    }
}

/// Errors allowed per glyph of the text read.
const MAX_ERROR_SHARE: f64 = 0.25;

fn same_glyph(a: char, b: char) -> bool {
    a == '?' || b == '?' || glyph_class(a) == glyph_class(b)
}

/// Edit distance between `read` and the start of `name` (spaces ignored on both sides: a narrow
/// space can go unread): the fewest edits turning `read` into some prefix of `name`.
fn prefix_distance(name: &str, read: &str) -> usize {
    let n: Vec<char> = name.chars().filter(|c| *c != ' ').collect();
    let r: Vec<char> = read.chars().filter(|c| *c != ' ').collect();
    // d[j]: distance between the glyphs read so far and n[..j].
    let mut d: Vec<usize> = (0..=n.len()).collect();
    for (i, &rc) in r.iter().enumerate() {
        let mut next = vec![i + 1; n.len() + 1];
        for j in 1..=n.len() {
            let sub = d[j - 1] + usize::from(!same_glyph(rc, n[j - 1]));
            next[j] = sub.min(d[j] + 1).min(next[j - 1] + 1);
        }
        d = next;
    }
    d.into_iter().min().unwrap_or(0)
}

/// Edit distance between the glyphs read and the whole of `name` (spaces ignored, look-alike
/// glyphs equal), as `prefix_distance` without stopping early.
fn whole_distance(name: &str, read: &str) -> usize {
    let n: Vec<char> = name.chars().filter(|c| *c != ' ').collect();
    let r: Vec<char> = read.chars().filter(|c| *c != ' ').collect();
    let mut d: Vec<usize> = (0..=n.len()).collect();
    for (i, &rc) in r.iter().enumerate() {
        let mut next = vec![i + 1; n.len() + 1];
        for j in 1..=n.len() {
            let sub = d[j - 1] + usize::from(!same_glyph(rc, n[j - 1]));
            next[j] = sub.min(d[j] + 1).min(next[j - 1] + 1);
        }
        d = next;
    }
    d[n.len()]
}

/// A shell's name with a trailing charge dropped ("M107 High Charge" → "M107"), as some cards
/// show it.
fn shown_name(shell: &Shell) -> String {
    ChargeLevel::ALL.into_iter().find_map(|c| shell.name.strip_suffix(c.word())).map_or_else(|| shell.name.clone(), |s| s.trim().to_owned())
}

fn allowed_errors(read: &str) -> usize {
    ((read.chars().filter(|c| *c != ' ').count() as f64) * MAX_ERROR_SHARE).ceil() as usize
}

/// Look-alike letters read as digits.
fn as_digits(text: &str) -> String {
    text.chars()
        .map(|c| match glyph_class(c) {
            'S' => '5',
            'I' => '1',
            'O' => '0',
            'B' => '8',
            'Z' => '2',
            c => c,
        })
        .collect()
}

/// Calibre and model of a weapon name or label: "155 mm M284", "155mm M284", "152.4 mm 2A64",
/// and the data's model-first names, "L9A1 165mm" (the screen shows "165 mm L9A1").
fn calibre_and_model(text: &str) -> Option<(u32, String)> {
    let tokens: Vec<&str> = text.split_whitespace().collect();
    if let [model, calibre] = tokens.as_slice()
        && let Some(digits) = calibre.strip_suffix("mm")
        && !model.ends_with("mm")
        && let Ok(c) = as_digits(digits).split('.').next().unwrap_or("").parse::<u32>()
    {
        return Some((c, (*model).to_owned()));
    }
    let calibre: String = as_digits(text).trim_start().chars().take_while(char::is_ascii_digit).collect();
    let model = tokens.last()?.to_string();
    // The model is the token after "mm" ("227mm M269" has none between).
    if model.ends_with("mm") || calibre.is_empty() {
        return None;
    }
    Some((calibre.parse().ok()?, model))
}

/// Whether two weapon labels as read name the same weapon: the same calibre and models within the
/// read tolerance (a misread letter is still the same label); labels without a calibre and model
/// only when equal.
pub fn same_weapon_label(a: &str, b: &str) -> bool {
    match (calibre_and_model(a), calibre_and_model(b)) {
        (Some((ca, ma)), Some((cb, mb))) => {
            let distance = prefix_distance(&ma, &mb) + ma.chars().count().saturating_sub(mb.chars().count());
            ca == cb && distance <= allowed_errors(&ma).max(allowed_errors(&mb))
        }
        _ => a.trim() == b.trim(),
    }
}

/// Whether two labels show the same calibre ("220 mm T6" and "220 mm TOS-1A").
pub fn same_calibre(a: &str, b: &str) -> bool {
    matches!((calibre_and_model(a), calibre_and_model(b)), (Some((ca, _)), Some((cb, _))) if ca == cb)
}

/// Whether the label `read` is `current` cut off: the same calibre and a model that is the start of
/// `current`'s and shorter (the 2S33's "152.4 mm 2A64" read as "152.4 mm 2" while its loading
/// animates, seen 7 times in one match). Such a read is not another weapon.
pub fn cut_off_label(read: &str, current: &str) -> bool {
    match (calibre_and_model(read), calibre_and_model(current)) {
        (Some((cr, mr)), Some((cc, mc))) => cr == cc && mr.chars().count() < mc.chars().count() && mc.starts_with(&mr),
        _ => false,
    }
}

/// The artillery weapons: every weapon the wiki's vehicle and placeable data flag `artillery` (the
/// flag the original calculator selects by, `fetchGuns.ts`), with the elevation range of the
/// turrets that carry it. Built from the wiki data downloaded at run time (`fetch`), else the copy
/// compiled in (`data/artillery_weapons.json`, built the same way).
#[derive(Clone, Debug, PartialEq, Deserialize, serde::Serialize)]
pub struct WeaponTable {
    pub weapons: Vec<ArtilleryWeapon>,
}

#[derive(Clone, Debug, PartialEq, Deserialize, serde::Serialize)]
pub struct ArtilleryWeapon {
    pub name: String,
    /// The turrets' vertical traverse (degrees, relative to the hull; the widest over all mounts).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wiki_elevation: Option<(f64, f64)>,
}

static WEAPON_TABLE: std::sync::OnceLock<WeaponTable> = std::sync::OnceLock::new();

impl WeaponTable {
    /// The table compiled into the program.
    pub fn compiled() -> Self {
        serde_json::from_str(include_str!("../../../data/artillery_weapons.json")).expect("data/artillery_weapons.json")
    }

    /// The table from the wiki's `vehicles.json` and `placeables.json` (towed guns, mortars):
    /// every `Weapon` module with a name, flagged artillery
    /// in any entity (the D-30 is flagged in one and not another), with the min and max of the
    /// vertical traverse of every `Turret` module that lists it.
    pub fn from_wiki(sources: &[&str]) -> Result<Self, String> {
        use serde_json::Value;
        fn modules<'a>(node: &'a Value, out: &mut Vec<&'a serde_json::Map<String, Value>>) {
            match node {
                Value::Object(map) => {
                    if let Some(Value::Object(m)) = map.get("modules") {
                        out.push(m);
                    }
                    map.values().for_each(|v| modules(v, out));
                }
                Value::Array(list) => list.iter().for_each(|v| modules(v, out)),
                _ => {}
            }
        }
        // Python's truth, as the tool reads the flag.
        fn truthy(v: Option<&Value>) -> bool {
            match v {
                None | Some(Value::Null) => false,
                Some(Value::Bool(b)) => *b,
                Some(Value::Number(n)) => n.as_f64() != Some(0.0),
                Some(Value::String(s)) => !s.is_empty(),
                Some(Value::Array(a)) => !a.is_empty(),
                Some(Value::Object(o)) => !o.is_empty(),
            }
        }
        fn weapon_name(module: &Value) -> Option<String> {
            (module.get("type")?.as_str()? == "Weapon").then_some(())?;
            Some(module.get("data")?.get("name")?.as_str()?.to_string()).filter(|n| !n.is_empty())
        }
        let mut flagged = BTreeMap::<String, bool>::new();
        let mut limits = BTreeMap::<String, (f64, f64)>::new();
        for source in sources {
            let root: Value = serde_json::from_str(source).map_err(|e| e.to_string())?;
            let mut found = Vec::new();
            modules(&root, &mut found);
            for table in found {
                for module in table.values() {
                    if let Some(name) = weapon_name(module) {
                        *flagged.entry(name).or_default() |= truthy(module["data"].get("artillery"));
                    }
                }
                for module in table.values() {
                    if module.get("type").and_then(Value::as_str) != Some("Turret") {
                        continue;
                    }
                    let vertical = &module["data"]["traverse"]["vertical"];
                    let (Some(lo), Some(hi)) = (vertical["min"].as_f64(), vertical["max"].as_f64()) else { continue };
                    for weapon in module["data"]["weapons"].as_array().into_iter().flatten() {
                        let Some(name) = weapon.as_str().and_then(|r| table.get(r)).and_then(weapon_name) else { continue };
                        let range = limits.entry(name).or_insert((lo, hi));
                        *range = (range.0.min(lo), range.1.max(hi));
                    }
                }
            }
        }
        let weapons: Vec<ArtilleryWeapon> = flagged
            .into_iter()
            .filter(|(_, artillery)| *artillery)
            .map(|(name, _)| ArtilleryWeapon { wiki_elevation: limits.get(&name).copied(), name })
            .collect();
        if weapons.is_empty() {
            return Err("no weapon flagged artillery".into());
        }
        Ok(Self { weapons })
    }

    /// Makes this the table the program goes by. Done once at start, before anything asks; `false`
    /// when the compiled one was already in use.
    pub fn install(self) -> bool {
        WEAPON_TABLE.set(self).is_ok()
    }

    /// The table in use: the installed one, else the compiled one.
    pub fn current() -> &'static WeaponTable {
        WEAPON_TABLE.get_or_init(Self::compiled)
    }
}

/// Whether `weapon` (the data's name, as decided from the label and the cards) is an artillery
/// weapon (`WeaponTable`). Tank guns share the game's loading menu and are in the shell data too,
/// but get no HUD. An exact name: a misread label never decides it (one read as `?,9c? mm M3` once
/// passed a looser match).
pub fn is_artillery_weapon(weapon: &str) -> bool {
    WeaponTable::current().weapons.iter().any(|w| w.name == weapon)
}

/// Mortars whose wiki range counts from a resting angle, not from level (`60mm M2`: wiki −12.5..32.5,
/// readout 40..85): only a range checked against the readout serves for them.
const RELATIVE_RANGE: [&str; 3] = ["60mm M2", "120mm M120", "60 mm Soltam Mortar"];

/// The elevation range of `weapon` in degrees, as the game's readout shows it on flat ground.
/// `weapon` is the data's name as decided from the label and the cards (a converted 2S19's
/// `152mm 2A64M` shows the same label as `152mm 2A64`, in the same turret). The range comes from
/// `data/elevation_limits.json` (checked against the readout), else the wiki's turret range
/// (`WeaponTable`); the four guns checked read exactly the wiki's range (M284, 2A64, L62, D-30).
/// Launchers whose boxes meet the hull at some headings (M270, BM-21) and the 2S7 have no single
/// range to measure; for them the HUD goes by the wiki's maximum, the most the gun can reach.
pub fn elevation_limits(weapon: &str) -> Option<(f64, f64)> {
    static LIMITS: std::sync::OnceLock<Vec<(String, (f64, f64))>> = std::sync::OnceLock::new();
    #[derive(Deserialize)]
    struct Entry {
        name: String,
        elevation: (f64, f64),
    }
    #[derive(Deserialize)]
    struct List {
        weapons: Vec<Entry>,
    }
    let limits = LIMITS.get_or_init(|| {
        serde_json::from_str::<List>(include_str!("../../../data/elevation_limits.json")).expect("data/elevation_limits.json").weapons.into_iter().map(|e| (e.name, e.elevation)).collect()
    });
    if let Some((_, range)) = limits.iter().find(|(name, _)| name == weapon) {
        return Some(*range);
    }
    WeaponTable::current().weapons.iter().filter(|w| !RELATIVE_RANGE.contains(&w.name.as_str())).find(|w| w.name == weapon).and_then(|w| w.wiki_elevation)
}


/// Where `weapon`'s shell leaves the gun relative to the player's arrow (`data/launch_points.json`),
/// and the measured weapon it was taken from when not measured itself (`None`: measured).
/// Mortars take the 60 mm mortar's, other emplaced weapons the D-30's, the rest the M109A6's.
pub fn launch_point(weapon: &str) -> (mtc_ballistics::LaunchPoint, Option<&'static str>) {
    #[derive(Deserialize)]
    struct Measured {
        weapon: String,
        height_m: f64,
        ahead_m: f64,
    }
    #[derive(Deserialize)]
    struct Table {
        measured: Vec<Measured>,
        like: std::collections::HashMap<String, String>,
        default: String,
    }
    static TABLE: std::sync::OnceLock<Table> = std::sync::OnceLock::new();
    let table = TABLE.get_or_init(|| serde_json::from_str(include_str!("../../../data/launch_points.json")).expect("data/launch_points.json"));
    let point = |name: &str| table.measured.iter().find(|m| m.weapon == name).map(|m| mtc_ballistics::LaunchPoint { height_m: m.height_m, ahead_m: m.ahead_m });
    if let Some(p) = point(weapon) {
        return (p, None);
    }
    let from = table.like.get(weapon).unwrap_or(&table.default);
    (point(from).expect("launch_points.json: like/default names a measured weapon"), Some(from.as_str()))
}

/// The muzzle velocity in a card's value text ("684 m/s", digits possibly read as look-alikes).
pub fn velocity_in(text: &str) -> Option<u32> {
    let digits: String = as_digits(text).trim_start().chars().take_while(char::is_ascii_digit).collect();
    digits.parse().ok()
}

impl AmmoData {
    /// Parses the data service's `shells.json`.
    pub fn from_json(json: &str) -> serde_json::Result<Self> {
        let file: File = serde_json::from_str(json)?;
        let weapons = file.data.into_values().flat_map(|g| g.data).collect();
        Ok(Self { weapons })
    }

    pub fn shells(&self, weapon: &str) -> &[Shell] {
        self.weapons.get(weapon).map_or(&[], Vec::as_slice)
    }

    /// The weapon a label on screen names.
    pub fn weapon_for_label(&self, label: &str) -> Option<&str> {
        let (calibre, model) = calibre_and_model(label)?;
        // Same calibre; the model closest by edit distance (the whole model, not a prefix).
        let scored: Vec<(usize, &str)> = self
            .weapons
            .keys()
            .filter_map(|name| {
                let (c, m) = calibre_and_model(name)?;
                let distance = prefix_distance(&m, &model) + m.chars().count().saturating_sub(model.chars().count());
                (c == calibre).then_some((distance, name.as_str()))
            })
            .collect();
        let best = scored.iter().map(|s| s.0).min()?;
        let winners: Vec<&str> = scored.iter().filter(|s| s.0 == best).map(|s| s.1).collect();
        (best <= allowed_errors(&model) && winners.len() == 1).then(|| winners[0])
    }

    /// The weapon a label names, told apart by the cards when the label fits more than one: a
    /// conversion swaps a weapon for one a letter apart (2S19: `152mm 2A64` ↔ `152mm 2A64M`), and
    /// the card names (`cards`, read prefixes) are in only one of them. Weapons of the label's
    /// calibre within one error more than `weapon_for_label` allows compete; the one with the most
    /// cards among its shells wins, then the closest model.
    pub fn weapon_for_label_and_cards(&self, label: &str, cards: &[&str]) -> Option<&str> {
        let (calibre, model) = calibre_and_model(label)?;
        let allowed = allowed_errors(&model) + 1;
        let mut scored: Vec<(usize, usize, &str)> = self
            .weapons
            .iter()
            .filter_map(|(name, shells)| {
                let (c, m) = calibre_and_model(name)?;
                let distance = prefix_distance(&m, &model) + m.chars().count().saturating_sub(model.chars().count());
                let hits = cards
                    .iter()
                    .map(|card| card.trim_end_matches(['.', ' ']).trim())
                    .filter(|card| shells.iter().any(|s| prefix_distance(&s.name, card) <= allowed_errors(card)))
                    .count();
                (c == calibre).then_some((hits, distance, name.as_str()))
            })
            .collect();
        // Cards that fit another weapon of the calibre and none of a candidate's shells say it is not
        // that weapon: a label misread short fits one by its model's prefix alone ("155 mm L" for
        // "155 mm M284" took the L62 over the M284's own M107 and M110 cards at 100 %, 1366x768).
        // Cards that fit no weapon (shells the data lacks) leave the label to decide.
        let cards_fit = scored.iter().any(|s| s.0 > 0);
        scored.retain(|&(hits, distance, _)| distance <= allowed && (hits > 0 || !cards_fit));
        scored.sort_by_key(|&(hits, distance, _)| (std::cmp::Reverse(hits), distance));
        match scored.as_slice() {
            [] => None,
            [only] => Some(only.2),
            [best, next, ..] => ((best.0, best.1) != (next.0, next.1)).then_some(best.2),
        }
    }

    /// The weapons a label can name: its calibre, the model within one error more than
    /// `weapon_for_label` allows (a conversion's weapon is a letter apart from the original's).
    pub fn label_weapons(&self, label: &str) -> Vec<&str> {
        let Some((calibre, model)) = calibre_and_model(label) else { return Vec::new() };
        let allowed = allowed_errors(&model) + 1;
        let mut out: Vec<&str> = self
            .weapons
            .keys()
            .filter(|name| {
                calibre_and_model(name).is_some_and(|(c, m)| c == calibre && prefix_distance(&m, &model) + m.chars().count().saturating_sub(model.chars().count()) <= allowed)
            })
            .map(String::as_str)
            .collect();
        out.sort_unstable();
        out
    }

    /// The artillery weapons of the data (`is_artillery_weapon`): whose cards to compare a card
    /// with when the weapon label cannot be read (a game notice over it, a new weapon).
    pub fn artillery_weapons(&self) -> Vec<&str> {
        let mut out: Vec<&str> = self.weapons.keys().map(String::as_str).filter(|w| is_artillery_weapon(w)).collect();
        out.sort_unstable();
        out
    }

    /// The muzzle velocities (m/s, rounded) the shells of these weapons fly at, every artillery
    /// weapon's when none is given: what a card's VEL value can be.
    pub fn card_velocities(&self, weapons: &[&str]) -> Vec<u32> {
        let all;
        let weapons = if weapons.is_empty() {
            all = self.artillery_weapons();
            &all[..]
        } else {
            weapons
        };
        let mut out: Vec<u32> = weapons.iter().flat_map(|w| self.shells(w)).map(|s| s.velocity.round() as u32).collect();
        out.sort_unstable();
        out.dedup();
        out
    }

    /// What a card of these weapons can show for a shell flying at `velocity` (every shell when
    /// not read): the name, the name without a trailing charge ("M107 High Charge" as "M107"),
    /// and either cut short with "..." after any number of its characters (a card cuts a name too
    /// long for it: "M107 High...", "Mediu..."). Whole strings to compare a card's name with.
    /// No weapons (the label not read): every artillery weapon's, when the velocity is read.
    pub fn card_texts(&self, weapons: &[&str], velocity: Option<u32>) -> Vec<String> {
        let all;
        let weapons = match (weapons.is_empty(), velocity) {
            (false, _) => weapons,
            (true, Some(_)) => {
                all = self.artillery_weapons();
                &all[..]
            }
            (true, None) => return Vec::new(),
        };
        let mut out: Vec<String> = Vec::new();
        let mut push = |t: String| {
            if !out.contains(&t) {
                out.push(t);
            }
        };
        for shell in weapons.iter().flat_map(|w| self.shells(w)) {
            if velocity.is_some_and(|v| (shell.velocity - f64::from(v)).abs() >= 0.5) {
                continue;
            }
            for name in [shell.name.clone(), shown_name(shell)] {
                let chars: Vec<char> = name.chars().collect();
                for n in 1..chars.len() {
                    let cut: String = chars[..n].iter().collect();
                    let cut = cut.trim_end();
                    if cut.chars().filter(|c| *c != ' ').count() >= 2 {
                        push(format!("{cut}..."));
                    }
                }
                push(name);
            }
        }
        out
    }

    /// The weapon labels the screen shows for the data's weapons, as far as their names give them:
    /// "<calibre> mm <model>", the calibre as the name writes it ("155 mm M284" for `155mm M284`,
    /// "165 mm L9A1" for `L9A1 165mm`, "7.62 mm MG3"); calibre 152 also as 152.4 (the screen's
    /// "152.4 mm 2A64" for `152mm 2A64`). Names without a calibre in
    /// millimetres (`M8 Rocket`, `7.5 cm PaK 40 L/46`) give none. What the gunner's weapon box is
    /// compared with whole (`mtc_vision::loading::read_weapon_label_among`).
    pub fn label_texts(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for name in self.weapons.keys() {
            let tokens: Vec<&str> = name.split_whitespace().collect();
            let is_calibre = |t: &str| !t.is_empty() && t.chars().all(|c| c.is_ascii_digit() || c == '.') && t.starts_with(|c: char| c.is_ascii_digit());
            // "155mm" as one token, or "88 mm" as two.
            let found = tokens.iter().enumerate().find_map(|(i, t)| match t.strip_suffix("mm") {
                Some(c) if is_calibre(c) => Some((c, i, i + 1)),
                _ if *t == "mm" && i > 0 && is_calibre(tokens[i - 1]) => Some((tokens[i - 1], i - 1, i + 1)),
                _ => None,
            });
            let Some((calibre, from, to)) = found else { continue };
            let model: Vec<&str> = tokens[..from].iter().chain(&tokens[to..]).copied().collect();
            if model.is_empty() {
                continue;
            }
            let model = model.join(" ");
            let calibres: &[&str] = if calibre == "152" { &["152", "152.4"] } else { &[calibre] };
            for c in calibres {
                let label = format!("{c} mm {model}");
                if !out.contains(&label) {
                    out.push(label);
                }
            }
        }
        out
    }

    /// Shells of `weapon` of data type `kind` ("HE", "TIME FUZE", ...) at `charge`: what the
    /// loaded slot's icon and charge bags on the gunner's bar leave (one ballistic choice on every
    /// artillery weapon of the data).
    pub fn shells_of_kind(&self, weapon: &str, kind: &str, charge: Option<ChargeLevel>) -> Vec<&Shell> {
        self.shells(weapon).iter().filter(|s| s.kind == kind && s.charge() == charge).collect()
    }

    /// Shells of `weapon` a card can show: `velocity` if read, the selected `charge` in the name
    /// (`None`: a card without charges, whose names carry none when the weapon has such shells),
    /// and of those the names closest to `prefix` (the cut name read; trailing dots dropped).
    pub fn card_candidates(&self, weapon: &str, prefix: &str, velocity: Option<u32>, charge: Option<ChargeLevel>) -> Vec<&Shell> {
        let all: Vec<&Shell> = self.shells(weapon).iter().collect();
        let by_velocity: Vec<&Shell> = match velocity {
            Some(v) => all.iter().copied().filter(|s| (s.velocity - f64::from(v)).abs() < 0.5).collect(),
            None => all,
        };
        let charged: Vec<&Shell> = match charge {
            Some(c) => by_velocity.iter().copied().filter(|s| s.name.contains(c.word())).collect(),
            None => by_velocity.iter().copied().filter(|s| !s.name.contains("Charge")).collect(),
        };
        let pool = if charged.is_empty() { by_velocity } else { charged };
        let read = prefix.trim();
        let cut = read.ends_with(['.', '…']);
        let prefix = read.trim_end_matches(['.', '…', ' ']).trim();
        let scored: Vec<(usize, &Shell)> = pool.into_iter().map(|s| (prefix_distance(&s.name, prefix), s)).collect();
        let Some(best) = scored.iter().map(|s| s.0).min() else { return Vec::new() };
        if best > allowed_errors(prefix) {
            return Vec::new();
        }
        let tied: Vec<&Shell> = scored.into_iter().filter(|s| s.0 == best).map(|s| s.1).collect();
        // A text not cut short is the card's whole name, and a card may show a name ending in its
        // charge without it ("M107 High Charge" as "M107", the fuzed "M107 High Charge VT" as
        // "M107 High..."; not every one: "M49A2 Low..."): of tied shells, those whose name so
        // shortened is the text. The HE M107 and its fuzed kin fly as far only at Low (lifetimes
        // 45 s against 30 and 20 at Medium and High in the wiki data).
        if !cut && tied.len() > 1 {
            let whole: Vec<(usize, &Shell)> = tied.iter().map(|s| (whole_distance(&shown_name(s), prefix), *s)).collect();
            if let Some(b) = whole.iter().map(|s| s.0).min().filter(|&b| b <= allowed_errors(prefix)) {
                return whole.into_iter().filter(|s| s.0 == b).map(|s| s.1).collect();
            }
        }
        tied
    }
}

impl AmmoData {
    /// The same shell at each of its charges (`shell` itself included), Low to High; just
    /// `shell` when it has no charges.
    pub fn charge_family<'a>(&'a self, weapon: &str, shell: &'a Shell) -> Vec<&'a Shell> {
        if shell.charge().is_none() {
            return vec![shell];
        }
        let base = shell.base_name();
        let mut family: Vec<&Shell> = self.shells(weapon).iter().filter(|s| s.charge().is_some() && s.base_name() == base).collect();
        family.sort_by_key(|s| s.charge().map(|c| c as u8));
        family
    }
}

/// Whether the shells fly alike (same velocity, gravity, drag and lifetime).
pub fn same_ballistics(shells: &[&Shell]) -> bool {
    shells.windows(2).all(|w| w[0].velocity == w[1].velocity && w[0].ballistics == w[1].ballistics)
}

#[cfg(test)]
mod tests {
    use super::*;

    const DATA: &str = r#"{"data":{"game":{"data":{
        "155mm M284":[
            {"name":"M107 High Charge","velocity":684,"type":"HE","ballistics":{"gravity":9.81,"lifetime":20,"dragConstant":7.954243e-05}},
            {"name":"M107 High Charge VT","velocity":684,"type":"TIME FUZE","ballistics":{"gravity":9.81,"lifetime":20,"dragConstant":7.954243e-05}},
            {"name":"M107 Low Charge","velocity":171,"type":"HE","ballistics":{"gravity":9.81,"lifetime":45,"dragConstant":3.4496436e-05}},
            {"name":"M110 High Charge","velocity":684,"type":"SMOKE","ballistics":{"gravity":9.81,"lifetime":45,"dragConstant":0.00115189828}}],
        "155mm L62":[
            {"name":"DM702A1","velocity":1015,"type":"Top Attack Shell","ballistics":{"gravity":9.81,"lifetime":60}},
            {"name":"DM702A1 High Charge","velocity":1015,"type":"SMOKE","ballistics":{"gravity":9.81,"lifetime":45,"dragConstant":9.7120835e-05}}],
        "152mm 2A64":[], "152mm 2A64M":[], "60mm M2":[], "12.7mm M2":[], "227mm M269":[]
    }}}}"#;

    #[test]
    fn shells_convert_like_the_upstream_calculator() {
        let shell: Shell = serde_json::from_str(
            r#"{"name":"3VO28 Low Charge","velocity":207,"type":"CLUSTER","ballistics":{"gravity":9.81,"lifetime":45,"dragConstant":0.009072919583567323},
               "explosive":{"killRadius":0.52,"mass":0.067049,"radius":9.8},"cluster":{"submunitions":42,"deployFactor":0.8,"dispersion":20}}"#,
        )
        .expect("json");
        let p = shell.projectile();
        assert_eq!(p.explosive_mass, Some(0.067));
        assert_eq!((p.submunitions, p.submunition_deploy_factor, p.submunition_angular_dispersion), (Some(42.0), Some(0.8), Some(20.0)));
        let plain: Shell = serde_json::from_str(r#"{"name":"DM702A1","velocity":1015,"ballistics":{"gravity":9.81,"lifetime":60}}"#).expect("json");
        assert_eq!(plain.projectile().drag_constant, None);
        assert_eq!(shell.base_name(), "3VO28");
    }

    /// Measured guns keep their own launch point; a converted 2S19 and a mortar not measured take the
    /// measured one named; anything else the M109A6's.
    #[test]
    fn launch_points_come_from_the_measured_guns() {
        let (d30, from) = launch_point("122mm D-30");
        assert_eq!((d30.height_m, d30.ahead_m, from), (1.3, 22.8, None));
        assert_eq!(launch_point("152mm 2A64"), (launch_point("152mm 2A64M").0, Some("152mm 2A64M")));
        assert_eq!(launch_point("120mm M120").1, Some("60mm M2"));
        assert_eq!(launch_point("227mm M269"), (launch_point("155mm M284").0, Some("155mm M284")));
        // Every weapon on the artillery list resolves.
        for w in ["105mm M2A1", "203mm 2A44", "155mm L62", "M8 Rocket", "F-82 Automatic Mortar"] {
            assert!(launch_point(w).0.ahead_m > 0.0, "{w}");
        }
    }

    #[test]
    fn checked_elevation_ranges_follow_the_weapon() {
        assert_eq!(elevation_limits("155mm M284"), Some((-3.0, 75.0)));
        assert_eq!(elevation_limits("152mm 2A64"), Some((-3.0, 52.0)));
        assert_eq!(elevation_limits("60mm M2"), Some((40.0, 85.0)));
        // The converted 2S19 (2S33) reads the 2A64's range, not the wiki's -3..70 joined with the 2S43.
        assert_eq!(elevation_limits("152mm 2A64M"), Some((-3.0, 52.0)));
        // Not checked against the readout: the wiki's range (the 2S7, M270, BM-21).
        assert_eq!(elevation_limits("203mm 2A44"), Some((-0.01, 60.0)));
        assert_eq!(elevation_limits("227mm M269"), Some((0.0, 45.0)));
        assert_eq!(elevation_limits("122mm Grad"), Some((-15.0, 55.0)));
        // A mortar whose wiki range counts from its rest: none until the readout has shown it.
        assert_eq!(elevation_limits("120mm M120"), None);
    }

    #[test]
    fn weapon_table_from_the_wiki_matches_the_compiled_one() {
        let snapshot = "../../reference/wiki/production-1.0.0/20261117";
        if !std::path::Path::new(snapshot).is_dir() {
            eprintln!("skipped: the wiki data snapshot is not on this machine");
            return;
        }
        let read =|f: &str| std::fs::read_to_string(format!("{snapshot}/{f}")).expect(f);
        let (vehicles, placeables) = (read("vehicles.json"), read("placeables.json"));
        assert_eq!(WeaponTable::from_wiki(&[&vehicles, &placeables]).expect("table"), WeaponTable::compiled());
    }

    #[test]
    fn artillery_weapons_are_told_from_tank_guns() {
        for weapon in ["152mm 2A64", "152mm 2A64M", "155mm M284", "122mm Grad", "227mm M269", "60mm M2", "122mm D-30", "M8 Rocket"] {
            assert!(is_artillery_weapon(weapon), "{weapon}");
        }
        for weapon in ["76mm M7", "90mm M3", "57 mm 6pdr OQF", "100mm 2A70"] {
            assert!(!is_artillery_weapon(weapon), "{weapon}");
        }
    }

    #[test]
    fn labels_the_screen_shows_for_data_names() {
        let d = AmmoData::from_json(
            r#"{"data":{"game":{"data":{
                "L9A1 165mm":[], "155mm M284":[], "152mm 2A64":[], "88 mm PaK 43/2 L/71":[],
                "7.62mm MG3":[], "M8 Rocket":[], "7.5 cm PaK 40 L/46":[]
            }}}}"#,
        )
        .expect("json");
        let mut labels = d.label_texts();
        labels.sort();
        assert_eq!(labels, ["152 mm 2A64", "152.4 mm 2A64", "155 mm M284", "165 mm L9A1", "7.62 mm MG3", "88 mm PaK 43/2 L/71"]);
    }

    #[test]
    fn model_first_names_and_label_changes() {
        // The data writes some names model first; the screen writes calibre first.
        let d = AmmoData::from_json(
            r#"{"data":{"game":{"data":{
                "L9A1 165mm":[{"name":"L33A1","velocity":259,"ballistics":{"gravity":9.81,"lifetime":20}}],
                "155mm M185":[{"name":"M107 Low Charge","velocity":171,"ballistics":{"gravity":9.81,"lifetime":45}}]
            }}}}"#,
        )
        .expect("json");
        assert_eq!(d.weapon_for_label("165 mm L9A1"), Some("L9A1 165mm"));
        // Misreadings seen in play name the same weapon, so the HUD keeps its loaded shell.
        assert_eq!(d.weapon_for_label("165 mm L941"), Some("L9A1 165mm"));
        assert_eq!(d.weapon_for_label("165 mm 29A1"), Some("L9A1 165mm"));
        assert_eq!(d.weapon_for_label_and_cards("165 mm L9A1", &["M107High..."]), Some("L9A1 165mm"));
        // One misread glyph is the same label; another gun is not.
        assert!(same_weapon_label("152.4 mm 2A64", "152.4 mm 2A6A"));
        assert!(same_weapon_label("155 mm M185", "155 mm M185"));
        assert!(!same_weapon_label("155 mm M185", "165 mm L9A1"));
        assert!(!same_weapon_label("155 mm M185", "155 mm M284"));
        // A label cut off is the same weapon; the whole label or another gun is not cut off.
        assert!(cut_off_label("152.4 mm 2", "152.4 mm 2A64"));
        assert!(cut_off_label("122 mm D-3", "122 mm D-30"));
        assert!(!cut_off_label("152.4 mm 2A64", "152.4 mm 2A64"));
        assert!(!cut_off_label("155 mm M2", "152.4 mm 2A64"));
        assert!(!cut_off_label("00171ffY5k??????", "152.4 mm 2A64"));
    }

    #[test]
    fn cards_tell_a_converted_weapon_apart() {
        let d = AmmoData::from_json(
            r#"{"data":{"game":{"data":{
                "152mm 2A64":[{"name":"3OF45 Low Charge","velocity":207,"ballistics":{"gravity":9.81,"lifetime":45}},
                              {"name":"3OF45 Proxy Low Charge","velocity":207,"ballistics":{"gravity":9.81,"lifetime":45}}],
                "152mm 2A64M":[{"name":"3OF61 Low Charge","velocity":207,"ballistics":{"gravity":9.81,"lifetime":45}},
                               {"name":"3VO28 Low Charge","velocity":207,"ballistics":{"gravity":9.81,"lifetime":45}}]
            }}}}"#,
        )
        .expect("json");
        assert_eq!(d.weapon_for_label_and_cards("152.4 mm 2A64", &["3vO28", "3OF61"]), Some("152mm 2A64M"));
        assert_eq!(d.weapon_for_label_and_cards("152.4 mm 2A64", &["3OF45 Proxy", "3OF45"]), Some("152mm 2A64"));
        assert_eq!(d.weapon_for_label_and_cards("152.4 mm 2A64", &[]), Some("152mm 2A64"));
        assert_eq!(d.weapon_for_label_and_cards("152.4 mm 2A64M", &[]), Some("152mm 2A64M"));
    }

    #[test]
    fn cards_of_another_weapon_overrule_a_short_label() {
        let d = AmmoData::from_json(
            r#"{"data":{"game":{"data":{
                "155mm M284":[{"name":"M107 High Charge","velocity":684,"ballistics":{"gravity":9.81,"lifetime":45}},
                              {"name":"M110 High Charge","velocity":684,"ballistics":{"gravity":9.81,"lifetime":45}}],
                "155mm L62":[{"name":"DM121 High Charge","velocity":945,"ballistics":{"gravity":9.81,"lifetime":45}}]
            }}}}"#,
        )
        .expect("json");
        // "155 mm M284" misread short at 100 %: its prefix fits the L62, its cards the M284.
        assert_eq!(d.weapon_for_label_and_cards("155 mm L", &["M107Higb...", "M110"]), None);
        // Cards no weapon has leave the label to decide.
        assert_eq!(d.weapon_for_label_and_cards("155 mm L62", &["XM1156"]), Some("155mm L62"));
        assert_eq!(d.weapon_for_label_and_cards("155 mm M284", &["M107 High..."]), Some("155mm M284"));
    }

    #[test]
    fn labels_name_their_weapons() {
        let d = AmmoData::from_json(DATA).expect("json");
        assert_eq!(d.weapon_for_label("1SS mm M284"), Some("155mm M284"));
        assert_eq!(d.weapon_for_label("152.4 mm 2A64"), Some("152mm 2A64"));
        assert_eq!(d.weapon_for_label("60 mm M2"), Some("60mm M2"));
        assert_eq!(d.weapon_for_label("227mm M269"), Some("227mm M269"));
        assert_eq!(d.weapon_for_label("75 mm M2"), None);
    }

    /// The M185's 155 mm shells as the wiki data had them: the fuzed M107 lives 20 s at High, the
    /// HE one 45 s. The card "M107" is the HE shell; "M107 High..." any of the three (in the game
    /// only the M110 card showed charge marks).
    #[test]
    fn a_whole_card_name_is_not_its_fuzed_kin() {
        let d = AmmoData::from_json(
            r#"{"data":{"game":{"data":{"155mm M185":[
            {"name":"M107 High Charge","velocity":684,"type":"HE","ballistics":{"gravity":9.81,"lifetime":45,"dragConstant":7.954243e-05}},
            {"name":"M107 High Charge VT","velocity":684,"type":"TIME FUZE","ballistics":{"gravity":9.81,"lifetime":20,"dragConstant":7.954243e-05}},
            {"name":"M107 High Charge PF","velocity":684,"type":"PROXIMITY FUZE","ballistics":{"gravity":9.81,"lifetime":20,"dragConstant":7.954243e-05}},
            {"name":"M110 High Charge","velocity":684,"type":"SMOKE","ballistics":{"gravity":9.81,"lifetime":45,"dragConstant":0.00115189828}}]}}}}"#,
        )
        .expect("json");
        let names = |c: Vec<&Shell>| c.iter().map(|s| s.name.clone()).collect::<Vec<_>>();
        assert_eq!(names(d.card_candidates("155mm M185", "M107", Some(684), Some(ChargeLevel::High))), ["M107 High Charge"]);
        let cut = d.card_candidates("155mm M185", "M107High...", Some(684), Some(ChargeLevel::High));
        assert_eq!(names(cut.clone()), ["M107 High Charge", "M107 High Charge VT", "M107 High Charge PF"]);
        assert!(!same_ballistics(&cut));
        assert_eq!(names(d.card_candidates("155mm M185", "M110", Some(684), Some(ChargeLevel::High))), ["M110 High Charge"]);
    }

    #[test]
    fn cards_narrow_to_one_ballistic_choice() {
        let d = AmmoData::from_json(DATA).expect("json");
        // "M107 High..." at High: HE and VT, which fly alike here.
        let c = d.card_candidates("155mm M284", "MI07High...", Some(684), Some(ChargeLevel::High));
        assert_eq!(c.len(), 2);
        assert!(same_ballistics(&c));
        // "M110" at High: the smoke shell only.
        assert_eq!(d.card_candidates("155mm M284", "M110", Some(684), Some(ChargeLevel::High))[0].kind, "SMOKE");
        // PzH 2000: DM702A1 without charges is the SMArt shell, with charges the smoke shell.
        assert_eq!(d.card_candidates("155mm L62", "DM702A1", Some(1015), None)[0].kind, "Top Attack Shell");
        assert_eq!(d.card_candidates("155mm L62", "DM702A1", Some(1015), Some(ChargeLevel::High))[0].kind, "SMOKE");
        // Misread glyphs: the closest name wins.
        assert_eq!(d.card_candidates("155mm M284", "M1I0", Some(684), Some(ChargeLevel::High))[0].kind, "SMOKE");
        assert_eq!(velocity_in("684 m s"), Some(684));
        let he = &d.shells("155mm M284")[0];
        let family: Vec<&str> = d.charge_family("155mm M284", he).iter().map(|s| s.name.as_str()).collect();
        assert_eq!(family, ["M107 Low Charge", "M107 High Charge"]);
        let smart = &d.shells("155mm L62")[0];
        assert_eq!(d.charge_family("155mm L62", smart).len(), 1);
        assert_eq!(velocity_in("1O1S m/s"), Some(1015));
    }
}
