//! The HUD's log kept as files, for debugging: everything printed to
//! stdout and stderr also goes, with the seconds since start, to a file per run in
//! `%LOCALAPPDATA%\MTC Calculator\logs` (`MTC_LOG_DIR` when set), and the last minutes stay in
//! memory for a problem report (`recent`). The console (or a redirect) still gets the output as
//! before. Old files are removed at start: at most `KEEP_FILES`, `KEEP_BYTES` in all, none older
//! than `KEEP_DAYS`.
use std::collections::VecDeque;
use std::fs::File;
use std::io::{BufRead, BufReader, Write};
use std::mem::ManuallyDrop;
use std::os::windows::io::{FromRawHandle, RawHandle};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime};

use windows::Win32::Foundation::HANDLE;
use windows::Win32::System::Console::{GetStdHandle, STD_ERROR_HANDLE, STD_HANDLE, STD_OUTPUT_HANDLE, SetStdHandle};
use windows::Win32::System::Pipes::CreatePipe;

const KEEP_FILES: usize = 10;
const KEEP_BYTES: u64 = 50 << 20;
const KEEP_DAYS: u64 = 30;
/// Lines kept in memory for a problem report, and at most this many bytes of them.
pub const RECENT: Duration = Duration::from_secs(600);
const RECENT_MAX_BYTES: usize = 4 << 20;

pub struct LogFile {
    path: PathBuf,
    started: Instant,
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

/// Starts writing the log files (once); `None` where the folder or the pipes cannot be made.
pub fn start(started: Instant) -> Option<Arc<LogFile>> {
    if let Some(log) = LOG.get() {
        return Some(log.clone());
    }
    let dir = dir()?;
    std::fs::create_dir_all(&dir).ok()?;
    clean(&dir);
    let stamp = day_time_stamp();
    let path = dir.join(format!("hud-{stamp}.log"));
    let file = Arc::new(Mutex::new(File::create(&path).ok()?));
    let log = Arc::new(LogFile { path, started, recent: Mutex::new(VecDeque::new()), recent_bytes: Mutex::new(0) });
    tee(STD_OUTPUT_HANDLE, "", file.clone(), log.clone())?;
    tee(STD_ERROR_HANDLE, "[stderr] ", file, log.clone())?;
    LOG.set(log.clone()).ok();
    Some(log)
}

/// The log, when it was started.
pub fn get() -> Option<Arc<LogFile>> {
    LOG.get().cloned()
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

/// The standard handle `which` redirected into a pipe whose lines a thread copies to where the
/// handle went before, to the file (with the seconds since start) and to the recent lines.
fn tee(which: STD_HANDLE, mark: &'static str, file: Arc<Mutex<File>>, log: Arc<LogFile>) -> Option<()> {
    let before = unsafe { GetStdHandle(which) }.ok().filter(|h| !h.is_invalid() && !h.0.is_null());
    let (mut read, mut write) = (HANDLE::default(), HANDLE::default());
    unsafe { CreatePipe(&mut read, &mut write, None, 0) }.ok()?;
    unsafe { SetStdHandle(which, write) }.ok()?;
    let read = unsafe { File::from_raw_handle(read.0 as RawHandle) };
    // Where the output went before (a console or a redirect), never closed here.
    let before = before.map(|h| ManuallyDrop::new(unsafe { File::from_raw_handle(h.0 as RawHandle) }));
    std::thread::Builder::new()
        .name(format!("log{}", if mark.is_empty() { "" } else { "-err" }))
        .spawn(move || {
            let mut before = before;
            let mut lines = BufReader::new(read);
            let mut line = String::new();
            loop {
                line.clear();
                match lines.read_line(&mut line) {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {}
                }
                if let Some(out) = before.as_mut() {
                    let _ = out.write_all(line.as_bytes());
                }
                let stamped = format!("[{:8.1}] {mark}{}", log.started.elapsed().as_secs_f64(), line.trim_end_matches(['\r', '\n']));
                if let Ok(mut f) = file.lock() {
                    let _ = writeln!(f, "{stamped}");
                }
                log.keep(stamped);
            }
        })
        .ok()?;
    Some(())
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
