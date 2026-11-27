//! Loading pipeline: the loading menu, the countdown and the loaded slot, and which shell is
//! loaded.
//!
//! One read-back region covers the cards in all three layouts, the countdown bar, both bars and
//! the weapon labels; it is read at most every `READ_INTERVAL_MS`. Card text (shell name, VEL)
//! costs ~15 ms per menu, so it is read when the cards appear or a card's charge changes, at most
//! every `TEXT_INTERVAL_MS`; each slot keeps its name and velocity with the charge they were read
//! at. When a slot is loaded its shell is looked up in the ammunition data from the weapon label,
//! the slot's name and the loaded charge. The shell of the newest loading (from its start, when
//! the slot is known) is the current shell for the firing solution.

use std::path::PathBuf;

use mtc_ammo::{AmmoData, ChargeLevel, Shell, same_ballistics};
use mtc_overlay::{MenuItem, MenuPick, PanelMenu};
use mtc_vision::FrameView;
use mtc_vision::bar::{BarLayout, BarReader};
use mtc_vision::loading::{CardCharge, Charge, CountdownBox, COUNTDOWN_BOX, Menu, MenuSeat, ShellIcons, countdown_progress_in, countdown_shown_in, find_countdown, gunner_bar_bags, gunner_bar_highlight, gunner_strip_highlight, read_card_texts_among, read_loader_bar, read_menu, read_weapon_label_among};

/// The loader layout counts as on screen this long after it was last seen (ms): three reads.
const LOADER_SEEN_MS: u64 = 600;
use mtc_vision::loading_state::{Loaded, LoadingEvent, LoadingFrame, LoadingTracker, SlotCharge};
use mtc_vision::text::TextReader;

const READ_INTERVAL_MS: u64 = 200;
/// Off the reference configuration the countdown bar is read where it was found on screen
/// (`find_countdown`) once this many finds agree within `COUNTDOWN_AGREE_PX` (at 100 % scaling
/// and 1920x1080, the same box on 44 of 46 countdown frames, the rest within 3 px).
const COUNTDOWN_AGREE: usize = 3;
const COUNTDOWN_AGREE_PX: i32 = 3;
/// After F (the fire key) the bar and the menu are read on every frame this long (ms) for the
/// screen to confirm the shot: the game shows a key's effect 45 ms after the press (median; at
/// most 63 ms over 146 presses), and the menu then stays up for at least a frame.
const FIRE_CONFIRM_MS: u64 = 1_500;
/// A press made with V1 or V2 open waits for the screen until `FIRE_CONFIRM_MS` after the view is
/// back on V3 (the loading menu shows only there), at most this long (ms): the X then confirms it
/// (`take_unconfirmed`).
const AWAY_PENDING_MAX_MS: u64 = 20_000;
/// A press waiting for V3 has this deadline until the view comes back.
const PENDING: u64 = u64::MAX;
/// The game shows the menu only on V3 (3019 recorded frames) and fades it back in after V1 or V2:
/// it read 196–259 ms after the view came back (three switches measured). The menu cannot change
/// meanwhile (no loading on V1; an open menu means nothing is loading), so the one shown when the
/// view left V3 stands in from the first frame back, for this long (ms) or until the menu reads,
/// with the region read every frame meanwhile. Otherwise the menu's charge marks would vanish and
/// reappear a beat late when V1 is toggled with M.
const BACK_ON_V3_MS: u64 = 500;
/// A menu kept as M was pressed stays kept through this long (ms) to the view's change.
const KEPT_FROM_KEY_MS: u64 = 1_000;
const TEXT_INTERVAL_MS: u64 = 1_000;
/// The weapon label is read this often (ms), whatever else shows: a new vehicle (another gun)
/// shows in its label first, often without a menu or a readout (third person).
const WEAPON_RECHECK_MS: u64 = 2_000;
/// The label is looked for this often (ms) while no weapon is decided.
const WEAPON_SEARCH_MS: u64 = 500;
/// Another weapon's label read this many times in a row is another weapon.
const LABEL_SWITCH_READS: u32 = 2;
/// Weapon names read as no weapon's label at all (not "<calibre> mm <name>") before the log says
/// so once, while no weapon was ever decided this run.
const UNREADABLE_LABELS_LOGGED: u32 = 10;

/// Counts a weapon name read that is not shaped like one and logs once when they keep coming with
/// no weapon decided. Only logged: the reading fails for our own reasons (a size the glyphs don't
/// fit) as well as for a font pack or translation, and the two can't be told apart here.
fn note_unreadable_label(label: &str, weapon_known: bool) {
    use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
    static COUNT: AtomicU32 = AtomicU32::new(0);
    static LOGGED: AtomicBool = AtomicBool::new(false);
    if weapon_known || shaped_like_weapon_label(label) {
        return;
    }
    if COUNT.fetch_add(1, Ordering::Relaxed) + 1 < UNREADABLE_LABELS_LOGGED || LOGGED.swap(true, Ordering::Relaxed) {
        return;
    }
    log::warn!("The weapon name keeps reading as no weapon's name (last \"{label}\"), and no weapon was recognised this run");
}

/// Whether `label` looks like a weapon name: a calibre in mm, then a name ("155 mm M185").
fn shaped_like_weapon_label(label: &str) -> bool {
    let mut words = label.split_whitespace();
    let calibre = words.next().is_some_and(|w| !w.is_empty() && w.chars().all(|c| c.is_ascii_digit() || c == '.'));
    calibre && words.next() == Some("mm") && words.next().is_some()
}
/// Guns on a page of the gun list.
const GUNS_PER_PAGE: usize = 6;

/// A page of the shell list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ListPage {
    /// The gun's shells.
    Shells,
    /// The guns from this one on (the calibre the label read first).
    Guns(usize),
}

/// A charge's name without " Charge" ("Medium").
fn charge_word(c: ChargeLevel) -> &'static str {
    c.word().trim_end_matches(" Charge")
}

/// A hidden countdown counts as run out this long (ms) after its predicted end.
const RAN_OUT_MARGIN_MS: u64 = 500;
/// A new loaded slot or charge on the gunner's bar is taken after this many reads in a row.
const BAR_SWITCH_READS: u32 = 2;
/// The bar showing no lit slot this many reads in a row (200 ms apart) after a loading: fired.
const FIRED_READS: u32 = 2;
/// The slot read lit for longer than this (ms) counts as a shell loaded: sitting down in the seat
/// shows the slot of the shell picked for a moment before the menu or the loading takes over,
/// while a loaded gun is laid for seconds before it fires.
const ARM_LIT_MS: u64 = 1_500;

/// What the gunner's bar shows loaded: the slot and its charge, and the type of shell its icon
/// stands for (`None` when not read).
#[derive(Clone, Copy, Debug, PartialEq)]
struct BarShell {
    loaded: Loaded,
    kind: Option<&'static str>,
}

/// A slot's text as last read, with the charge shown then.
struct CardRead {
    name: String,
    velocity: Option<u32>,
    charge: SlotCharge,
}

/// The shell being loaded or loaded last, resolved in the ammunition data.
#[derive(Clone, Debug, PartialEq)]
pub struct ShellPick {
    pub weapon: String,
    pub slot: usize,
    /// Of shells that fly alike (fuze variants), the shortest name.
    pub shell: Shell,
    /// The same shell at each of its charges, Low to High (just `shell` without charges).
    pub family: Vec<Shell>,
    /// The shell was picked by hand (the shell list), not read.
    pub by_hand: bool,
    /// The loaded charge is known: read for this loading or picked by hand. Otherwise `shell` is
    /// the family's member at the charge last seen, standing in for the family.
    pub charge_known: bool,
    /// The charge was picked by hand for this shot.
    pub charge_by_hand: bool,
}

impl ShellPick {
    /// The loaded charge, when known.
    pub fn loaded_charge(&self) -> Option<ChargeLevel> {
        self.shell.charge().filter(|_| self.charge_known)
    }
}

