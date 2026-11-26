//! Dataset inspection tool.
//!
//! - `mtc-data list`: sessions with frame counts, marks and size on disk.
//! - `mtc-data sheet <session> --roi x,y,w,h`: tiles the same region of many frames into one
//!   image for labelling, and prints the frame order.

use std::collections::BTreeMap;
use std::path::PathBuf;

use clap::{Parser, Subcommand};
use mtc_capture::window::{Rect, WindowGeometry};
use mtc_dataset::image::RgbImage;
use mtc_dataset::{FrameMeta, SaveReason, Session, SessionInfo, datasets_dir, labels, sessions};

#[derive(Parser)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// List recorded sessions.
    List,
    /// Create a session from existing full-window screenshots (PNG).
    Import {
        session: String,
        files: Vec<PathBuf>,
        /// Client area origin inside the screenshot.
        #[arg(long, value_delimiter = ',', default_values_t = [0, 34])]
        client_offset: Vec<i32>,
        #[arg(long, default_value_t = 144)]
        dpi: u32,
    },
    /// Tile one region of many frames into a contact sheet.
    Sheet {
        session: String,
        /// Region in frame pixels: x,y,width,height.
        #[arg(long, value_delimiter = ',', required = true)]
        roi: Vec<u32>,
        /// Use every Nth frame.
        #[arg(long, default_value_t = 1)]
        every: usize,
        /// Skip this many frames first.
        #[arg(long, default_value_t = 0)]
        skip: usize,
        /// Maximum tiles.
        #[arg(long, default_value_t = 48)]
        limit: usize,
        /// Only frames saved for this reason (periodic, mark_history, mark_follow, snapshot, continuous).
        #[arg(long)]
        reason: Option<String>,
        #[arg(long, default_value_t = 2)]
        scale: u32,
        #[arg(long, default_value_t = 6)]
        columns: u32,
        #[arg(long)]
        out: PathBuf,
    },
}

fn main() -> std::io::Result<()> {
    match Cli::parse().command {
        Command::List => list(),
        Command::Import {
            session,
            files,
            client_offset,
            dpi,
        } => import(&session, &files, &client_offset, dpi),
        Command::Sheet {
            session,
            roi,
            every,
            skip,
            limit,
            reason,
            scale,
            columns,
            out,
        } => sheet(&session, &roi, every, skip, limit, reason, scale, columns, &out),
    }
}

fn list() -> std::io::Result<()> {
    for session in sessions()? {
        let mut reasons: BTreeMap<String, usize> = BTreeMap::new();
        for meta in &session.frames {
            *reasons
                .entry(serde_json::to_string(&meta.reason)?.trim_matches('"').to_owned())
                .or_default() += 1;
        }
        let marks = session.frames.iter().filter_map(|m| m.mark).max().unwrap_or(0);
        let bytes: u64 = session
            .frames
            .iter()
            .filter_map(|m| std::fs::metadata(session.frame_path(m)).ok())
            .map(|m| m.len())
            .sum();
        let labelled = labels(&session.info.id)?.len();
        println!(
            "{}  {} frames ({:?}), {} marks, {} labelled, {:.0} MB",
            session.info.id,
            session.frames.len(),
            reasons,
            marks,
            labelled,
            bytes as f64 / 1e6
        );
    }
    println!("root: {}", datasets_dir().display());
    Ok(())
}

fn import(id: &str, files: &[PathBuf], client_offset: &[i32], dpi: u32) -> std::io::Result<()> {
    let dir = datasets_dir().join("raw").join(id);
    std::fs::create_dir_all(dir.join("frames"))?;
    let mut lines = String::new();
    let mut geometry = None;
    for (i, path) in files.iter().enumerate() {
        let image = RgbImage::load_png(path)?;
        let (w, h) = (image.width as i32, image.height as i32);
        let g = WindowGeometry {
            frame: Rect { x: 0, y: 0, width: w, height: h },
            client: Rect {
                x: client_offset[0],
                y: client_offset[1],
                width: w - client_offset[0],
                height: h - client_offset[1],
            },
            dpi,
        };
        geometry.get_or_insert(g);
        let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("frame");
        let file = format!("frames/{stem}.png");
        image.save_png(&dir.join(&file))?;
        let meta = FrameMeta {
            file,
            t_ms: i as u64,
            unix_ms: 0,
            reason: SaveReason::Snapshot,
            mark: None,
            width: image.width,
            height: image.height,
            geometry: g,
            cursor: None,
            roblox_foreground: true,
        };
        lines += &serde_json::to_string(&meta)?;
        lines.push('\n');
    }
    std::fs::write(dir.join("frames.jsonl"), lines)?;
    let info = SessionInfo {
        id: id.to_owned(),
        started_at: String::new(),
        recorder_version: format!("import {}", env!("CARGO_PKG_VERSION")),
        geometry,
    };
    std::fs::write(dir.join("session.json"), serde_json::to_vec_pretty(&info)?)?;
    println!("imported {} frames into {}", files.len(), dir.display());
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn sheet(
    id: &str,
    roi: &[u32],
    every: usize,
    skip: usize,
    limit: usize,
    reason: Option<String>,
    scale: u32,
    columns: u32,
    out: &PathBuf,
) -> std::io::Result<()> {
    let session = Session::open(datasets_dir().join("raw").join(id))?;
    let [x, y, w, h] = [roi[0], roi[1], roi[2], roi[3]];
    let mut frames: Vec<_> = session
        .frames
        .iter()
        .filter(|m| {
            reason.as_deref().is_none_or(|r| {
                serde_json::to_string(&m.reason).is_ok_and(|s| s.trim_matches('"') == r)
            })
        })
        .collect();
    frames.sort_by_key(|m| m.t_ms);
    let picked: Vec<_> = frames.into_iter().skip(skip).step_by(every.max(1)).take(limit).collect();

    let gap = 4;
    let (tw, th) = (w * scale, h * scale);
    let rows = (picked.len() as u32).div_ceil(columns).max(1);
    let mut canvas = RgbImage::new(columns * (tw + gap) - gap, rows * (th + gap) - gap);
    canvas.data.fill(40);
    for (i, meta) in picked.iter().enumerate() {
        let tile = RgbImage::load_png(&session.frame_path(meta))?
            .crop(x, y, w, h)
            .scale_nearest(scale);
        let (col, row) = (i as u32 % columns, i as u32 / columns);
        canvas.paste(&tile, col * (tw + gap), row * (th + gap));
        println!("{:3} r{} c{}  {}", i, row, col, meta.file);
    }
    canvas.save_png(out)
}
