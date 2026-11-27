//! Diagnostic data, in two tiers.
//!
//! Statistics, sent only when the player has agreed: a line per shot and one record per run.
//! They hold plain numbers only: no screenshots, no player names, no key presses, no file paths,
//! and the day is the only time. Each kind of record carries its own random id made at start, so
//! the two kinds cannot be joined and no run can be joined to another. The records wait in a file
//! (`pending.jsonl`) that a sender thread posts every `SEND_EVERY` and at exit, and that "View
//! data" opens; nothing is written while the player has not agreed.
//!
//! A problem report is made by the player on request and takes this run's records whatever the
//! sharing setting (`Diag::run_lines`).
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::Serialize;

/// Bumped when what is collected grows, so the startup question comes back for players who
/// agreed to an earlier version.
pub const RECORD_VERSION: u32 = 1;
/// Where the statistics go: the receiver on our own domain (`tools/diag/worker.js`; workers.dev
/// is blocked in mainland China). `MTC_DIAG_URL` overrides it, for a local test receiver.
const ENDPOINT: Option<&str> = Some("https://stats.mtc-calc.dev/");
pub const SEND_EVERY: Duration = Duration::from_secs(600);
/// The waiting file is kept under this size (the oldest lines go).
const PENDING_MAX_BYTES: u64 = 1 << 20;

/// The folder of the waiting statistics and of problem reports.
pub fn dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("MTC_DIAG_DIR") {
        return Some(PathBuf::from(dir));
    }
    Some(PathBuf::from(std::env::var_os("LOCALAPPDATA")?).join("MTC Calculator").join("diagnostics"))
}

/// A random id for one kind of record in one run.
fn random_id() -> String {
    use std::hash::{BuildHasher, Hasher};
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_u128(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos()));
    h.write_u32(std::process::id());
    format!("{:016x}", h.finish())
}

/// One shot, written when it is settled: its X seen and faded, or given up on.
#[derive(Clone, Debug, Serialize)]
pub struct ShotRecord {
    pub kind: &'static str,
    pub v: u32,
    pub run: String,
    pub app: &'static str,
    pub day: String,
    pub map: String,
    pub weapon: Option<String>,
    pub shell: Option<String>,
    pub charge: Option<String>,
    /// The launch point used: measured for this weapon, or borrowed from the named one.
    pub launch: Option<String>,
    pub elevation: Option<f64>,
    pub azimuth: Option<f64>,
    /// The readout was the one the shot was fired at.
    pub sure: bool,
    /// The gun (normalised map coordinates), the view its arrow was read on and its pixels a metre.
    pub gun: Option<[f64; 2]>,
    pub gun_view: Option<&'static str>,
    pub gun_px_per_m: Option<f64>,
    pub predicted: Option<[f64; 2]>,
    pub predicted_m: Option<f64>,
    pub predicted_s: Option<f64>,
    pub x: Option<[f64; 2]>,
    pub x_px_per_m: Option<f64>,
    /// Along and across the line of fire (m, beyond and right positive), landed late (s).
    pub along_m: Option<f64>,
    pub across_m: Option<f64>,
    pub late_s: Option<f64>,
    /// "screen": confirmed on screen; "backfill": a press the screen did not confirm, joined to
    /// its X afterwards.
    pub recorded: &'static str,
    /// "x": its X seen; "no_x"; "no_x_under_icon": predicted under an objective icon; "no_prediction".
    pub outcome: &'static str,
    /// The shot's number in this run.
    pub n: u32,
}