pub struct LoadingPipeline {
    /// Cycles and calls of the costly steps since the frame loop last took them: card texts, the
    /// weapon name, deciding the weapon.
    pub cost: [(u64, u32); 3],
    bar: BarReader,
    reader: TextReader,
    /// Another display configuration than the reference is on screen (`set_reader`).
    elsewhere: bool,
    /// There, the countdown bar as found on screen and taken (`countdown_bar`), and the finds
    /// before it was.
    countdown_box: Option<CountdownBox>,
    countdown_finds: Vec<CountdownBox>,
    tracker: LoadingTracker,
    ammo: Option<AmmoData>,
    /// The labels the data's weapons show (`AmmoData::label_texts`): what the gunner's weapon box
    /// is compared with whole before it is read glyph by glyph.
    labels: Vec<String>,
    /// On a first start without the data: what a background retry downloaded, taken on the next
    /// read (`retry_ammo_data`).
    ammo_arrived: std::sync::Arc<std::sync::Mutex<Option<AmmoData>>>,
    /// Weapon (data name) and when its label was last read.
    weapon: Option<String>,
    last_weapon_read: Option<u64>,
    cards: Vec<Option<CardRead>>,
    layout: Option<(MenuSeat, usize)>,
    last_read: Option<u64>,
    /// The view came back to V3 then (ms): read every frame for `BACK_ON_V3_MS`, until the menu.
    back_on_v3: Option<u64>,
    /// M went down on V3 with the menu open then (ms): the region is read every frame until the
    /// menu is gone, for how soon the game hides it.
    m_on_v3: Option<u64>,
    /// The menu on screen when the view left V3 (and when), and whether it stands in since
    /// coming back.
    menu_on_leave: Option<(Menu, u64)>,
    menu_restored: bool,
    last_text: Option<u64>,
    reported_charges: Vec<SlotCharge>,
    /// The weapon label as last read: with the cards it decides the weapon.
    weapon_label: Option<String>,
    /// The label everything weapon-dependent (cards, charges, the loaded shell) belongs to, and
    /// another label being read, with how many reads in a row.
    vehicle_label: Option<String>,
    label_change: Option<(String, u32)>,
    /// The gunner's bar as last applied: the highlighted (loaded) slot, the charge its capsule
    /// shows and the shell type its icon shows. A teammate's loading shows only here: the gunner
    /// sees no menu, and the bar's slot order differs from the menu's.
    bar_loaded: Option<BarShell>,
    /// A different bar reading and how many reads in a row it held (applied at `BAR_SWITCH_READS`).
    bar_pending: Option<(BarShell, u32)>,
    /// A loading just ended with the charge the menu showed: the bar, read again, has the last
    /// word on the charge (the menu read can be stale; the bar's capsule is what is loaded).
    bar_check: bool,
    icons: ShellIcons,
    /// The current loading's countdown went out of view and its predicted end has passed.
    ran_out: bool,
    /// The loaded shell was fired: the bar's lit slot went dark, open or collapsed (the game
    /// lights the slot from the loading's start until the shot; a rocket pod's until its last
    /// rocket). Cleared by the next loading or a lit slot. And how many reads in a row showed no
    /// lit slot.
    fired: bool,
    fired_reads: u32,
    /// The last read with the slot lit and the first read since without (ms): the shot fell
    /// between them.
    last_lit_ms: Option<u64>,
    first_dark_ms: Option<u64>,
    /// The gunner's bar showed a lit (loaded) slot since the last shot: loaded as far as the bar
    /// tells, with no menu or countdown on this screen (with a player in the loader seat the
    /// loading menu never pops up; without this no shot was counted in a whole match).
    lit_since_shot: bool,
    /// The loading menu showed on the last read (as read, not a menu kept over a view change).
    menu_was_shown: bool,
    /// A shot was counted and the next shell has not been seen loaded yet: one shot per loading
    /// (the loading right after a shot is that shot's reload, not another).
    awaiting_load: bool,
    /// Since the shot, the reload was seen: its countdown showed, or (with no menu since) the slot
    /// read dark off the full map (a loader reloads out of the gunner's sight: dark until he has).
    reload_seen: bool,
    /// When the slot first read lit in the current run of lit reads.
    lit_run_from: Option<u64>,
    /// F presses (ms) the screen has not matched to a shot yet, each with the time (ms) it may be
    /// matched until: `FIRE_CONFIRM_MS` after it, or after the view is back on V3 for a press made
    /// on V1 or V2 (`PENDING` until then; the menu shows only once the map closes, so shots fired
    /// on V1 went unrecorded otherwise). Presses while the gun is loading do nothing, the first
    /// after it is loaded fires.
    fire_keys: Vec<(u64, u64)>,
    /// V1 or V2 is open (between `left_v3` and `back_on_v3`).
    away: bool,
    /// Presses the screen did not confirm with the gun loaded (ms): shots if their X shows where
    /// they would land (`take_unconfirmed`, the shot log).
    unconfirmed: Vec<u64>,
    /// Key presses come through (the input thread runs, or the replayed recording logged keys):
    /// a screen change with no F press before it is no shot (a seat or vehicle change).
    keys_live: bool,
    /// A shot not yet taken by the shot log: between these reads (ms).
    shot: Option<(u64, u64)>,
    /// The open bar's layout as last read, with the readout label x it was read at: the bar
    /// collapsed under the full map keeps its columns.
    open_bar: Option<(i32, BarLayout)>,
    /// Per slot, the last charge selected on screen. The menu fades out for about half a second
    /// when a loading key is pressed, and a fading charge column reads as none; a card with
    /// charges always shows one, so a slot that has shown one keeps it.
    last_selected: Vec<Option<SlotCharge>>,
    current: Option<ShellPick>,
    /// Slots of the loader's menu or simplified bar, and when it was last seen.
    loader: Option<(usize, u64)>,
    /// The card menu in the latest read and when it was read (`None` once a read misses it).
    menu: Option<(Menu, u64)>,
    /// The weapon label names an artillery weapon (tank guns get no HUD).
    artillery: bool,
    /// A different loader slot count seen, and in how many reads in a row: the panel's place
    /// changes only when it holds (a vehicle interior behind the bar can flicker the count).
    loader_pending: Option<(usize, u32)>,
    /// Per slot, the last menu read did not show its charge (the charge column and the bag
    /// capsule unread): the slot's charge is the one remembered from before, which may have
    /// changed since.
    charge_unread: Vec<bool>,
    /// The slot last seen loading or loaded, its charge as read, and whether that charge was read
    /// for this loading.
    loaded_at: Option<(Loaded, bool)>,
    /// Shells picked by hand per slot (names without the charge): that slot holds it from now on
    /// whatever is read, until Auto or another weapon.
    manual: Vec<Option<String>>,
    /// Picked with no slot seen loading yet: the next slot loaded takes it.
    manual_next: Option<String>,
    /// This shot's charge picked by hand, with the slot it was picked for; until the next loading.
    manual_charge: Option<(Option<usize>, ChargeLevel)>,
    /// The gun was picked by hand: the label read does not change it (another label does).
    weapon_by_hand: bool,
    /// The current shell with the player's picks over what was read (`apply_manual`).
    chosen: Option<ShellPick>,
}

/// A new loader slot count is taken after this many reads in a row.
const LOADER_SWITCH_READS: u32 = 2;

/// A card of the menu on screen: where it is, whether its charge rows show, and its shell at
/// each charge (Low to High; empty while the shell is not known).
#[derive(Clone, Debug)]
pub struct MenuCard {
    pub centre_x: f64,
    pub rows_shown: bool,
    /// The charge family of each kind of shell the card can be (one when its text tells; more
    /// when a cut-short name fits shells that fly apart, e.g. "M107 High..." the HE, VT and PF
    /// M107, whose lifetimes differ at Medium and High).
    pub families: Vec<Vec<Shell>>,
}

/// The ammunition data: `MTC_AMMO_DATA` when set (development); else the copy downloaded from the
/// wiki data service (`mtc_ammo::fetch`), fetched now when there is none (`refresh_wiki_data`
/// keeps it fresh); else, with no network on a first start, the newest development snapshot
/// (`reference/wiki/*/*/shells.json`).
fn ammo_data_path() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("MTC_AMMO_DATA") {
        return Some(PathBuf::from(p));
    }
    let cached = mtc_ammo::fetch::shells_path();
    if cached.is_file() {
        return Some(cached);
    }
    log::info!("Ammunition data: none on this computer, downloading from the wiki data service");
    match mtc_ammo::fetch::download_shells() {
        Ok(_) => return Some(cached),
        Err(e) => log::warn!("Ammunition data could not be downloaded: {e}"),
    }
    let root = PathBuf::from("reference/wiki");
    let mut found: Vec<PathBuf> = std::fs::read_dir(&root)
        .into_iter()
        .flatten()
        .flatten()
        .flat_map(|service| std::fs::read_dir(service.path()).into_iter().flatten().flatten())
        .map(|dated| dated.path().join("shells.json"))
        .filter(|p| p.is_file())
        .collect();
    found.sort();
    found.pop()
}

/// The artillery weapon table: the one built from the downloaded wiki data, else the one compiled
/// in (a first start, or no network so far). Installed before anything asks whether a weapon is
/// artillery.
fn install_weapon_table() {
    match mtc_ammo::fetch::load_weapons() {
        Some(table) => {
            let count = table.weapons.len();
            if table.install() {
                log::info!("Artillery weapons: wiki data, {count}");
            }
        }
        None => log::info!("Artillery weapons: the built-in table ({}), wiki data downloading in the background", mtc_ammo::WeaponTable::current().weapons.len()),
    }
}

/// Where the ammunition data tried again in the background arrives: one retry per run, shared by
/// every pipeline (a capture started again makes a new one).
fn ammo_retry_slot() -> std::sync::Arc<std::sync::Mutex<Option<AmmoData>>> {
    static SLOT: std::sync::OnceLock<std::sync::Arc<std::sync::Mutex<Option<AmmoData>>>> = std::sync::OnceLock::new();
    SLOT.get_or_init(|| {
        let slot = std::sync::Arc::new(std::sync::Mutex::new(None));
        retry_ammo_data(slot.clone());
        slot
    })
    .clone()
}

/// The ammunition data not downloaded on a first start, tried again in the background
/// (`retry::delay`) until it comes; handed over through `arrived`.
fn retry_ammo_data(arrived: std::sync::Arc<std::sync::Mutex<Option<AmmoData>>>) {
    std::thread::Builder::new()
        .name("ammo-retry".into())
        .spawn(move || {
            for attempt in 0.. {
                std::thread::sleep(crate::retry::delay(attempt));
                let data = mtc_ammo::fetch::download_shells()
                    .ok()
                    .and_then(|_| std::fs::read_to_string(mtc_ammo::fetch::shells_path()).ok())
                    .and_then(|json| AmmoData::from_json(&json).ok());
                match data {
                    Some(data) => {
                        log::info!("Ammunition data: downloaded on retry {}", attempt + 1);
                        *arrived.lock().expect("ammo slot") = Some(data);
                        return;
                    }
                    None => log::info!("Ammunition data: retry {} failed again, next try in {} s", attempt + 1, crate::retry::delay(attempt + 1).as_secs()),
                }
            }
        })
        .expect("ammo retry thread");
}

/// Refreshes the downloaded wiki data that is missing or a day old, on a thread, once per run:
/// the next start reads it. `shells`: the ammunition data comes from the cache (not
/// `MTC_AMMO_DATA`).
fn refresh_wiki_data(shells: bool) {
    static STARTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if STARTED.swap(true, std::sync::atomic::Ordering::Relaxed) {
        return;
    }
    let shells = shells && mtc_ammo::fetch::stale(&mtc_ammo::fetch::shells_path());
    let weapons = mtc_ammo::fetch::stale(&mtc_ammo::fetch::weapons_path());
    if !shells && !weapons {
        return;
    }
    std::thread::Builder::new().name("wiki-refresh".into()).spawn(move || {
        if shells {
            match mtc_ammo::fetch::download_shells() {
                Ok(true) => log::info!("Ammunition data updated in the background, used from the next start"),
                Ok(false) => {}
                Err(e) => log::warn!("Ammunition data could not be updated ({e}), using the cached copy"),
            }
        }
        if weapons {
            match mtc_ammo::fetch::download_weapons() {
                Ok(true) => log::info!("Artillery weapons updated in the background, used from the next start"),
                Ok(false) => {}
                Err(e) => log::warn!("Artillery weapons could not be updated ({e}), keeping the current table"),
            }
        }
    }).expect("wiki refresh thread");
}

fn level(charge: SlotCharge) -> Option<ChargeLevel> {
    match charge {
        SlotCharge::Selected(Charge::Low) => Some(ChargeLevel::Low),
        SlotCharge::Selected(Charge::Medium) => Some(ChargeLevel::Medium),
        SlotCharge::Selected(Charge::High) => Some(ChargeLevel::High),
        SlotCharge::None | SlotCharge::Unknown => None,
    }
}

fn charge_name(charge: SlotCharge) -> &'static str {
    match charge {
        SlotCharge::Selected(Charge::Low) => "Low",
        SlotCharge::Selected(Charge::Medium) => "Medium",
        SlotCharge::Selected(Charge::High) => "High",
        SlotCharge::None => "no charge",
        SlotCharge::Unknown => "charge unknown",
    }
}

