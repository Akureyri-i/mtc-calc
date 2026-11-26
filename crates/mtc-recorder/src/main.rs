//! Records Roblox frames into `datasets/raw/<session>` while the user plays.
//!
//! - Periodic: one full frame every `--periodic` seconds.
//! - Mark (F7): saves the last `--history` seconds kept in memory plus the next `--follow`
//!   seconds, both at `--rate` frames per second (default 5).
//! - Snapshot (F8): saves the current frame.
//! - Continuous (F6): toggles saving every frame at `--rate`.
//! - Map: while the full map (V1) or the scope map (V2) is open, frames are saved at `--map-rate`
//!   without any key, so a whole match can be recorded while playing normally.

mod capture;
mod hotkeys;
mod keys;
mod writer;

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use clap::Parser;
use mtc_capture::window::{WindowGeometry, enable_per_monitor_dpi_awareness, find_roblox};
use mtc_dataset::SessionInfo;

#[derive(Parser, Clone, Debug)]
#[command(about = "Record Roblox frames for the MTC calculator dataset")]
pub struct Args {
    /// Output root; a session directory is created inside it.
    #[arg(long)]
    pub out: Option<PathBuf>,
    /// Frame rate for mark history, mark follow-up and continuous recording. MTC frames are
    /// about 6 MB as lossless PNG, so 5 fps keeps a mark near 300 MB.
    #[arg(long, default_value_t = 5.0)]
    pub rate: f64,
    /// Seconds kept in memory and saved when a mark is set.
    #[arg(long, default_value_t = 6.0)]
    pub history: f64,
    /// Seconds recorded after a mark.
    #[arg(long, default_value_t = 4.0)]
    pub follow: f64,
    /// Seconds between periodic frames (0 disables them).
    #[arg(long, default_value_t = 10.0)]
    pub periodic: f64,
    /// Frames per second saved while V1 or V2 is open (0 disables it).
    #[arg(long, default_value_t = 2.0)]
    pub map_rate: f64,
    /// PNG encoder threads.
    #[arg(long, default_value_t = 3)]
    pub encoders: usize,
    /// Set a mark automatically every N seconds (0 disables it), for unattended recording.
    #[arg(long, default_value_t = 0.0)]
    pub auto_mark: f64,
    /// Stop after N seconds (0 records until Ctrl+C).
    #[arg(long, default_value_t = 0.0)]
    pub duration: f64,
    #[arg(long, default_value = "F7")]
    pub mark_key: String,
    #[arg(long, default_value = "F8")]
    pub snapshot_key: String,
    #[arg(long, default_value = "F6")]
    pub continuous_key: String,
}

/// State shared between the hotkey thread, the capture callback and the writers.
#[derive(Default)]
pub struct Control {
    pub stop: AtomicBool,
    pub mark: AtomicBool,
    pub snapshot: AtomicBool,
    pub continuous: AtomicBool,
    pub captured: AtomicU64,
    pub saved: AtomicU64,
    pub map_frames: AtomicU64,
    pub saved_bytes: AtomicU64,
    pub history_bytes: AtomicU64,
    pub readback_us: AtomicU64,
    pub readbacks: AtomicU64,
}

fn main() {
    enable_per_monitor_dpi_awareness();
    let args = Args::parse();

    let root = args
        .out
        .clone()
        .unwrap_or_else(|| mtc_dataset::datasets_dir().join("raw"));
    let started = chrono::Local::now();
    let id = started.format("%Y%m%d-%H%M%S").to_string();
    let dir = root.join(&id);
    std::fs::create_dir_all(dir.join("frames")).expect("create session directory");

    let control = Arc::new(Control::default());
    {
        let control = control.clone();
        ctrlc::set_handler(move || control.stop.store(true, Ordering::SeqCst))
            .expect("install Ctrl+C handler");
    }
    hotkeys::spawn(&args, control.clone());
    keys::spawn(dir.join("keys.jsonl"), control.clone());
    if args.duration > 0.0 {
        let control = control.clone();
        let duration = Duration::from_secs_f64(args.duration);
        std::thread::spawn(move || {
            std::thread::sleep(duration);
            control.stop.store(true, Ordering::SeqCst);
        });
    }
    if args.auto_mark > 0.0 {
        let control = control.clone();
        let period = Duration::from_secs_f64(args.auto_mark);
        std::thread::spawn(move || {
            loop {
                std::thread::sleep(period);
                control.mark.store(true, Ordering::SeqCst);
            }
        });
    }
    let writer = writer::Writer::spawn(&args, dir.clone(), control.clone());

    println!("MTC recorder · session {id}");
    println!("  Output folder: {}", dir.display());
    println!(
        "  {} = mark (saves {} s before + {} s after)   {} = single frame   {} = continuous recording on/off",
        args.mark_key, args.history, args.follow, args.snapshot_key, args.continuous_key
    );
    println!("  A frame saved every {} s; Ctrl+C ends.", args.periodic);
    if args.map_rate > 0.0 {
        println!("  With the full map or the scope map open, {} frames a second.", args.map_rate);
    }

    let mut info_written = false;
    let started_at = Instant::now();
    while !control.stop.load(Ordering::SeqCst) {
        let Some(window) = find_roblox() else {
            println!("Waiting for the Roblox window...");
            wait(&control, Duration::from_secs(2));
            continue;
        };
        let geometry = WindowGeometry::query(&window).ok();
        if !info_written {
            let info = SessionInfo {
                id: id.clone(),
                started_at: started.to_rfc3339(),
                recorder_version: env!("CARGO_PKG_VERSION").to_owned(),
                geometry,
            };
            std::fs::write(
                dir.join("session.json"),
                serde_json::to_vec_pretty(&info).expect("serialize session"),
            )
            .expect("write session.json");
            info_written = true;
        }
        if let Some(g) = geometry {
            println!(
                "Roblox found: window {}x{}, client offset ({}, {}), DPI {}",
                g.frame.width,
                g.frame.height,
                g.client.x - g.frame.x,
                g.client.y - g.frame.y,
                g.dpi
            );
        }
        capture::run(window, &args, control.clone(), writer.sender(), started_at);
        if !control.stop.load(Ordering::SeqCst) {
            println!("The Roblox window closed, waiting again...");
        }
    }

    println!("Writing the remaining frames...");
    writer.finish();
    println!(
        "Done: {} frames saved, {:.1} MB.",
        control.saved.load(Ordering::SeqCst),
        control.saved_bytes.load(Ordering::SeqCst) as f64 / 1e6
    );
}

fn wait(control: &Control, duration: Duration) {
    let deadline = Instant::now() + duration;
    while Instant::now() < deadline && !control.stop.load(Ordering::SeqCst) {
        std::thread::sleep(Duration::from_millis(100));
    }
}