/// What a 5 s report measured (`stats::Stats::maybe_report`).
#[derive(Clone, Debug, Default)]
pub struct Report {
    pub cpu_cores: f64,
    pub working_set_mb: f64,
    pub heap_mb: f64,
    pub game_frame_ms: f64,
    pub latency_p50_ms: f64,
    pub read: u64,
    pub unreadable: u64,
    pub absent: u64,
    /// Each part of the frame callback (cores).
    pub parts: Vec<(&'static str, f64)>,
}

/// One display configuration the run was in.
#[derive(Clone, Debug, Default, Serialize)]
pub struct ConfigRecord {
    pub client: [u32; 2],
    pub scaling_pct: u32,
    pub monitor: Option<[i32; 2]>,
    pub minutes: f64,
    /// The minimap's and the full map's containers found on screen less the rules' (left, top,
    /// side; px).
    pub minimap_found: Option<[f64; 3]>,
    pub full_map_found: Option<[f64; 3]>,
    /// The countdown bar found on screen (reference geometry: x0, y0, x1, y1).
    pub countdown: Option<[i32; 4]>,
}

/// Counted over the run.
#[derive(Clone, Debug, Default)]
struct Counts {
    reports: Vec<Report>,
    configs: Vec<ConfigRecord>,
    current_config: Option<usize>,
    config_since: Option<Instant>,
    weapons: BTreeMap<String, u32>,
    unknown_weapon_labels: BTreeMap<String, u32>,
    maps: BTreeMap<String, String>,
    loads: u32,
    shots: BTreeMap<&'static str, u32>,
    seat_reads: u32,
    seat_own_found: u32,
}

/// The run's record, built when it is sent or viewed.
#[derive(Clone, Debug, Serialize)]
pub struct RunRecord {
    pub kind: &'static str,
    pub v: u32,
    pub run: String,
    pub app: &'static str,
    pub day: String,
    pub windows: Option<String>,
    pub gpu: Option<String>,
    pub cpu_threads: Option<usize>,
    pub minutes: f64,
    pub configs: Vec<ConfigRecord>,
    /// The process's CPU (cores): mean and 95th percentile of the 5 s reports, and each part's mean.
    pub cpu_mean: Option<f64>,
    pub cpu_p95: Option<f64>,
    pub cpu_parts: BTreeMap<&'static str, f64>,
    pub working_set_max_mb: Option<f64>,
    pub heap_max_mb: Option<f64>,
    pub game_frame_ms_median: Option<f64>,
    pub latency_ms_median: Option<f64>,
    /// Frames with the angle readout read, unreadable, absent.
    pub readout: [u64; 3],
    /// Weapons recognised (data names) and labels not recognised (the game's text) with counts.
    pub weapons: BTreeMap<String, u32>,
    pub unknown_weapon_labels: BTreeMap<String, u32>,
    /// Maps recognised and how.
    pub maps: BTreeMap<String, String>,
    pub loads: u32,
    /// Shots by how they settled (`ShotRecord::outcome`, `recorded`).
    pub shots: BTreeMap<&'static str, u32>,
    pub seat_reads: u32,
    pub seat_own_found: u32,
    pub settings: BTreeMap<&'static str, String>,
    pub panics: u32,
}

static PANICS: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

/// What the pipelines saw that the run record counts, noted where it happens and taken by the
/// frame loop (`Diag::take_events`).
#[derive(Clone, Debug)]
pub enum Event {
    Weapon(String),
    /// A weapon label read in full that no weapon in the data matches (the game's text).
    UnknownLabel(String),
    /// A map recognised, and how.
    Map(String, &'static str),
    Load,
    /// A seat menu read, and whether the player's own row was found in it.
    SeatRead(bool),
    /// A map container found on screen (the full map's or the minimap's) less the rule's.
    Container(bool, [f64; 3]),
    Countdown((i32, i32, i32, i32)),
}

static EVENTS: Mutex<Vec<Event>> = Mutex::new(Vec::new());

/// Notes an event for the run record (dropped past 10 000 untaken, as in a replay).
pub fn note(event: Event) {
    if let Ok(mut events) = EVENTS.lock()
        && events.len() < 10_000
    {
        events.push(event);
    }
}

/// Counts panics for the run record (the message still goes to the log).
pub fn count_panics() {
    // Into the log (and, in development builds, onto the console through it).
    std::panic::set_hook(Box::new(|info| {
        PANICS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        log::error!("thread '{}' {info}", std::thread::current().name().unwrap_or("unnamed"));
    }));
}

pub struct Diag {
    started: Instant,
    shot_id: String,
    run_id: String,
    day: String,
    /// The player agreed to send them.
    sharing: bool,
    shots: Vec<ShotRecord>,
    counts: Counts,
    pending: Option<PathBuf>,
    /// The waiting file is written by the frame loop and emptied by the sender.
    file_lock: Arc<Mutex<()>>,
    last_send: Instant,
    sending: Option<std::thread::JoinHandle<()>>,
    env: (Option<String>, Option<String>, Option<usize>),
}

impl Diag {
    pub fn new(sharing: bool) -> Self {
        let pending = dir().map(|d| d.join("pending.jsonl"));
        Self {
            started: Instant::now(),
            shot_id: random_id(),
            run_id: random_id(),
            day: crate::logfile::utc_day(),
            sharing,
            shots: Vec::new(),
            counts: Counts::default(),
            pending,
            file_lock: Arc::new(Mutex::new(())),
            last_send: Instant::now(),
            sending: None,
            env: (windows_version(), gpu_class(), std::thread::available_parallelism().ok().map(|n| n.get())),
        }
    }