impl LoadingPipeline {
    pub fn new() -> Self {
        install_weapon_table();
        let ammo = ammo_data_path().and_then(|p| {
            let data = std::fs::read_to_string(&p).ok().and_then(|json| AmmoData::from_json(&json).ok());
            match &data {
                Some(_) => log::info!("Ammunition data: {}", p.display()),
                None => log::info!("Ammunition data unreadable: {}", p.display()),
            }
            data
        });
        let mut ammo_arrived = std::sync::Arc::new(std::sync::Mutex::new(None));
        if ammo.is_none() {
            log::info!("No ammunition data: only slots and charges are reported");
            if std::env::var_os("MTC_AMMO_DATA").is_none() {
                ammo_arrived = ammo_retry_slot();
            }
        }
        refresh_wiki_data(std::env::var_os("MTC_AMMO_DATA").is_none());
        Self {
            bar: BarReader::new(),
            reader: TextReader::new(),
            elsewhere: false,
            countdown_box: None,
            countdown_finds: Vec::new(),
            tracker: LoadingTracker::default(),
            labels: ammo.as_ref().map(AmmoData::label_texts).unwrap_or_default(),
            ammo,
            ammo_arrived,
            weapon: None,
            last_weapon_read: None,
            cost: [(0, 0); 3],
            cards: Vec::new(),
            layout: None,
            last_read: None,
            back_on_v3: None,
            m_on_v3: None,
            menu_on_leave: None,
            menu_restored: false,
            last_text: None,
            reported_charges: Vec::new(),
            weapon_label: None,
            vehicle_label: None,
            label_change: None,
            bar_loaded: None,
            bar_pending: None,
            bar_check: false,
            icons: ShellIcons::new(),
            ran_out: false,
            fired: false,
            fired_reads: 0,
            last_lit_ms: None,
            first_dark_ms: None,
            lit_since_shot: false,
            menu_was_shown: false,
            awaiting_load: false,
            reload_seen: false,
            lit_run_from: None,
            fire_keys: Vec::new(),
            away: false,
            unconfirmed: Vec::new(),
            keys_live: false,
            shot: None,
            open_bar: None,
            last_selected: Vec::new(),
            current: None,
            loader: None,
            menu: None,
            artillery: false,
            loader_pending: None,
            charge_unread: Vec::new(),
            loaded_at: None,
            manual: Vec::new(),
            manual_next: None,
            manual_charge: None,
            weapon_by_hand: false,
            chosen: None,
        }
    }

    /// The text reader for the display configuration on screen (`ui_text::UiText`): the 150 %
    /// asset at the reference, templates drawn for the configuration elsewhere.
    /// `elsewhere`: the configuration is not the reference, where the countdown bar is looked for
    /// on screen again (it does not follow the bottom group's rule, `find_countdown`).
    pub fn set_reader(&mut self, reader: TextReader, elsewhere: bool) {
        self.reader = reader;
        self.elsewhere = elsewhere;
        self.countdown_box = None;
        self.countdown_finds.clear();
        // Card names read with the old templates (another configuration, or the text size before a
        // readout fit) are read again with these.
        self.cards.clear();
        self.last_text = None;
    }

    /// Where the countdown bar is read on this view: the reference's place there, else as found
    /// on screen for this configuration, taken once `COUNTDOWN_AGREE` finds agree (a find before
    /// that is read where it was found).
    fn countdown_bar(&mut self, view: &FrameView, origin: (i32, i32)) -> Option<CountdownBox> {
        if !self.elsewhere {
            return Some(COUNTDOWN_BOX);
        }
        if let Some(bar) = self.countdown_box {
            return Some(bar);
        }
        let found = find_countdown(view, origin)?;
        let close = |a: &CountdownBox| {
            let (p, q) = (a.interior, found.interior);
            [(p.0, q.0), (p.1, q.1), (p.2, q.2), (p.3, q.3)].iter().all(|(u, v)| (u - v).abs() <= COUNTDOWN_AGREE_PX)
        };
        self.countdown_finds.retain(close);
        self.countdown_finds.push(found);
        if self.countdown_finds.len() >= COUNTDOWN_AGREE {
            self.countdown_box = Some(found);
            let (x0, y0, x1, y1) = found.interior;
            let (r0, s0, r1, s1) = COUNTDOWN_BOX.interior;
            log::info!("Loading countdown bar: on screen at x {x0}–{x1}, y {y0}–{y1} (reference geometry; at the reference x {r0}–{r1}, y {s0}–{s1})");
            crate::diag::note(crate::diag::Event::Countdown(found.interior));
        }
        Some(found)
    }

    /// The card menu on screen now (read within two read intervals), with the weapon and each
    /// card's shell family.
    pub fn menu_cards(&self, now_ms: u64) -> Option<(MenuSeat, String, Vec<MenuCard>)> {
        let (menu, _) = self.menu.as_ref().filter(|(_, at)| self.artillery && now_ms.saturating_sub(*at) <= 2 * READ_INTERVAL_MS)?;
        let (Some(ammo), Some(weapon)) = (&self.ammo, &self.weapon) else { return None };
        let cards = menu
            .cards
            .iter()
            .enumerate()
            .map(|(slot, card)| {
                let rows_shown = matches!(card.charge, CardCharge::Selected(_));
                let families = if rows_shown {
                    let shown = self.tracker.charges().get(slot).copied().filter(|c| matches!(c, SlotCharge::Selected(_)));
                    let charge = shown.or(self.last_selected.get(slot).copied().flatten()).unwrap_or(SlotCharge::Unknown);
                    // One family per kind (fuze variants of one kind are one family).
                    let mut kinds: Vec<&Shell> = Vec::new();
                    for shell in self.card_shells(Loaded { slot, charge }) {
                        match kinds.iter_mut().find(|k| k.base_name() == shell.base_name()) {
                            Some(k) if shell.name.len() < k.name.len() => *k = shell,
                            Some(_) => {}
                            None => kinds.push(shell),
                        }
                    }
                    kinds.into_iter().map(|shell| ammo.charge_family(weapon, shell).into_iter().cloned().collect()).collect()
                } else {
                    Vec::new()
                };
                MenuCard { centre_x: card.centre_x, rows_shown, families }
            })
            .collect();
        Some((menu.seat, weapon.clone(), cards))
    }

    /// Slot count of the loader seat's menu or bar, while it shows (the panel's place there) and
    /// the weapon is one of the artillery data's: tanks have a menu built the same way (in a tank
    /// match it read as "loaded, no charge" with no weapon recognised).
    pub fn loader_layout(&self, now_ms: u64) -> Option<usize> {
        if !self.artillery {
            return None;
        }
        self.loader.filter(|&(_, t)| now_ms.saturating_sub(t) <= LOADER_SEEN_MS).map(|(n, _)| n)
    }

    /// The weapon's elevation range where it was checked against the readout.
    pub fn elevation_range(&self) -> Option<(f64, f64)> {
        self.weapon.as_deref().and_then(mtc_ammo::elevation_limits)
    }

    /// The weapon decided (the data's name).
    pub fn weapon(&self) -> Option<&str> {
        self.weapon.as_deref()
    }

    /// The shell being loaded or loaded last, with the player's picks over what was read.
    pub fn shell(&self) -> Option<&ShellPick> {
        self.chosen.as_ref().or(self.current.as_ref())
    }

    /// A loading is in progress.
    pub fn is_loading(&self) -> bool {
        self.tracker.loading().is_some() && !self.ran_out
    }

    /// A shell was loaded (and no loading started since), or a hidden countdown has run out; and
    /// it has not been fired.
    pub fn is_loaded(&self) -> bool {
        (self.tracker.loaded().is_some() || self.ran_out) && !self.fired
    }

    /// The loaded shell was fired and nothing is loaded yet.
    pub fn is_fired(&self) -> bool {
        self.fired
    }

    /// A shot of a single-shot gun since the last call: the last read with the bar's slot lit and
    /// the first without (ms), the shot between them. Only artillery's: a weapon known not to be
    /// artillery (a tank's gun) is not recorded; one not recognised yet still is.
    pub fn take_shot(&mut self) -> Option<(u64, u64)> {
        let shot = self.shot.take();
        shot.filter(|_| self.artillery || self.weapon.is_none())
    }

    /// M went down (at `key_ms`) on V3 with the menu open: read every frame until it is gone.
    pub fn m_on_v3(&mut self, key_ms: u64) {
        self.m_on_v3 = Some(key_ms);
    }

    /// The view left V3 for V1 or V2 (or is about to: M was pressed): the menu on screen (read
    /// within two intervals) is kept; called again as the view does leave, after the menu began to
    /// fade, the one kept a moment ago stays.
    pub fn left_v3(&mut self, now_ms: u64) {
        self.away = true;
        let fresh = self.menu.as_ref().filter(|(_, at)| now_ms.saturating_sub(*at) <= 2 * READ_INTERVAL_MS).map(|(m, _)| (m.clone(), now_ms));
        let recent = self.menu_on_leave.take().filter(|(_, at)| now_ms.saturating_sub(*at) <= KEPT_FROM_KEY_MS);
        self.menu_on_leave = fresh.or(recent);
    }

    /// The view came back to V3 from V1 or V2 (or is about to): the menu kept when it left stands
    /// in until the menu reads again, read from the next frame on.
    pub fn back_on_v3(&mut self, now_ms: u64) {
        self.back_on_v3 = Some(now_ms);
        self.away = false;
        for (_, until) in &mut self.fire_keys {
            if *until == PENDING {
                *until = now_ms + FIRE_CONFIRM_MS;
            }
        }
        if let Some((menu, _)) = self.menu_on_leave.take() {
            self.menu = Some((menu, now_ms));
            self.menu_restored = true;
        }
    }

    /// F presses will come through `fire_key`.
    pub fn keys_live(&mut self) {
        self.keys_live = true;
    }

    /// F, the fire key, went down at `key_ms` (Roblox in front): the shot it may be is confirmed by
    /// the screen within `FIRE_CONFIRM_MS`, timed at the key.
    pub fn fire_key(&mut self, key_ms: u64) {
        self.fire_keys.retain(|&(_, until)| until >= key_ms);
        self.fire_keys.push((key_ms, if self.away { PENDING } else { key_ms + FIRE_CONFIRM_MS }));
    }

    /// Presses the screen did not confirm with the gun loaded, since the last call (ms).
    pub fn take_unconfirmed(&mut self) -> Vec<u64> {
        std::mem::take(&mut self.unconfirmed)
    }

    /// Whether a press at `k` waiting until `until` may still be matched at `now_ms`.
    fn open(k: u64, until: u64, now_ms: u64) -> bool {
        k <= now_ms && now_ms <= until && (until != PENDING || now_ms - k <= AWAY_PENDING_MAX_MS)
    }

