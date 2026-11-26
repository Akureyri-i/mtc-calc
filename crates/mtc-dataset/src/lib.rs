//! Dataset layout shared by the recorder, the labelling tools and the recognizer tests.
//!
//! ```text
//! datasets/
//!   raw/<session>/              local only (git-ignored)
//!     session.json              SessionInfo
//!     frames.jsonl              one FrameMeta per saved frame, in save order
//!     frames/<t_ms>.png         full captured frame, BGRA converted to RGB
//!     keys.jsonl                one KeyEvent per game key pressed or released (newer recordings)
//!   labels/<session>.jsonl      committed: one FrameLabel per labelled frame
//! ```
//!
//! Frame coordinates are pixels of the captured frame (the window's visible bounds, title bar
//! included). `FrameMeta::geometry` converts them to client or screen coordinates.

pub mod eval;
pub mod image;
pub mod labels;

use std::fs::File;
use std::io::{self, BufRead, BufReader};
use std::path::{Path, PathBuf};

use mtc_capture::window::WindowGeometry;
use serde::{Deserialize, Serialize};

pub use labels::FrameLabel;

/// Written once when a recording session starts.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SessionInfo {
    pub id: String,
    /// Local start time, RFC 3339.
    pub started_at: String,
    pub recorder_version: String,
    /// Window geometry when capture first started.
    pub geometry: Option<WindowGeometry>,
}

/// Why a frame was saved.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SaveReason {
    /// Regular sample for background and scene variety.
    Periodic,
    /// From the in-memory history dumped by a mark.
    MarkHistory,
    /// Recorded at the burst rate right after a mark.
    MarkFollow,
    /// Single snapshot requested by hotkey.
    Snapshot,
    /// Continuous recording toggled by hotkey.
    Continuous,
    /// Saved automatically while the full map (V1) or the scope map (V2) is open.
    Map,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FrameMeta {
    /// Image path relative to the session directory.
    pub file: String,
    /// Capture time in milliseconds since the session started (frame timestamp, not save time).
    pub t_ms: u64,
    /// Wall clock at capture, Unix milliseconds.
    pub unix_ms: u64,
    pub reason: SaveReason,
    /// Index of the mark this frame belongs to, for mark history and follow frames.
    pub mark: Option<u32>,
    pub width: u32,
    pub height: u32,
    pub geometry: WindowGeometry,
    /// Cursor in frame coordinates (can lie outside the frame).
    pub cursor: Option<(i32, i32)>,
    pub roblox_foreground: bool,
}

/// A game key (F, M, X, 1–6) pressed or released while Roblox was in front.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct KeyEvent {
    /// Virtual-key code.
    pub vk: u16,
    pub down: bool,
    /// Wall clock, Unix milliseconds.
    pub unix_ms: u64,
}

/// A recorded session on disk.
pub struct Session {
    pub dir: PathBuf,
    pub info: SessionInfo,
    pub frames: Vec<FrameMeta>,
}

impl Session {
    pub fn open(dir: impl AsRef<Path>) -> io::Result<Self> {
        let dir = dir.as_ref().to_path_buf();
        let info = serde_json::from_reader(File::open(dir.join("session.json"))?)?;
        let frames = read_jsonl(&dir.join("frames.jsonl"))?;
        Ok(Self { dir, info, frames })
    }

    pub fn frame_path(&self, meta: &FrameMeta) -> PathBuf {
        self.dir.join(&meta.file)
    }

    /// The game keys logged, oldest first, each with its session time (ms, the frames' clock);
    /// none for recordings made before keys were logged.
    pub fn keys(&self) -> Vec<(u64, KeyEvent)> {
        let mut offsets: Vec<i64> = self.frames.iter().map(|f| f.unix_ms as i64 - f.t_ms as i64).collect();
        offsets.sort_unstable();
        let Some(&offset) = offsets.get(offsets.len() / 2) else { return Vec::new() };
        let keys: Vec<KeyEvent> = read_jsonl(&self.dir.join("keys.jsonl")).unwrap_or_default();
        keys.into_iter().map(|k| ((k.unix_ms as i64 - offset).max(0) as u64, k)).collect()
    }
}

/// Repository root `datasets/` directory.
pub fn datasets_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("crate lives in <root>/crates/<name>")
        .join("datasets")
}

/// Whether the recorded sessions are on this machine (`datasets/`, kept out of the public
/// repository): the tests over them skip without them.
pub fn recordings_present() -> bool {
    datasets_dir().join("raw").is_dir() && datasets_dir().join("labels").is_dir()
}

/// Every recorded session under `datasets/raw`, oldest first.
pub fn sessions() -> io::Result<Vec<Session>> {
    let raw = datasets_dir().join("raw");
    let mut dirs: Vec<PathBuf> = match std::fs::read_dir(&raw) {
        Ok(entries) => entries
            .filter_map(Result::ok)
            .map(|e| e.path())
            .filter(|p| p.join("session.json").is_file())
            .collect(),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Vec::new(),
        Err(error) => return Err(error),
    };
    dirs.sort();
    dirs.into_iter().map(Session::open).collect()
}

/// Labels of one session, from `datasets/labels/<session>.jsonl`.
pub fn labels(session_id: &str) -> io::Result<Vec<FrameLabel>> {
    let path = datasets_dir().join("labels").join(format!("{session_id}.jsonl"));
    if !path.is_file() {
        return Ok(Vec::new());
    }
    read_jsonl(&path)
}

pub fn read_jsonl<T: for<'de> Deserialize<'de>>(path: &Path) -> io::Result<Vec<T>> {
    let mut items = Vec::new();
    for line in BufReader::new(File::open(path)?).lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        items.push(serde_json::from_str(&line)?);
    }
    Ok(items)
}