    /// The player turned sharing on or off; off empties the waiting file.
    pub fn set_sharing(&mut self, on: bool) {
        self.sharing = on;
        if !on && let Some(path) = &self.pending {
            let _guard = self.file_lock.lock();
            let _ = std::fs::remove_file(path);
        }
    }

    pub fn day(&self) -> &str {
        &self.day
    }

    /// A settled shot (its run id and day filled in here).
    pub fn add_shot(&mut self, mut record: ShotRecord) {
        record.run.clone_from(&self.shot_id);
        record.day.clone_from(&self.day);
        *self.counts.shots.entry(record.outcome).or_default() += 1;
        *self.counts.shots.entry(record.recorded).or_default() += 1;
        if self.sharing {
            self.append(&[serde_json::to_string(&record).expect("shot record")]);
        }
        self.shots.push(record);
    }

    pub fn add_report(&mut self, report: Report) {
        self.counts.reports.push(report);
    }

    /// The events noted since the last call.
    pub fn take_events(&mut self) {
        let events = EVENTS.lock().map(|mut e| std::mem::take(&mut *e)).unwrap_or_default();
        for event in events {
            match event {
                Event::Weapon(name) => self.weapon(&name),
                Event::UnknownLabel(text) => self.unknown_weapon_label(&text),
                Event::Map(id, how) => self.map(&id, how),
                Event::Load => self.load(),
                Event::SeatRead(own) => self.seat_read(own),
                Event::Container(full, diff) => self.container_found(full, diff),
                Event::Countdown(interior) => self.countdown_found(interior),
            }
        }
    }

    /// The display configuration on screen now (client size, DPI, monitor size).
    pub fn config(&mut self, client: (u32, u32), dpi: u32, monitor: Option<(i32, i32)>) {
        let key = ([client.0, client.1], dpi * 100 / 96);
        let now = Instant::now();
        let index = match self.counts.configs.iter().position(|c| (c.client, c.scaling_pct) == key) {
            Some(i) => i,
            None => {
                self.counts.configs.push(ConfigRecord { client: key.0, scaling_pct: key.1, monitor: monitor.map(|m| [m.0, m.1]), ..Default::default() });
                self.counts.configs.len() - 1
            }
        };
        if self.counts.current_config != Some(index) {
            self.close_config(now);
            self.counts.current_config = Some(index);
            self.counts.config_since = Some(now);
        }
    }

    fn close_config(&mut self, now: Instant) {
        if let (Some(i), Some(since)) = (self.counts.current_config, self.counts.config_since) {
            self.counts.configs[i].minutes += now.saturating_duration_since(since).as_secs_f64() / 60.0;
            self.counts.config_since = Some(now);
        }
    }

    /// A map container found on screen for the configuration on screen (less the rule's).
    pub fn container_found(&mut self, full_map: bool, diff: [f64; 3]) {
        if let Some(i) = self.counts.current_config {
            let c = &mut self.counts.configs[i];
            if full_map { c.full_map_found = Some(diff) } else { c.minimap_found = Some(diff) }
        }
    }

    pub fn countdown_found(&mut self, interior: (i32, i32, i32, i32)) {
        if let Some(i) = self.counts.current_config {
            self.counts.configs[i].countdown = Some([interior.0, interior.1, interior.2, interior.3]);
        }
    }

    pub fn weapon(&mut self, name: &str) {
        *self.counts.weapons.entry(name.to_owned()).or_default() += 1;
    }

    pub fn unknown_weapon_label(&mut self, text: &str) {
        let n = self.counts.unknown_weapon_labels.len();
        if n < 20 || self.counts.unknown_weapon_labels.contains_key(text) {
            *self.counts.unknown_weapon_labels.entry(text.to_owned()).or_default() += 1;
        }
    }

    pub fn map(&mut self, id: &str, how: &str) {
        self.counts.maps.insert(id.to_owned(), how.to_owned());
    }

    pub fn load(&mut self) {
        self.counts.loads += 1;
    }

    pub fn seat_read(&mut self, own_found: bool) {
        self.counts.seat_reads += 1;
        self.counts.seat_own_found += u32::from(own_found);
    }