    /// The frame region to read back now, if any.
    pub fn wanted(&self, origin: (i32, i32), now_ms: u64) -> Option<(i32, i32, i32, i32)> {
        let eager = self.back_on_v3.is_some_and(|t| now_ms.saturating_sub(t) < BACK_ON_V3_MS)
            || self.m_on_v3.is_some_and(|t| now_ms.saturating_sub(t) < BACK_ON_V3_MS)
            || self.fire_keys.iter().any(|&(k, until)| now_ms.saturating_sub(k) < FIRE_CONFIRM_MS || (until != PENDING && now_ms <= until));
        if !eager && self.last_read.is_some_and(|t| now_ms.saturating_sub(t) < READ_INTERVAL_MS) {
            return None;
        }
        let (x, y, w, h) = mtc_vision::loading::LOADING_REGION;
        Some((origin.0 + x, origin.1 + y, w, h))
    }

    /// One read-back (or full) frame. `label_x`: where the angle readout's label is (gunner
    /// seat), for the bar layout. `full_map`: V1 is open (the bar is collapsed, its label moved).
    pub fn process(&mut self, view: &FrameView, origin: (i32, i32), label_x: Option<i32>, full_map: bool, now_ms: u64) {
        self.last_read = Some(now_ms);
        // The data a background retry got on a first start (`retry_ammo_data`).
        if self.ammo.is_none()
            && let Some(data) = self.ammo_arrived.lock().ok().and_then(|mut slot| slot.take())
        {
            self.labels = data.label_texts();
            self.ammo = Some(data);
            log::info!("Ammunition data: using the copy a retry downloaded");
        }
        // A press the screen did not confirm while the gun read loaded may be a shot missed; only
        // for artillery (a tank's or a machine gun's F printed it 558 times in one match).
        if let Some(&(k, until)) = self.fire_keys.iter().rev().find(|&&(k, until)| !Self::open(k, until, now_ms) && k <= now_ms)
            && !self.awaiting_load
            && self.artillery
        {
            let when = if until == PENDING || until > k + FIRE_CONFIRM_MS { " (pressed on the full or scope map, waited until back on the minimap view)".to_owned() } else { String::new() };
            log::info!("F with no shot confirmed on screen within {FIRE_CONFIRM_MS} ms{when} ({:.1} s): recorded later if an X matches the prediction", k as f64 / 1000.0);
            self.unconfirmed.push(k);
        }
        self.fire_keys.retain(|&(k, until)| Self::open(k, until, now_ms) || k > now_ms);
        let menu = read_menu(view, origin);
        let loader_bar = read_loader_bar(view, origin);
        let layout = label_x.and_then(|x| self.bar.layout(view, origin, x));
        let gunner_slot = layout.as_ref().and_then(|l| gunner_bar_highlight(view, origin, l));
        if let (Some(x), Some(l)) = (label_x, layout) {
            self.open_bar = Some((x, l));
        }
        // Whether the gunner's bar shows a lit slot: the open bar without the menu (which hides
        // the highlight), or under the full map the collapsed bar in the open bar's columns.
        let lit = if full_map {
            self.open_bar.filter(|(x, _)| label_x == Some(*x)).and_then(|(_, l)| gunner_strip_highlight(view, origin, &l))
        } else if menu.is_none() {
            layout.map(|_| gunner_slot)
        } else {
            None
        };
        // The loaded slot's charge from its capsule (1 bag Low, 2 Medium, 3 High). A weapon whose
        // shells have no charges shows no capsule or a single bag (TOS-1A none, M270 and BM-21
        // one), which says nothing of a charge.
        let uncharged = self.ammo.as_ref().zip(self.weapon.as_ref()).is_some_and(|(a, w)| {
            let shells = a.shells(w);
            !shells.is_empty() && shells.iter().all(|s| s.charge().is_none())
        });
        let bar_loaded = layout.as_ref().zip(gunner_slot).and_then(|(l, slot)| {
            let charge = match (uncharged, gunner_bar_bags(view, origin, l, slot)) {
                (true, 0 | 1) => SlotCharge::None,
                (false, 1) => SlotCharge::Selected(Charge::Low),
                (false, 2) => SlotCharge::Selected(Charge::Medium),
                (false, 3) => SlotCharge::Selected(Charge::High),
                _ => return None,
            };
            // Only this weapon's shell types compete (cluster and incendiary look alike).
            let kinds: Option<Vec<&str>> = self.ammo.as_ref().zip(self.weapon.as_ref()).map(|(a, w)| {
                let mut k: Vec<&str> = a.shells(w).iter().map(|s| s.kind.as_str()).collect();
                k.sort_unstable();
                k.dedup();
                k
            });
            let kind = self.icons.read_among(view, origin, l, slot, kinds.as_deref()).map(|m| m.kind);
            Some(BarShell { loaded: Loaded { slot, charge }, kind })
        });
        let countdown_bar = self.countdown_bar(view, origin);
        let countdown = countdown_bar.is_some_and(|bar| countdown_shown_in(view, origin, &bar));
        let progress = countdown_bar.filter(|_| countdown).and_then(|bar| countdown_progress_in(view, origin, &bar));
        let frame = LoadingFrame { t_ms: now_ms, menu: menu.as_ref(), countdown, progress, gunner_slot, loader_bar: loader_bar.as_ref() };
        let events = self.tracker.observe(&frame);
        // A countdown hidden before its end (the full map opened over it, the bar collapsed): once
        // past its predicted end, loaded as far as the panel tells; the bar confirms it later.
        self.ran_out = !countdown && self.tracker.predicted_end_ms().is_some_and(|end| now_ms > end + RAN_OUT_MARGIN_MS);
        let loader_slots = match (&menu, &loader_bar) {
            (Some(m), _) if m.seat != MenuSeat::Gunner => Some(m.cards.len()),
            (_, Some(bar)) => Some(bar.slots),
            _ => None,
        };
        if let Some(n) = loader_slots {
            self.loader = match self.loader {
                Some((m, _)) if m != n => {
                    let reads = match self.loader_pending {
                        Some((p, c)) if p == n => c + 1,
                        _ => 1,
                    };
                    if reads >= LOADER_SWITCH_READS {
                        self.loader_pending = None;
                        Some((n, now_ms))
                    } else {
                        self.loader_pending = Some((n, reads));
                        Some((m, now_ms))
                    }
                }
                _ => {
                    self.loader_pending = None;
                    Some((n, now_ms))
                }
            };
        }
        if let Some(key) = self.m_on_v3 {
            let after = now_ms.saturating_sub(key);
            if menu.is_none() {
                log::info!("Loading menu: gone {after} ms after M");
                self.m_on_v3 = None;
            } else if after >= BACK_ON_V3_MS {
                log::info!("Loading menu: still read {BACK_ON_V3_MS} ms after M");
                self.m_on_v3 = None;
            }
        }
        let fading_in = self.back_on_v3.is_some_and(|back| now_ms.saturating_sub(back) < BACK_ON_V3_MS);
        match (&menu, self.menu.as_mut()) {
            // Back from V1 or V2, the menu still fading in: the one kept stands in.
            (None, Some((_, at))) if fading_in && self.menu_restored => *at = now_ms,
            _ => self.menu = menu.clone().map(|m| (m, now_ms)),
        }
        if let Some(back) = self.back_on_v3 {
            let shown = if self.menu_restored { ", the menu from before leaving shown meanwhile" } else { "" };
            if menu.is_some() {
                log::info!("Loading menu: read {} ms after back on V3{shown}", now_ms.saturating_sub(back));
                self.back_on_v3 = None;
                self.menu_restored = false;
            } else if !fading_in {
                if self.menu_restored {
                    log::info!("Loading menu: not read {BACK_ON_V3_MS} ms after back on V3, the menu from before dropped");
                }
                self.back_on_v3 = None;
                self.menu_restored = false;
            }
        }
        if let Some(menu) = &menu {
            self.charge_unread = menu.cards.iter().map(|card| card.charge == CardCharge::Hidden && !(1..=3).contains(&card.bags)).collect();
            self.read_texts(view, origin, menu, now_ms);
            self.last_selected.resize(menu.cards.len(), None);
            for (slot, &charge) in self.tracker.charges().iter().enumerate() {
                if matches!(charge, SlotCharge::Selected(_)) && slot < self.last_selected.len() {
                    self.last_selected[slot] = Some(charge);
                }
            }
            self.report_charges();
            self.decide_weapon();
        }
        let every = if self.weapon.is_none() { WEAPON_SEARCH_MS } else { WEAPON_RECHECK_MS };
        let due = self.last_weapon_read.is_none_or(|t| now_ms.saturating_sub(t) >= every);
        // Under the full map there is no label where it is read (the vehicle cannot change there).
        if due && !full_map {
            self.read_weapon(view, origin, now_ms);
        }
        // A shot by how the game shows it: with no loader in his seat the loading menu pops up the
        // moment the shell leaves (nothing is loaded) and the player picks the next one at once,
        // often within the second, so the lit slot may never read dark;
        // with a loader the menu never shows, the slot goes dark until he has loaded; on the full
        // map the gunner cannot load, so the collapsed bar's slot stays dark. So after a lit slot
        // (or a loading seen through), whichever comes first is the shot: the menu popping up, a
        // loading starting, or the slot dark on two reads in a row (below).
        let loaded_before = self.tracker.loaded().is_some() || self.ran_out || self.lit_since_shot;
        let menu_popped = menu.is_some() && !self.menu_was_shown;
        self.menu_was_shown = menu.is_some();
        let started = events.iter().any(|e| matches!(e, LoadingEvent::Started { .. }));
        if loaded_before && !self.awaiting_load && (menu_popped || started) {
            let what = if menu_popped { "loading menu popped up" } else { "loading started" };
            let dark = self.first_dark_ms.filter(|&t| self.fired_reads > 0 && t <= now_ms).unwrap_or(now_ms);
            let by_screen = (self.last_lit_ms.filter(|&t| t <= dark).unwrap_or(dark), now_ms);
            self.lit_since_shot = false;
            let keys = self.no_key_note(by_screen, now_ms);
            match self.timed_by_key(by_screen, now_ms) {
                Some(shot) => {
                    self.shot = Some(shot);
                    self.fired = true;
                    self.fired_reads = 0;
                    self.awaiting_load = true;
                    self.reload_seen = false;
                    log::info!("Fired: {what}{}", self.key_note(now_ms));
                }
                None => log::info!("{what} with no F pressed before: no shot recorded (seat or vehicle change, or a menu opened){keys}"),
            }
        }
        // A loading started: whatever was fired, the next shell is coming.
        if started {
            self.fired = false;
        }
        for event in events {
            match event {
                LoadingEvent::Started { slot } => {
                    // Another loading: a charge picked by hand was for the shot before.
                    self.manual_charge = None;
                    match slot {
                        Some(s) => {
                            let charge = self.tracker.charges().get(s).copied().unwrap_or(SlotCharge::Unknown);
                            let loaded = self.with_shown_charge(Loaded { slot: s, charge });
                            let known = self.charge_read(s) || loaded.charge == SlotCharge::None;
                            log::info!("Loading: slot {}{}", s + 1, if known { "" } else { " (its charge not read for this loading)" });
                            self.note_loading(loaded, known);
                            self.pick(loaded, known);
                        }
                        None => log::info!("Loading"),
                    }
                }
                LoadingEvent::Loaded(loaded) => {
                    self.fired = false;
                    self.awaiting_load = false;
                    let loaded = self.with_shown_charge(loaded);
                    let known = self.charge_read(loaded.slot) || loaded.charge == SlotCharge::None;
                    log::info!("Loaded: {}{}", self.describe(loaded), if known { "" } else { " (charge not read for this loading: shown as unknown)" });
                    crate::diag::note(crate::diag::Event::Load);
                    self.note_loading(loaded, known);
                    self.pick(loaded, known);
                    self.bar_check = true;
                }
            }
        }
        if self.awaiting_load {
            // A menu after the shot: the gunner reloads, and seeing that takes his countdown.
            if menu.is_some() {
                self.reload_seen = false;
            }
            if countdown {
                self.reload_seen = true;
            }
        }
        let loaded_before = (self.tracker.loaded().is_some() || self.ran_out || self.lit_since_shot) && !started;
        match lit {
            Some(Some(_)) => {
                // Lit with no menu and no countdown after the reload was seen: the next shell is
                // in. Its loading's end may not show as an end (sparse recordings, or the full map
                // over it; waiting for it once missed a shot). The slot reads lit under the menu
                // that pops at the shot and in the moment between picking the shell and the
                // countdown (which once counted three loadings again), and a menu read during the
                // loading is not another shot.
                if self.awaiting_load && self.reload_seen && !countdown && menu.is_none() {
                    self.awaiting_load = false;
                }
                self.fired_reads = 0;
                if !self.awaiting_load {
                    self.fired = false;
                    self.last_lit_ms = Some(now_ms);
                    let from = *self.lit_run_from.get_or_insert(now_ms);
                    if now_ms.saturating_sub(from) >= ARM_LIT_MS {
                        self.lit_since_shot = true;
                    }
                }
            }
            Some(None) if self.awaiting_load => {
                // Dark with no menu: a loader's reload (on the full map the collapsed bar is dark
                // whatever is loaded).
                if !full_map && menu.is_none() {
                    self.reload_seen = true;
                }
                self.lit_run_from = None;
            }
            Some(None) if loaded_before && !countdown && !self.fired => {
                self.lit_run_from = None;
                self.fired_reads += 1;
                if self.fired_reads == 1 {
                    self.first_dark_ms = Some(now_ms);
                }
                if self.fired_reads >= FIRED_READS {
                    // `fired` either way: one decision per dark spell.
                    self.fired = true;
                    self.fired_reads = 0;
                    self.lit_since_shot = false;
                    let dark = self.first_dark_ms.unwrap_or(now_ms);
                    let by_screen = (self.last_lit_ms.filter(|&t| t <= dark).unwrap_or(dark), dark);
                    let keys = self.no_key_note(by_screen, now_ms);
                    match self.timed_by_key(by_screen, now_ms) {
                        Some(shot) => {
                            self.awaiting_load = true;
                            self.reload_seen = false;
                            self.shot = Some(shot);
                            log::info!("Fired: no lit slot on the ammo bar, waiting for the next loading{}", self.key_note(now_ms));
                        }
                        None => log::info!("No lit slot on the ammo bar with no F pressed before: no shot recorded{keys}"),
                    }
                }
            }
            Some(None) => {
                self.fired_reads = 0;
                self.lit_run_from = None;
            }
            None => self.fired_reads = 0,
        }
        // What the gunner's bar shows loaded is what will be fired: a teammate's loading (no menu
        // on this screen, often no countdown either) or a charge the menu did not show.
        let confirmed = match (bar_loaded.filter(|_| menu.is_none()), self.bar_pending) {
            // The bar as before, after a loading took the menu's charge: the bar's charge stands
            // where they differ (in a recording the menu last read at Medium, the bar's capsule
            // High, and the X landed where High lands).
            (Some(loaded), _) if self.bar_loaded == Some(loaded) => {
                self.bar_pending = None;
                let differs = self.current.as_ref().is_some_and(|c| c.loaded_charge() != level(loaded.loaded.charge));
                (std::mem::take(&mut self.bar_check) && differs && loaded.kind.is_some()).then_some(loaded)
            }
            (Some(loaded), Some((pending, reads))) if pending == loaded => {
                self.bar_pending = Some((loaded, reads + 1));
                (reads + 1 >= BAR_SWITCH_READS).then_some(loaded)
            }
            (Some(loaded), _) => {
                self.bar_pending = Some((loaded, 1));
                (BAR_SWITCH_READS <= 1).then_some(loaded)
            }
            (None, _) => None,
        };
        if let Some(bar) = confirmed {
            self.bar_pending = None;
            self.bar_loaded = Some(bar);
            self.bar_check = false;
            // The shell from the icon's type and the capsule's charge, never from cards read
            // earlier: with a loader aboard the gunner may never see the cards, and in older game
            // versions the bar's order changed with the loader seat.
            self.note_loading(bar.loaded, true);
            if !self.current_is(bar) {
                let loaded = bar.loaded;
                match self.pick_by_kind(bar) {
                    Some(name) => log::info!("Ammo bar: slot {} · {} · icon {} → {name}", loaded.slot + 1, charge_name(loaded.charge), bar.kind.unwrap_or("")),
                    None => {
                        log::info!("Ammo bar: slot {} · {} · icon {} (shell not decided)", loaded.slot + 1, charge_name(loaded.charge), bar.kind.unwrap_or("not recognised"));
                        self.current = None;
                    }
                }
            }
        }
        self.apply_manual();
    }

