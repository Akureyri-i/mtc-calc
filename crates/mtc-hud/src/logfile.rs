//! The HUD's log. Every message (the `log` macros) goes, with the seconds since start, to a file
//! per run in `%LOCALAPPDATA%\MTC Calculator\logs` (`MTC_LOG_DIR` when set), and the last minutes
//! stay in memory for a problem report (`recent`). Development builds, which have a console, print
//! the messages there too (warnings on standard error); replays only print them. Old files are
//! removed at start: at most `KEEP_FILES`, `KEEP_BYTES` in all, none older than `KEEP_DAYS`.
use std::collections::VecDeque;
use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime};

use log::{Level, LevelFilter, Log, Metadata, Record};

const KEEP_FILES: usize = 10;
const KEEP_BYTES: u64 = 50 << 20;
const KEEP_DAYS: u64 = 30;
/// Lines kept in memory for a problem report, and at most this many bytes of them.
pub const RECENT: Duration = Duration::from_secs(600);
const RECENT_MAX_BYTES: usize = 4 << 20;

pub struct LogFile {
    path: PathBuf,
    recent: Mutex<VecDeque<(Instant, String)>>,
    recent_bytes: Mutex<usize>,
}

static LOG: OnceLock<Arc<LogFile>> = OnceLock::new();

/// Where the log files go.
pub fn dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("MTC_LOG_DIR") {
        return Some(PathBuf::from(dir));
    }
    Some(PathBuf::from(std::env::var_os("LOCALAPPDATA")?).join("MTC Calculator").join("logs"))
}

/// Starts the log (once): the file, and the messages printed as well when `console`. `None` where
/// the file cannot be made (the messages are then only printed, when `console`).
pub fn start(started: Instant, console: bool) -> Option<Arc<LogFile>> {
    if let Some(log) = LOG.get() {
        return Some(log.clone());
    }
    let opened = dir().and_then(|dir| {
        std::fs::create_dir_all(&dir).ok()?;
        clean(&dir);
        let path = dir.join(format!("hud-{}.log", day_time_stamp()));
        let file = File::create(&path).ok()?;
        Some((Arc::new(LogFile { path, recent: Mutex::new(VecDeque::new()), recent_bytes: Mutex::new(0) }), Mutex::new(file)))
    });
    let log = opened.as_ref().map(|(log, _)| log.clone());
    install(Logger { file: opened, started, console });
    if let Some(log) = &log {
        LOG.set(log.clone()).ok();
    }
    log
}

/// The messages printed only (replays and tools).
pub fn console_only() {
    install(Logger { file: None, started: Instant::now(), console: true });
}

fn install(logger: Logger) {
    if log::set_boxed_logger(Box::new(logger)).is_ok() {
        log::set_max_level(LevelFilter::Info);
    }
}

/// The log, when it was started.
pub fn get() -> Option<Arc<LogFile>> {
    LOG.get().cloned()
}

struct Logger {
    file: Option<(Arc<LogFile>, Mutex<File>)>,
    started: Instant,
    console: bool,
}

impl Log for Logger {
    fn enabled(&self, metadata: &Metadata) -> bool {
        // The calculator's own messages; of the libraries it uses (network, TLS), warnings only.
        metadata.level() <= if metadata.target().starts_with("mtc_") { Level::Info } else { Level::Warn }
    }

    fn log(&self, record: &Record) {
        if !self.enabled(record.metadata()) {
            return;
        }
        let text = record.args().to_string();
        if self.console {
            if record.level() <= Level::Warn {
                eprintln!("{text}");
            } else {
                println!("{text}");
            }
        }
        let Some((log, file)) = &self.file else { return };
        let mark = match record.level() {
            Level::Error => "[error] ",
            Level::Warn => "[warning] ",
            _ => "",
        };
        let secs = self.started.elapsed().as_secs_f64();
        let Ok(mut file) = file.lock() else { return };
        for line in text.split('\n') {
            let stamped = format!("[{secs:8.1}] {mark}{}", line.trim_end_matches('\r'));
            let _ = writeln!(file, "{stamped}");
            log.keep(stamped);
        }
    }

    fn flush(&self) {}
}

impl LogFile {
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The lines of the last `within`, each with its seconds since start.
    pub fn recent(&self, within: Duration) -> Vec<String> {
        let now = Instant::now();
        let recent = self.recent.lock().expect("log lines");
        recent.iter().filter(|(at, _)| now.saturating_duration_since(*at) <= within).map(|(_, line)| line.clone()).collect()
    }

    fn keep(&self, line: String) {
        let now = Instant::now();
        let mut recent = self.recent.lock().expect("log lines");
        let mut bytes = self.recent_bytes.lock().expect("log bytes");
        *bytes += line.len();
        recent.push_back((now, line));
        while let Some((at, front)) = recent.front() {
            if now.saturating_duration_since(*at) <= RECENT && *bytes <= RECENT_MAX_BYTES {
                break;
            }
            *bytes -= front.len();
            recent.pop_front();
        }
    }
}

/// Removes old log files: beyond the newest `KEEP_FILES`, beyond `KEEP_BYTES` in all, older than
/// `KEEP_DAYS`.
fn clean(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    let mut files: Vec<(SystemTime, u64, PathBuf)> = entries
        .filter_map(Result::ok)
        .filter(|e| e.path().extension().is_some_and(|x| x == "log"))
        .filter_map(|e| {
            let m = e.metadata().ok()?;
            Some((m.modified().ok()?, m.len(), e.path()))
        })
        .collect();
    files.sort_by(|a, b| b.0.cmp(&a.0));
    let now = SystemTime::now();
    let mut total = 0;
    for (i, (modified, len, path)) in files.into_iter().enumerate() {
        total += len;
        let old = now.duration_since(modified).is_ok_and(|age| age > Duration::from_secs(KEEP_DAYS * 86_400));
        // The newest run before this one always stays (the one a report is often about).
        if i >= 1 && (i >= KEEP_FILES - 1 || total > KEEP_BYTES || old) {
            let _ = std::fs::remove_file(path);
        }
    }
}

/// `YYYYMMDD-HHMMSS` in UTC, for file names.
pub fn day_time_stamp() -> String {
    let secs = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let (y, m, d) = civil_from_days((secs / 86_400) as i64);
    let s = secs % 86_400;
    format!("{y:04}{m:02}{d:02}-{:02}{:02}{:02}", s / 3600, s / 60 % 60, s % 60)
}

/// The UTC day `YYYY-MM-DD`, the only time the diagnostic records carry.
pub fn utc_day() -> String {
    let secs = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let (y, m, d) = civil_from_days((secs / 86_400) as i64);
    format!("{y:04}-{m:02}-{d:02}")
}

/// Days since 1970-01-01 to (year, month, day) (Howard Hinnant's algorithm).
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

#[cfg(test)]
mod tests {
    use super::civil_from_days;

    #[test]
    fn days_to_dates() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(20_783), (2026, 11, 26));
        assert_eq!(civil_from_days(11_016), (2000, 2, 29));
    }
}