    /// The run's record now.
    pub fn run_record(&mut self, settings: BTreeMap<&'static str, String>) -> RunRecord {
        self.close_config(Instant::now());
        let reports = &self.counts.reports;
        let mut cpus: Vec<f64> = reports.iter().map(|r| r.cpu_cores).collect();
        cpus.sort_by(f64::total_cmp);
        let median = |mut v: Vec<f64>| -> Option<f64> {
            v.retain(|x| x.is_finite());
            v.sort_by(f64::total_cmp);
            v.get(v.len() / 2).copied()
        };
        let mut parts: BTreeMap<&'static str, f64> = BTreeMap::new();
        for r in reports {
            for &(name, cores) in &r.parts {
                *parts.entry(name).or_default() += cores / reports.len() as f64;
            }
        }
        let round = |x: f64| (x * 10_000.0).round() / 10_000.0;
        RunRecord {
            kind: "run",
            v: RECORD_VERSION,
            run: self.run_id.clone(),
            app: env!("CARGO_PKG_VERSION"),
            day: self.day.clone(),
            windows: self.env.0.clone(),
            gpu: self.env.1.clone(),
            cpu_threads: self.env.2,
            minutes: (self.started.elapsed().as_secs_f64() / 60.0 * 10.0).round() / 10.0,
            configs: self.counts.configs.iter().map(|c| ConfigRecord { minutes: (c.minutes * 10.0).round() / 10.0, ..c.clone() }).collect(),
            cpu_mean: (!cpus.is_empty()).then(|| round(cpus.iter().sum::<f64>() / cpus.len() as f64)),
            cpu_p95: cpus.get(((cpus.len().max(1) - 1) as f64 * 0.95) as usize).copied().map(round),
            cpu_parts: parts.into_iter().map(|(k, v)| (k, round(v))).collect(),
            working_set_max_mb: reports.iter().map(|r| r.working_set_mb).reduce(f64::max).map(f64::round),
            heap_max_mb: reports.iter().map(|r| r.heap_mb).reduce(f64::max).map(f64::round),
            game_frame_ms_median: median(reports.iter().map(|r| r.game_frame_ms).collect()).map(|x| (x * 10.0).round() / 10.0),
            latency_ms_median: median(reports.iter().map(|r| r.latency_p50_ms).collect()).map(|x| (x * 10.0).round() / 10.0),
            readout: [reports.iter().map(|r| r.read).sum(), reports.iter().map(|r| r.unreadable).sum(), reports.iter().map(|r| r.absent).sum()],
            weapons: self.counts.weapons.clone(),
            unknown_weapon_labels: self.counts.unknown_weapon_labels.clone(),
            maps: self.counts.maps.clone(),
            loads: self.counts.loads,
            shots: self.counts.shots.clone(),
            seat_reads: self.counts.seat_reads,
            seat_own_found: self.counts.seat_own_found,
            settings,
            panics: PANICS.load(std::sync::atomic::Ordering::Relaxed),
        }
    }

    /// This run's records as lines (the problem report).
    pub fn run_lines(&mut self, settings: BTreeMap<&'static str, String>) -> Vec<String> {
        let mut lines: Vec<String> = self.shots.iter().map(|s| serde_json::to_string(s).expect("shot record")).collect();
        lines.push(serde_json::to_string(&self.run_record(settings)).expect("run record"));
        lines
    }

    fn append(&self, lines: &[String]) {
        let Some(path) = &self.pending else { return };
        let _guard = self.file_lock.lock();
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        append_capped(path, lines);
    }

    /// Writes what would be sent now into a text file and returns it ("View data").
    pub fn view_file(&mut self, settings: BTreeMap<&'static str, String>) -> Option<PathBuf> {
        let dir = dir()?;
        std::fs::create_dir_all(&dir).ok()?;
        let waiting = self.pending.as_ref().and_then(|p| std::fs::read_to_string(p).ok()).unwrap_or_default();
        let run = serde_json::to_string_pretty(&self.run_record(settings)).ok()?;
        let state = if self.sharing { "Sharing is on: these are sent every 10 minutes and when the calculator closes." } else { "Sharing is off: nothing is written or sent. This is what would be sent." };
        let shots = if waiting.is_empty() { "(none waiting)".to_owned() } else { waiting };
        let text = format!("{state}\nOne line per shot, then this run's record. Plain numbers only: no screenshots, no player names, no key presses, no file paths; the day is the only time.\n\n== Shots waiting ==\n{shots}\n\n== This run ==\n{run}\n");
        let path = dir.join("to-send.txt");
        std::fs::write(&path, text).ok()?;
        Some(path)
    }