    /// The current shell is the bar's: its slot, at its charge, of its icon's type.
    fn current_is(&self, bar: BarShell) -> bool {
        self.current.as_ref().is_some_and(|c| c.slot == bar.loaded.slot && c.charge_known && c.shell.charge() == level(bar.loaded.charge) && bar.kind.is_some_and(|k| c.shell.kind == k))
    }

    /// Makes the shell of the bar's type and charge the current one (one ballistic choice); its
    /// name, `None` when the type is not read or does not decide.
    fn pick_by_kind(&mut self, bar: BarShell) -> Option<String> {
        let (Some(ammo), Some(weapon)) = (&self.ammo, &self.weapon) else { return None };
        // The icon not read: the weapon's shells at this charge, when they all fly alike (the
        // TOS-1A's three rockets, whose icons the library has but the bar did not decide).
        let candidates: Vec<&mtc_ammo::Shell> = match bar.kind {
            Some(kind) => ammo.shells_of_kind(weapon, kind, level(bar.loaded.charge)),
            None => ammo.shells(weapon).iter().filter(|s| s.charge() == level(bar.loaded.charge)).collect(),
        };
        if candidates.is_empty() || !same_ballistics(&candidates) {
            return None;
        }
        let shell = candidates.iter().min_by_key(|s| s.name.len())?;
        let pick = ShellPick {
            weapon: weapon.clone(),
            slot: bar.loaded.slot,
            shell: (*shell).clone(),
            family: ammo.charge_family(weapon, shell).into_iter().cloned().collect(),
            by_hand: false,
            charge_known: true,
            charge_by_hand: false,
        };
        let name = pick.shell.name.clone();
        self.current = Some(pick);
        Some(name)
    }

    /// A loaded slot whose charge read as none while a charge was shown on it before (the
    /// fading menu): that charge.
    fn with_shown_charge(&self, loaded: Loaded) -> Loaded {
        match (loaded.charge, self.last_selected.get(loaded.slot).copied().flatten()) {
            (SlotCharge::None | SlotCharge::Unknown, Some(shown)) => Loaded { charge: shown, ..loaded },
            _ => loaded,
        }
    }

    /// Adds the cycles since `since` to cost slot `slot` as one call.
    fn count(&mut self, slot: usize, since: u64) {
        self.cost[slot].0 += crate::stats::cycles().saturating_sub(since);
        self.cost[slot].1 += 1;
    }

    fn read_weapon(&mut self, view: &FrameView, origin: (i32, i32), now_ms: u64) {
        // An attempt counts whether or not a label shows (none under the full map): at most one
        // per `WEAPON_RECHECK_MS`.
        self.last_weapon_read = Some(now_ms);
        let c = crate::stats::cycles();
        let label = read_weapon_label_among(view, origin, &self.reader, &self.labels);
        self.count(1, c);
        let (Some(_), Some(label)) = (&self.ammo, label) else { return };
        // The label cut off (as while the 2S33 loads) is the same weapon, not a switch.
        if self.vehicle_label.as_deref().is_some_and(|current| mtc_ammo::cut_off_label(&label, current)) {
            return;
        }
        // Two misreadings of one gun name the same data weapon: not a switch ("165 mm L941" and
        // "165 mm 29A1" for the AVRE's "165 mm L9A1" used to reset its loaded shell).
        if let (Some(current), Some(ammo)) = (self.vehicle_label.as_deref(), &self.ammo)
            && let Some(weapon) = ammo.weapon_for_label(&label)
            && ammo.weapon_for_label(current) == Some(weapon)
        {
            self.label_change = None;
            return;
        }
        // A label of the current calibre naming no weapon is the current one partly covered: a game
        // notice ("Match progression bonus") over "220 mm TOS-1A" read "220 mm T6" and reset the
        // loading five times in one match.
        if let (Some(current), Some(ammo)) = (self.vehicle_label.as_deref(), &self.ammo)
            && mtc_ammo::same_calibre(&label, current)
            && ammo.weapon_for_label(&label).is_none()
        {
            return;
        }
        match &self.vehicle_label {
            // A label with glyphs that could not be read (`?`) is the current one partly covered or
            // too soft to read, not another weapon: a game notice over "152.4 mm 2A64" read
            // "1524 mm2-? ka?" and switched the 2S19 away and back (100 % scaling, 1920x1080).
            // Another weapon's label reads whole against the data's labels.
            Some(current) if !mtc_ammo::same_weapon_label(current, &label) && label.contains('?') => return,
            Some(current) if !mtc_ammo::same_weapon_label(current, &label) => {
                let reads = match &self.label_change {
                    Some((pending, n)) if mtc_ammo::same_weapon_label(pending, &label) => n + 1,
                    _ => 1,
                };
                if reads < LABEL_SWITCH_READS {
                    self.label_change = Some((label, reads));
                    return;
                }
                self.switch_weapon(&label);
            }
            Some(_) => self.label_change = None,
            None => self.vehicle_label = Some(label.clone()),
        }
        self.weapon_label = Some(label);
        self.decide_weapon();
    }

    /// Another weapon on screen (another vehicle or seat): the old weapon's cards, charges and
    /// loaded shell go, and nothing shows until the new one is decided.
    fn switch_weapon(&mut self, label: &str) {
        log::info!("Weapon changed: screen \"{label}\" (before \"{}\")", self.vehicle_label.as_deref().unwrap_or(""));
        self.vehicle_label = Some(label.to_owned());
        self.label_change = None;
        self.weapon = None;
        self.artillery = false;
        self.cards.clear();
        self.last_selected.clear();
        self.layout = None;
        self.last_text = None;
        self.reported_charges.clear();
        self.current = None;
        self.tracker = LoadingTracker::default();
        self.loader = None;
        self.loader_pending = None;
        self.menu = None;
        self.bar_loaded = None;
        self.bar_pending = None;
        self.fired = false;
        self.fired_reads = 0;
        self.open_bar = None;
        self.lit_since_shot = false;
        self.last_lit_ms = None;
        self.awaiting_load = false;
        self.reload_seen = false;
        self.lit_run_from = None;
        self.menu_was_shown = false;
        self.charge_unread.clear();
        self.loaded_at = None;
        self.manual.clear();
        self.manual_next = None;
        self.manual_charge = None;
        self.weapon_by_hand = false;
        self.chosen = None;
    }

    /// The shot of a screen change: at the first F press within `FIRE_CONFIRM_MS` before it and not
    /// before the slot last read lit (less a read interval: the slot goes dark as the gun fires), or
    /// failing that the last press within `FIRE_CONFIRM_MS` (F is the only key that fires, and the
    /// slot may stay lit after the shot: 0.8–1.4 s on the Centurion AVRE and now and then 0.65 s
    /// on the 2S33, going by where those shots' X appeared); the key's time is the shot's. With no
    /// press, none when presses come through (`keys_live`), else between the screen's reads. Every
    /// press up to now is spent.
    fn timed_by_key(&mut self, by_screen: (u64, u64), now_ms: u64) -> Option<(u64, u64)> {
        let recent: Vec<u64> = self.fire_keys.iter().filter(|&&(k, until)| Self::open(k, until, now_ms)).map(|&(k, _)| k).collect();
        let key = recent.iter().copied().find(|&k| k + READ_INTERVAL_MS >= by_screen.0).or_else(|| recent.last().copied());
        self.fire_keys.retain(|&(k, _)| k > now_ms);
        match key {
            Some(k) => Some((k, k)),
            None if self.keys_live => None,
            None => Some(by_screen),
        }
    }

    /// For a screen change taken as no shot: the F presses received in the last `FIRE_CONFIRM_MS`
    /// and how early a press may be to count. It tells a press that never reached the HUD from one
    /// that came too early for the rule (a shot fired between two loading reads once went
    /// unrecorded although the recorder logged three F presses).
    fn no_key_note(&self, by_screen: (u64, u64), now_ms: u64) -> String {
        let recent: Vec<String> = self.fire_keys.iter().filter(|&&(k, until)| Self::open(k, until, now_ms)).map(|&(k, _)| format!("{} ms ago", now_ms - k)).collect();
        let earliest = (by_screen.0.saturating_sub(READ_INTERVAL_MS) as i64 - now_ms as i64).unsigned_abs();
        if recent.is_empty() {
            format!("; no F in these {FIRE_CONFIRM_MS} ms")
        } else {
            format!("; F received: {}, a shot counts from {earliest} ms ago at the earliest (last lit less one read interval)", recent.join(", "))
        }
    }

    fn key_note(&self, now_ms: u64) -> String {
        match self.shot {
            Some((a, b)) if a == b => format!(" (confirmed on screen {} ms after F)", now_ms.saturating_sub(a)),
            _ => " (no F received, recorded from the screen)".to_owned(),
        }
    }

    /// The weapon from the label and the card names read (a converted weapon differs from the
    /// original by a letter; its cards tell them apart).
    fn decide_weapon(&mut self) {
        let (Some(ammo), Some(label)) = (&self.ammo, &self.weapon_label) else { return };
        let names: Vec<&str> = self.cards.iter().flatten().map(|c| c.name.as_str()).collect();
        let c = crate::stats::cycles();
        let decided = ammo.weapon_for_label_and_cards(label, &names);
        let cycles = crate::stats::cycles().saturating_sub(c);
        self.cost[2].0 += cycles;
        self.cost[2].1 += 1;
        let Some(weapon) = decided else {
            if !label.contains('?') {
                crate::diag::note(crate::diag::Event::UnknownLabel(label.clone()));
            }
            note_unreadable_label(label, self.weapon.is_some());
            return;
        };
        // A gun picked by hand stands while this label is on screen.
        if self.weapon_by_hand {
            return;
        }
        if self.weapon.as_deref() != Some(weapon) {
            log::info!("Weapon: {weapon} (screen \"{label}\", cards {names:?})");
            crate::diag::note(crate::diag::Event::Weapon(weapon.to_owned()));
            // Another weapon (another vehicle) has other cards; a correction by the cards keeps them.
            if self.weapon.is_some() && names.is_empty() {
                self.cards.clear();
                self.last_selected.clear();
            }
            self.weapon = Some(weapon.to_owned());
        }
        // The decided weapon, not the label as read (a misread label decides nothing).
        let artillery = mtc_ammo::is_artillery_weapon(weapon);
        if artillery != self.artillery {
            if !artillery {
                log::info!("Weapon {weapon} is not artillery: no HUD");
            }
            self.artillery = artillery;
        }
    }

    /// Card text when the cards first show or a card's charge changed since it was read.
    fn read_texts(&mut self, view: &FrameView, origin: (i32, i32), menu: &Menu, now_ms: u64) {
        let layout = (menu.seat, menu.cards.len());
        if self.layout.is_some_and(|(_, n)| n != layout.1) {
            // Other cards (another weapon, or a frame caught mid-fade): read them again.
            self.cards.clear();
            self.last_selected.clear();
        }
        self.layout = Some(layout);
        self.cards.resize_with(menu.cards.len(), || None);
        let charges = self.tracker.charges().to_vec();
        let stale = self.cards.iter().zip(&charges).any(|(c, &now)| c.as_ref().is_none_or(|c| c.charge != now && now != SlotCharge::Unknown));
        if !stale && self.weapon.is_some() || self.last_text.is_some_and(|t| now_ms.saturating_sub(t) < TEXT_INTERVAL_MS) {
            return;
        }
        self.last_text = Some(now_ms);
        let c = crate::stats::cycles();
        // Card names compared with the names the cards can show: the decided weapon's shells, or
        // those of every weapon the label can name (a conversion's cards decide between them).
        let weapons: Vec<String> = match (&self.weapon, &self.weapon_label, &self.ammo) {
            (Some(w), _, _) => vec![w.clone()],
            (None, Some(label), Some(ammo)) => ammo.label_weapons(label).into_iter().map(str::to_owned).collect(),
            _ => Vec::new(),
        };
        let weapons: Vec<&str> = weapons.iter().map(String::as_str).collect();
        let ammo = self.ammo.as_ref();
        let texts = read_card_texts_among(view, origin, menu, &self.reader, &|| ammo.map_or_else(Vec::new, |a| a.card_velocities(&weapons)), &|velocity| ammo.map_or_else(Vec::new, |a| a.card_texts(&weapons, velocity)));
        self.count(0, c);
        for ((slot, text), &charge) in self.cards.iter_mut().zip(texts).zip(&charges) {
            if let Some(name) = text.name.filter(|n| n.len() >= 2) {
                *slot = Some(CardRead { name, velocity: text.velocity, charge });
            }
        }
    }

    fn report_charges(&mut self) {
        let charges = self.tracker.charges();
        if charges != self.reported_charges.as_slice() && !charges.contains(&SlotCharge::Unknown) {
            let list: Vec<String> = charges.iter().enumerate().map(|(i, &c)| format!("#{} {}", i + 1, charge_name(c))).collect();
            log::info!("Charges: {}", list.join(" · "));
            self.reported_charges = charges.to_vec();
        }
    }

    /// The shells a loaded slot can be: one ballistic choice, or empty when not decided.
    fn candidates(&self, loaded: Loaded) -> Vec<&Shell> {
        let candidates = self.card_shells(loaded);
        if same_ballistics(&candidates) { candidates } else { Vec::new() }
    }

    /// Every shell the slot's card can be at its charge, flying alike or not: by its name,
    /// velocity and charge, less the kinds other cards of the menu are known to be (a menu holds
    /// each shell once: beside the card "M107", the HE M107, "M107 High..." is the VT or PF one).
    fn card_shells(&self, loaded: Loaded) -> Vec<&Shell> {
        let shells_of = |slot: usize, charge: SlotCharge| -> Vec<&Shell> {
            let (Some(ammo), Some(weapon), Some(Some(card))) = (&self.ammo, &self.weapon, self.cards.get(slot)) else {
                return Vec::new();
            };
            let velocity = card.velocity.filter(|_| card.charge == charge);
            ammo.card_candidates(weapon, &card.name, velocity, level(charge))
        };
        // Even shells that fly alike at this charge: their families may not at the others.
        let shells = shells_of(loaded.slot, loaded.charge);
        if shells.len() < 2 {
            return shells;
        }
        let taken: Vec<String> = (0..self.cards.len())
            .filter(|&slot| slot != loaded.slot)
            .filter_map(|slot| {
                let card = self.cards[slot].as_ref()?;
                let theirs = shells_of(slot, card.charge);
                let kind = theirs.first()?.base_name();
                theirs.iter().all(|s| s.base_name() == kind).then_some(kind)
            })
            .collect();
        let left: Vec<&Shell> = shells.iter().copied().filter(|s| !taken.contains(&s.base_name())).collect();
        if left.is_empty() { shells } else { left }
    }

    /// Makes the slot's shell the current one, when it is known; whether it was. `charge_known`:
    /// the charge was read for this loading (else the shell is found by the charge last seen and
    /// its charge shown as unknown).
    fn pick(&mut self, loaded: Loaded, charge_known: bool) -> bool {
        let candidates = self.candidates(loaded);
        // Fuze variants (HE, VT, PF) fly alike: the shortest name stands for them.
        let (Some(shell), Some(ammo), Some(weapon)) = (candidates.iter().min_by_key(|s| s.name.len()), &self.ammo, &self.weapon) else { return false };
        let pick = ShellPick {
            weapon: weapon.clone(),
            slot: loaded.slot,
            shell: (*shell).clone(),
            family: ammo.charge_family(weapon, shell).into_iter().cloned().collect(),
            by_hand: false,
            charge_known: charge_known || shell.charge().is_none(),
            charge_by_hand: false,
        };
        if self.current.as_ref() != Some(&pick) {
            self.current = Some(pick);
        }
        true
    }