    /// Every `SEND_EVERY`, and at exit (`last`): this run's record and the waiting shots posted in
    /// the background; what fails to go stays for the next time.
    pub fn maybe_send(&mut self, settings: impl FnOnce() -> BTreeMap<&'static str, String>, last: bool) {
        if !self.sharing || (!last && self.last_send.elapsed() < SEND_EVERY) {
            return;
        }
        self.last_send = Instant::now();
        let Some(url) = std::env::var("MTC_DIAG_URL").ok().or_else(|| ENDPOINT.map(str::to_owned)) else { return };
        let Some(path) = self.pending.clone() else { return };
        if self.sending.as_ref().is_some_and(|t| !t.is_finished()) {
            return;
        }
        let run = serde_json::to_string(&self.run_record(settings())).expect("run record");
        let waiting = {
            let _guard = self.file_lock.lock();
            let text = std::fs::read_to_string(&path).unwrap_or_default();
            let _ = std::fs::remove_file(&path);
            text
        };
        let lock = self.file_lock.clone();
        let handle = std::thread::Builder::new()
            .name("diag-send".to_owned())
            .spawn(move || {
                let body = format!("{waiting}{run}\n");
                let agent = ureq::AgentBuilder::new().timeout_connect(Duration::from_secs(10)).timeout_read(Duration::from_secs(20)).build();
                match agent.post(&url).set("Content-Type", "application/x-ndjson").send_string(&body) {
                    Ok(_) => log::info!("Statistics: sent {} lines", body.lines().count()),
                    Err(error) => {
                        log::info!("Statistics: not sent ({error}), trying again later");
                        let lines: Vec<String> = waiting.lines().map(str::to_owned).collect();
                        let _guard = lock.lock();
                        append_capped(&path, &lines);
                    }
                }
            })
            .ok();
        self.sending = handle;
        if last && let Some(t) = self.sending.take() {
            // At exit: a few seconds for the last post.
            let deadline = Instant::now() + Duration::from_secs(5);
            while !t.is_finished() && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(50));
            }
        }
    }
}

/// Appends lines to the waiting file, dropping its oldest half when it grows past the cap.
fn append_capped(path: &Path, lines: &[String]) {
    use std::io::Write;
    if lines.is_empty() {
        return;
    }
    if std::fs::metadata(path).is_ok_and(|m| m.len() > PENDING_MAX_BYTES)
        && let Ok(text) = std::fs::read_to_string(path)
    {
        let kept: Vec<&str> = text.lines().collect();
        let _ = std::fs::write(path, kept[kept.len() / 2..].join("\n") + "\n");
    }
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
        for line in lines {
            let _ = writeln!(f, "{line}");
        }
    }
}

#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "system" {
    fn RtlGetVersion(info: *mut OsVersionInfo) -> i32;
}

#[repr(C)]
struct OsVersionInfo {
    size: u32,
    major: u32,
    minor: u32,
    build: u32,
    platform: u32,
    csd: [u16; 128],
}

/// "Windows 11 (26200)": the major version and build.
pub fn windows_version() -> Option<String> {
    let mut info = OsVersionInfo { size: size_of::<OsVersionInfo>() as u32, major: 0, minor: 0, build: 0, platform: 0, csd: [0; 128] };
    if unsafe { RtlGetVersion(&mut info) } != 0 {
        return None;
    }
    let name = if info.major == 10 && info.build >= 22_000 { "Windows 11" } else if info.major == 10 { "Windows 10" } else { "Windows" };
    Some(format!("{name} ({})", info.build))
}