    /// The slot's charge was read on the last menu read (the game keeps each card's charge, so a
    /// charge read long before still stands; one remembered past an unread column may not).
    fn charge_read(&self, slot: usize) -> bool {
        !self.charge_unread.get(slot).copied().unwrap_or(false)
    }

    /// A slot seen loading or loaded: the slot the player's picks go to. A shell picked before any
    /// was seen goes to this one.
    fn note_loading(&mut self, loaded: Loaded, charge_known: bool) {
        self.loaded_at = Some((loaded, charge_known));
        if let Some(name) = self.manual_next.take() {
            self.set_slot_pick(loaded.slot, Some(name));
        }
    }

    fn set_slot_pick(&mut self, slot: usize, shell: Option<String>) {
        if self.manual.len() <= slot {
            self.manual.resize(slot + 1, None);
        }
        self.manual[slot] = shell;
    }

    /// The slot the player's picks go to (`None`: none seen loading yet).
    fn pick_slot(&self) -> Option<usize> {
        self.loaded_at.map(|(l, _)| l.slot)
    }

    /// The shell picked by hand for that slot.
    fn slot_pick(&self) -> Option<&String> {
        match self.pick_slot() {
            Some(slot) => self.manual.get(slot).and_then(Option::as_ref),
            None => self.manual_next.as_ref(),
        }
    }

    /// What was read for the slot the picks go to: the shell (name without its charge) and its
    /// charge, when known.
    fn read_for_slot(&self) -> (Option<String>, Option<ChargeLevel>) {
        let slot = self.pick_slot();
        match self.current.as_ref().filter(|c| Some(c.slot) == slot) {
            Some(c) => (Some(c.shell.base_name()), c.loaded_charge()),
            None => (None, self.loaded_at.filter(|&(_, known)| known).and_then(|(l, _)| level(l.charge))),
        }
    }

    /// The current shell with the player's picks over what was read: the shell picked for the
    /// slot, at the charge read or picked for this shot. With no slot seen loading yet, a pick
    /// stands for whatever is loaded.
    fn apply_manual(&mut self) {
        self.chosen = None;
        let slot = self.pick_slot();
        let picked = self.slot_pick().cloned();
        let charge = self.manual_charge.filter(|&(s, _)| s == slot).map(|(_, c)| c);
        if picked.is_none() && charge.is_none() {
            return;
        }
        let (Some(ammo), Some(weapon)) = (&self.ammo, &self.weapon) else { return };
        let (read_shell, read_charge) = self.read_for_slot();
        let Some(base) = picked.clone().or(read_shell) else { return };
        let shells: Vec<&Shell> = ammo.shells(weapon).iter().filter(|s| s.base_name() == base).collect();
        let charged = shells.iter().any(|s| s.charge().is_some());
        let want = charge.or(read_charge);
        let shell = match want {
            Some(c) if charged => shells.iter().find(|s| s.charge() == Some(c)).or(shells.first()),
            _ => shells.first(),
        };
        let Some(shell) = shell else { return };
        self.chosen = Some(ShellPick {
            weapon: weapon.clone(),
            slot: slot.unwrap_or_default(),
            shell: (*shell).clone(),
            family: ammo.charge_family(weapon, shell).into_iter().cloned().collect(),
            by_hand: picked.is_some(),
            charge_known: !charged || shell.charge().is_some() && shell.charge() == want,
            charge_by_hand: charge.is_some(),
        });
    }

    /// A row of the shell or charge list was clicked: the pick taken (paging is the caller's).
    pub fn pick_from_list(&mut self, pick: &MenuPick) {
        match pick {
            MenuPick::Auto => self.set_manual_shell(None),
            MenuPick::Shell(name) => self.set_manual_shell(Some(name.clone())),
            MenuPick::Gun(name) => self.set_manual_weapon(name),
            MenuPick::Charge(index) => self.set_manual_charge(index.map(|i| ChargeLevel::ALL[usize::from(i.min(2))])),
            MenuPick::More(_) | MenuPick::Back(_) => {}
        }
        self.apply_manual();
    }

    /// The shell picked for the slot the picks go to (`None`: as read again).
    fn set_manual_shell(&mut self, shell: Option<String>) {
        let slot = self.pick_slot();
        let (read, _) = self.read_for_slot();
        let card = slot.and_then(|s| self.cards.get(s)).and_then(Option::as_ref).map(|c| c.name.clone());
        match slot {
            Some(s) => self.set_slot_pick(s, shell.clone()),
            None => self.manual_next = shell.clone(),
        }
        let at = slot.map_or("the next slot loaded".to_owned(), |s| format!("slot {}", s + 1));
        log::info!("Shell picked by hand: {} for {at} (read {}, card {})", shell.as_deref().unwrap_or("Auto"), read.as_deref().unwrap_or("nothing"), card.as_deref().unwrap_or("not read"));
        crate::diag::note(crate::diag::Event::ManualPick(crate::diag::ManualPick { what: "shell", weapon: self.weapon.clone(), slot: slot.map(|s| s as u32 + 1), picked: shell, read, card }));
    }

    /// This shot's charge picked (`None`: as read again).
    fn set_manual_charge(&mut self, charge: Option<ChargeLevel>) {
        let slot = self.pick_slot();
        let (_, read) = self.read_for_slot();
        self.manual_charge = charge.map(|c| (slot, c));
        log::info!("Charge picked by hand: {} (read {})", charge.map_or("Auto", charge_word), read.map_or("nothing", charge_word));
        let picked = charge.map(|c| charge_word(c).to_owned());
        crate::diag::note(crate::diag::Event::ManualPick(crate::diag::ManualPick { what: "charge", weapon: self.weapon.clone(), slot: slot.map(|s| s as u32 + 1), picked, read: read.map(|c| charge_word(c).to_owned()), card: None }));
    }

    /// The gun picked by hand: its cards read and its shells listed from now on, until another
    /// weapon's label is read.
    fn set_manual_weapon(&mut self, weapon: &str) {
        log::info!("Gun picked by hand: {weapon} (recognised {}, label {})", self.weapon.as_deref().unwrap_or("none"), self.weapon_label.as_deref().unwrap_or("not read"));
        crate::diag::note(crate::diag::Event::ManualPick(crate::diag::ManualPick { what: "gun", weapon: Some(weapon.to_owned()), slot: None, picked: Some(weapon.to_owned()), read: self.weapon.clone(), card: self.weapon_label.clone() }));
        if self.weapon.as_deref() != Some(weapon) {
            // The other gun's shells, cards and bar reading go: read again as this gun's.
            self.manual.clear();
            self.manual_next = None;
            self.manual_charge = None;
            self.current = None;
            self.cards.clear();
            self.last_selected.clear();
            self.last_text = None;
            self.bar_loaded = None;
            self.bar_pending = None;
        }
        self.weapon = Some(weapon.to_owned());
        self.weapon_by_hand = true;
        self.artillery = mtc_ammo::is_artillery_weapon(weapon);
    }

    /// The page the shell list opens on: the shells, or the guns while no gun is known.
    pub fn first_page(&self) -> ListPage {
        if self.weapon.is_some() { ListPage::Shells } else { ListPage::Guns(0) }
    }

    /// Every artillery gun with shells in the data, for the gun list: those of the calibre the
    /// weapon label reads first, then by calibre.
    fn gun_order(&self) -> Vec<String> {
        let Some(ammo) = &self.ammo else { return Vec::new() };
        let read = [self.weapon_label.as_deref(), self.vehicle_label.as_deref(), self.weapon.as_deref()].into_iter().flatten().find_map(mtc_ammo::calibre);
        let mut guns: Vec<(bool, u32, &str)> = ammo.artillery_weapons().into_iter().map(|g| {
            let calibre = mtc_ammo::calibre(g);
            (read.is_none() || calibre != read, calibre.unwrap_or(u32::MAX), g)
        }).collect();
        guns.sort_unstable();
        guns.into_iter().map(|(_, _, g)| g.to_owned()).collect()
    }

    /// The shell list on `page` (`None` with no ammunition data): "Auto" with what was read, the
    /// gun's shells with their icons, another gun; or a page of guns with what carries them.
    pub fn shell_list(&self, page: ListPage, open: bool) -> Option<PanelMenu> {
        let ammo = self.ammo.as_ref()?;
        let picked = self.slot_pick();
        let row = |label: String, kind: Option<String>, detail: Option<String>, current: bool, pick: MenuPick| MenuItem { label, kind, detail, current, pick };
        let mut items = Vec::new();
        // With no gun known the list is the guns'.
        let page = match (page, &self.weapon) {
            (ListPage::Shells, None) => ListPage::Guns(0),
            (page, _) => page,
        };
        match page {
            ListPage::Shells => {
                let weapon = self.weapon.as_ref()?;
                let (read, _) = self.read_for_slot();
                items.push(row("Auto".into(), None, Some(read.map_or("not read".to_owned(), |r| format!("read {r}"))), picked.is_none(), MenuPick::Auto));
                let mut listed: Vec<String> = Vec::new();
                for shell in ammo.shells(weapon) {
                    let base = shell.base_name();
                    if !listed.contains(&base) {
                        items.push(row(base.clone(), Some(shell.kind.clone()), None, picked == Some(&base), MenuPick::Shell(base.clone())));
                        listed.push(base);
                    }
                }
                items.push(row("Other gun".into(), None, None, false, MenuPick::More(0)));
            }
            ListPage::Guns(start) => {
                let guns = self.gun_order();
                if start > 0 {
                    items.push(row("Back".into(), None, None, false, MenuPick::Back(Some(start.saturating_sub(GUNS_PER_PAGE)))));
                } else if self.weapon.is_some() {
                    items.push(row("Shells".into(), None, None, false, MenuPick::Back(None)));
                }
                for gun in guns.iter().skip(start).take(GUNS_PER_PAGE) {
                    let detail = mtc_ammo::carriers(gun).first().cloned();
                    items.push(row(gun.clone(), None, detail, self.weapon.as_deref() == Some(gun.as_str()), MenuPick::Gun(gun.clone())));
                }
                if guns.len() > start + GUNS_PER_PAGE {
                    items.push(row("More guns".into(), None, None, false, MenuPick::More(start + GUNS_PER_PAGE)));
                }
            }
        }
        Some(PanelMenu { open, items, manual: picked.is_some() })
    }