/// The graphics adapters by kind only ("NVIDIA dedicated + Intel integrated"): no model, so the
/// hardware does not single anyone out.
fn gpu_class() -> Option<String> {
    use windows::Win32::Graphics::Dxgi::{CreateDXGIFactory1, DXGI_ADAPTER_FLAG_SOFTWARE, IDXGIFactory1};
    let factory: IDXGIFactory1 = unsafe { CreateDXGIFactory1() }.ok()?;
    let mut kinds = Vec::new();
    for i in 0.. {
        let Ok(adapter) = (unsafe { factory.EnumAdapters1(i) }) else { break };
        let Ok(desc) = (unsafe { adapter.GetDesc1() }) else { continue };
        if desc.Flags & DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32 != 0 {
            continue;
        }
        let vendor = match desc.VendorId {
            0x10DE => "NVIDIA",
            0x1002 => "AMD",
            0x8086 => "Intel",
            _ => "other",
        };
        let kind = if desc.DedicatedVideoMemory >= 1 << 30 { "dedicated" } else { "integrated" };
        kinds.push(format!("{vendor} {kind}"));
    }
    (!kinds.is_empty()).then(|| kinds.join(" + "))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_waiting_file_keeps_its_newest_half_past_the_cap() {
        let dir = std::env::temp_dir().join(format!("mtc-diag-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("pending.jsonl");
        let _ = std::fs::remove_file(&path);
        let line = "x".repeat(1000);
        for _ in 0..1100 {
            append_capped(&path, std::slice::from_ref(&line));
        }
        let len = std::fs::metadata(&path).unwrap().len();
        assert!(len <= PENDING_MAX_BYTES + 2_000, "{len}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// With sharing on, a settled shot waits in the file, and the send posts it with the run's
    /// record to the receiver and empties the file; "View data" shows the same.
    #[test]
    fn shots_wait_and_are_posted_with_the_run_record() {
        use std::io::{Read, Write};
        let dir = std::env::temp_dir().join(format!("mtc-diag-send-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        // The test receiver: one request, its body kept (sized or chunked).
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
            let mut data = Vec::new();
            let mut buf = [0u8; 4096];
            let body = loop {
                let n = stream.read(&mut buf).unwrap_or(0);
                data.extend_from_slice(&buf[..n]);
                let text = String::from_utf8_lossy(&data).to_string();
                let done = text.find("\r\n\r\n").and_then(|head| {
                    let headers = text[..head].to_ascii_lowercase();
                    let rest = &text[head + 4..];
                    match headers.lines().find_map(|l| l.strip_prefix("content-length:").map(|v| v.trim().parse::<usize>().unwrap())) {
                        Some(length) => (rest.len() >= length).then(|| rest.to_owned()),
                        None => rest.contains("\r\n0\r\n\r\n").then(|| rest.to_owned()),
                    }
                });
                if let Some(body) = done {
                    break body;
                }
                if n == 0 {
                    break text;
                }
            };
            let _ = stream.write_all(b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
            body
        });
        // SAFETY: only this test reads these variables.
        unsafe {
            std::env::set_var("MTC_DIAG_DIR", &dir);
            std::env::set_var("MTC_DIAG_URL", format!("http://127.0.0.1:{port}/"));
        }
        let mut d = Diag::new(true);
        d.add_shot(ShotRecord {
            kind: "shot", v: RECORD_VERSION, run: String::new(), app: "test", day: String::new(), map: "rohkshort".into(),
            weapon: Some("155mm M284".into()), shell: Some("M107 Low Charge".into()), charge: Some("Low Charge".into()), launch: Some("measured".into()),
            elevation: Some(14.96), azimuth: Some(187.49), sure: true, gun: Some([0.7965, 0.3184]), gun_view: Some("V3"), gun_px_per_m: Some(0.077),
            predicted: Some([0.742, 0.742]), predicted_m: Some(1415.4), predicted_s: Some(8.7), x: Some([0.742, 0.742]), x_px_per_m: Some(0.077),
            along_m: Some(1.9), across_m: Some(-7.1), late_s: Some(0.1), recorded: "screen", outcome: "x", n: 1,
        });
        let pending = dir.join("pending.jsonl");
        assert!(std::fs::read_to_string(&pending).unwrap().contains("\"along_m\":1.9"));
        let view = std::fs::read_to_string(d.view_file(BTreeMap::new()).unwrap()).unwrap();
        assert!(view.contains("Sharing is on") && view.contains("\"along_m\":1.9") && view.contains("\"kind\": \"run\""), "{view}");
        d.maybe_send(BTreeMap::new, true);
        let body = server.join().unwrap();
        assert!(body.contains("\"kind\":\"shot\"") && body.contains("\"kind\":\"run\""), "{body}");
        assert!(!body.contains("Users"), "{body}");
        assert!(!pending.exists());
        unsafe {
            std::env::remove_var("MTC_DIAG_DIR");
            std::env::remove_var("MTC_DIAG_URL");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_run_record_carries_the_day_and_the_configuration() {
        let mut d = Diag::new(false);
        d.config((1920, 1080), 96, Some((2560, 1440)));
        note(Event::Container(false, [0.1, -0.4, 0.1]));
        d.take_events();
        let record = d.run_record(BTreeMap::new());
        assert_eq!(record.day.len(), 10);
        assert_eq!(record.configs[0].client, [1920, 1080]);
        assert_eq!(record.configs[0].scaling_pct, 100);
        assert_eq!(record.configs[0].minimap_found, Some([0.1, -0.4, 0.1]));
    }
}