    /// The charge list for the current shell (`None` when it has no charges): "Auto" with the
    /// charge read, then each charge.
    pub fn charge_list(&self, open: bool) -> Option<PanelMenu> {
        let shell = self.shell()?;
        if shell.shell.charge().is_none() {
            return None;
        }
        let slot = self.pick_slot();
        let picked = self.manual_charge.filter(|&(s, _)| s == slot).map(|(_, c)| c);
        let (_, read) = self.read_for_slot();
        let mut items = vec![MenuItem { label: "Auto".into(), kind: None, detail: Some(read.map_or("not read".to_owned(), |c| format!("read {}", charge_word(c)))), current: picked.is_none(), pick: MenuPick::Charge(None) }];
        for member in &shell.family {
            let Some(c) = member.charge() else { continue };
            items.push(MenuItem { label: charge_word(c).into(), kind: None, detail: None, current: picked == Some(c), pick: MenuPick::Charge(Some(c as u8)) });
        }
        Some(PanelMenu { open, items, manual: picked.is_some() })
    }

    /// "slot 2 · High · M107 High Charge · 684 m/s", or the slot and charge when the shell is not
    /// known.
    fn describe(&self, loaded: Loaded) -> String {
        let plain = format!("slot {} · {}", loaded.slot + 1, charge_name(loaded.charge));
        let candidates = self.candidates(loaded);
        match candidates.iter().min_by_key(|s| s.name.len()) {
            Some(shell) => {
                let alike = if candidates.len() > 1 { format!(" (and {} with the same trajectory)", candidates.len() - 1) } else { String::new() };
                format!("{plain} · {}{alike} · {:.0} m/s", shell.name, shell.velocity)
            }
            None => match self.cards.get(loaded.slot) {
                Some(Some(card)) if self.ammo.is_some() && self.weapon.is_some() => format!("{plain} · shell not decided (card \"{}\")", card.name),
                _ => plain,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    /// Weapon names in the game's font have a calibre in mm; what a font pack's glyphs read as
    /// does not.
    #[test]
    fn weapon_label_shape() {
        for label in ["155 mm M185", "152.4 mm 2A64", "120 mm Rh120 L/44"] {
            assert!(super::shaped_like_weapon_label(label), "{label}");
        }
        for label in ["1 o5 nn Y113o", "tJ", "ELuATr", "h(ULUlVUU jUFfVLdD", "155 mm"] {
            assert!(!super::shaped_like_weapon_label(label), "{label}");
        }
    }

    use super::*;

    fn low(slot: usize) -> Loaded {
        Loaded { slot, charge: SlotCharge::Selected(Charge::Low) }
    }

    /// A shell picked by hand holds for its slot, at the charge read, until Auto; another slot is
    /// read as before; a pick before any loading goes to the next slot loaded.
    #[test]
    fn a_shell_picked_by_hand_stays_with_its_slot() {
        let mut p = LoadingPipeline::new();
        if p.ammo.is_none() {
            eprintln!("skipped: no ammunition data on this machine");
            return;
        }
        p.weapon = Some("155mm M284".into());
        p.note_loading(low(1), true);
        p.pick_from_list(&MenuPick::Shell("M110".into()));
        let shell = p.shell().expect("picked");
        assert_eq!((shell.shell.base_name().as_str(), shell.loaded_charge(), shell.by_hand), ("M110", Some(ChargeLevel::Low), true));
        p.note_loading(low(2), true);
        p.apply_manual();
        assert!(p.shell().is_none(), "slot 3 is read, and nothing was");
        p.note_loading(low(1), true);
        p.apply_manual();
        assert_eq!(p.shell().map(|s| s.shell.base_name()), Some("M110".into()), "slot 2 again");
        p.pick_from_list(&MenuPick::Auto);
        assert!(p.shell().is_none(), "Auto: as read");

        let mut p = LoadingPipeline::new();
        p.weapon = Some("155mm M284".into());
        p.pick_from_list(&MenuPick::Shell("M107 VT".into()));
        assert_eq!(p.shell().map(|s| (s.shell.base_name(), s.charge_known)), Some(("M107 VT".into(), false)), "before any loading: charge unknown");
        p.note_loading(low(0), true);
        p.apply_manual();
        assert_eq!(p.manual.first().cloned().flatten().as_deref(), Some("M107 VT"), "the next slot loaded took it");
        assert_eq!(p.shell().and_then(ShellPick::loaded_charge), Some(ChargeLevel::Low));
    }

    /// A charge picked by hand is this shot's; a gun picked by hand stands against the label it
    /// was picked over and goes with another weapon's.
    #[test]
    fn charges_and_guns_picked_by_hand() {
        let mut p = LoadingPipeline::new();
        if p.ammo.is_none() {
            eprintln!("skipped: no ammunition data on this machine");
            return;
        }
        p.weapon = Some("155mm M284".into());
        p.note_loading(Loaded { slot: 0, charge: SlotCharge::Unknown }, false);
        p.pick_from_list(&MenuPick::Shell("M107".into()));
        assert_eq!(p.shell().map(|s| s.charge_known), Some(false), "charge not read");
        p.pick_from_list(&MenuPick::Charge(Some(2)));
        let shell = p.shell().expect("picked");
        assert_eq!((shell.loaded_charge(), shell.charge_by_hand), (Some(ChargeLevel::High), true));
        assert!(p.charge_list(false).is_some_and(|m| m.manual));
        p.note_loading(low(1), true);
        p.apply_manual();
        assert!(p.shell().is_none(), "another slot: the charge was the other shot's");

        let mut p = LoadingPipeline::new();
        p.weapon_label = Some("155 mm M284".into());
        p.pick_from_list(&MenuPick::Gun("155mm M185".into()));
        assert_eq!((p.weapon.as_deref(), p.artillery), (Some("155mm M185"), true));
        p.decide_weapon();
        assert_eq!(p.weapon.as_deref(), Some("155mm M185"), "the label it was picked over");
        let guns = p.gun_order();
        assert!(guns.iter().take(3).all(|g| mtc_ammo::calibre(g) == Some(155)), "the label's calibre first: {guns:?}");
        p.switch_weapon("152.4 mm 2A64");
        assert!(!p.weapon_by_hand && p.weapon.is_none(), "another weapon's label");
    }

    /// A slot's charge counts for a loading unless the last menu read left it unread.
    #[test]
    fn a_charge_remembered_past_an_unread_column_is_unknown() {
        let mut p = LoadingPipeline::new();
        assert!(p.charge_read(0), "no menu read: as before");
        p.charge_unread = vec![false, true];
        assert!(p.charge_read(0));
        assert!(!p.charge_read(1));
    }

    #[test]
    fn only_artillery_shots_are_recorded() {
        let mut p = LoadingPipeline::new();
        let shot = |p: &mut LoadingPipeline, weapon: Option<&str>, artillery: bool| {
            (p.weapon, p.artillery, p.shot) = (weapon.map(str::to_owned), artillery, Some((1_000, 1_100)));
            p.take_shot()
        };
        assert_eq!(shot(&mut p, Some("155mm M284"), true), Some((1_000, 1_100)));
        assert_eq!(shot(&mut p, Some("120mm TKG"), false), None, "a tank's gun");
        assert_eq!(shot(&mut p, None, false), Some((1_000, 1_100)), "a weapon not recognised yet");
    }

    #[test]
    fn a_shot_is_timed_at_its_fire_key() {
        let mut p = LoadingPipeline::new();
        // Pressed while loading (nothing fires), then pressed with the gun loaded.
        p.fire_key(10_000);
        p.fire_key(11_000);
        // Last lit read at 10 950, the menu seen at 11 080.
        assert_eq!(p.timed_by_key((10_950, 11_080), 11_080), Some((11_000, 11_000)));
        assert!(p.fire_keys.is_empty());
    }

    #[test]
    fn a_shot_without_a_recent_key_is_timed_by_the_screen() {
        let mut p = LoadingPipeline::new();
        p.fire_key(5_000);
        assert_eq!(p.timed_by_key((7_000, 7_200), 7_200), Some((7_000, 7_200)));
        // No key at all (input unavailable).
        assert_eq!(p.timed_by_key((9_000, 9_200), 9_200), Some((9_000, 9_200)));
    }

    #[test]
    fn with_keys_coming_through_a_screen_change_without_f_is_no_shot() {
        let mut p = LoadingPipeline::new();
        p.keys_live();
        // A seat change: the menu pops with no F before it.
        assert_eq!(p.timed_by_key((20_000, 20_200), 20_200), None);
        p.fire_key(30_000);
        assert_eq!(p.timed_by_key((29_900, 30_100), 30_100), Some((30_000, 30_000)));
    }

    #[test]
    fn a_slot_lit_after_the_key_still_takes_the_key() {
        let mut p = LoadingPipeline::new();
        p.keys_live();
        // Centurion AVRE: the slot read lit until 850 ms after F, the menu at 900 ms.
        p.fire_key(40_000);
        assert_eq!(p.timed_by_key((40_850, 40_900), 40_900), Some((40_000, 40_000)));
        // F pressed three times before the slot went dark: the last of them.
        for k in [50_000, 50_300, 50_600] {
            p.fire_key(k);
        }
        assert_eq!(p.timed_by_key((51_000, 51_100), 51_100), Some((50_600, 50_600)));
        // A press older than `FIRE_CONFIRM_MS` is no shot.
        p.fire_key(60_000);
        assert_eq!(p.timed_by_key((61_600, 61_700), 61_700), None);
    }

    #[test]
    fn a_press_on_the_full_map_waits_for_v3() {
        let mut p = LoadingPipeline::new();
        p.keys_live();
        // Centurion AVRE, from a recorded match: F on V1, the map closed 1.6 s later, the loading
        // menu 190 ms after that.
        p.left_v3(100_000);
        p.fire_key(101_000);
        p.back_on_v3(102_600);
        assert_eq!(p.timed_by_key((102_790, 102_790), 102_790), Some((101_000, 101_000)));
        // The seat changed on V1 and the map closed 5 s after F: still the press.
        p.left_v3(110_000);
        p.fire_key(111_000);
        p.back_on_v3(116_000);
        assert_eq!(p.timed_by_key((116_050, 116_050), 116_050), Some((111_000, 111_000)));
        // Back on V3 more than `FIRE_CONFIRM_MS` before the menu: no shot by the screen.
        p.left_v3(120_000);
        p.fire_key(121_000);
        p.back_on_v3(122_000);
        assert_eq!(p.timed_by_key((123_600, 123_600), 123_600), None);
        // Left on V1 longer than `AWAY_PENDING_MAX_MS`: no longer the screen's to confirm.
        p.left_v3(130_000);
        p.fire_key(131_000);
        assert_eq!(p.timed_by_key((152_000, 152_000), 152_000), None);
    }
}
