//! Keeps the in-memory history (QOI-compressed), encodes saved frames to PNG on a small thread
//! pool and appends their metadata to `frames.jsonl`.

use std::collections::{BTreeSet, VecDeque};
use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::thread::JoinHandle;

use crossbeam_channel::{Receiver, Sender};
use mtc_capture::window::WindowGeometry;
use mtc_dataset::{FrameMeta, SaveReason};

use crate::{Args, Control};

/// A frame read back from the GPU, BGRA rows.
pub struct Captured {
    pub t_ms: u64,
    pub unix_ms: u64,
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
    pub geometry: WindowGeometry,
    pub cursor: Option<(i32, i32)>,
    pub foreground: bool,
    /// Keep in the in-memory history.
    pub history: bool,
    /// Save to disk now.
    pub save: Option<SaveReason>,
    pub mark: Option<u32>,
}

pub enum Packet {
    Frame(Captured),
    /// Save the in-memory history under this mark.
    Mark { index: u32 },
}

enum Pixels {
    Bgra(Vec<u8>),
    Qoi(Vec<u8>),
}

struct Job {
    meta: FrameMeta,
    pixels: Pixels,
}

struct HistoryEntry {
    meta: FrameMeta,
    qoi: Vec<u8>,
}

pub struct Writer {
    sender: Sender<Packet>,
    threads: Vec<JoinHandle<()>>,
}

impl Writer {
    pub fn spawn(args: &Args, dir: PathBuf, control: Arc<Control>) -> Self {
        let (sender, packets) = crossbeam_channel::bounded::<Packet>(32);
        let (job_sender, jobs) = crossbeam_channel::bounded::<Job>(512);
        let (meta_sender, metas) = crossbeam_channel::unbounded::<FrameMeta>();
        let mut threads = Vec::new();

        for _ in 0..args.encoders.max(1) {
            let jobs = jobs.clone();
            let meta_sender = meta_sender.clone();
            let dir = dir.clone();
            let control = control.clone();
            threads.push(std::thread::spawn(move || {
                for job in jobs {
                    match encode(&dir, &job) {
                        Ok(bytes) => {
                            control.saved.fetch_add(1, Ordering::Relaxed);
                            control.saved_bytes.fetch_add(bytes, Ordering::Relaxed);
                            let _ = meta_sender.send(job.meta);
                        }
                        Err(error) => eprintln!("Saving {} failed: {error}", job.meta.file),
                    }
                }
            }));
        }
        drop(meta_sender);
        drop(jobs);

        let jsonl = dir.join("frames.jsonl");
        threads.push(std::thread::spawn(move || write_metas(&jsonl, metas)));

        let history_ms = (args.history * 1000.0) as u64;
        threads.push(std::thread::spawn(move || {
            dispatch(packets, job_sender, history_ms, &control);
        }));

        Self { sender, threads }
    }

    pub fn sender(&self) -> Sender<Packet> {
        self.sender.clone()
    }

    /// Waits until every queued frame is on disk.
    pub fn finish(self) {
        drop(self.sender);
        for thread in self.threads {
            let _ = thread.join();
        }
    }
}

fn meta_for(c: &Captured, reason: SaveReason, mark: Option<u32>) -> FrameMeta {
    FrameMeta {
        file: format!("frames/{:09}.png", c.t_ms),
        t_ms: c.t_ms,
        unix_ms: c.unix_ms,
        reason,
        mark,
        width: c.width,
        height: c.height,
        geometry: c.geometry,
        cursor: c.cursor,
        roblox_foreground: c.foreground,
    }
}

fn dispatch(packets: Receiver<Packet>, jobs: Sender<Job>, history_ms: u64, control: &Control) {
    let mut history: VecDeque<HistoryEntry> = VecDeque::new();
    let mut history_bytes = 0usize;
    // Capture times already saved, so a mark never writes the same frame twice.
    let mut saved: BTreeSet<u64> = BTreeSet::new();

    for packet in packets {
        match packet {
            Packet::Frame(captured) => {
                if captured.history {
                    let qoi = qoi::encode_to_vec(&captured.pixels, captured.width, captured.height)
                        .expect("QOI encode of a full frame");
                    history_bytes += qoi.len();
                    history.push_back(HistoryEntry {
                        meta: meta_for(&captured, SaveReason::MarkHistory, None),
                        qoi,
                    });
                    while history
                        .front()
                        .is_some_and(|e| e.meta.t_ms + history_ms < captured.t_ms)
                    {
                        history_bytes -= history.pop_front().map_or(0, |e| e.qoi.len());
                    }
                    control
                        .history_bytes
                        .store(history_bytes as u64, Ordering::Relaxed);
                }
                if let Some(reason) = captured.save {
                    saved.insert(captured.t_ms);
                    let meta = meta_for(&captured, reason, captured.mark);
                    let _ = jobs.send(Job {
                        meta,
                        pixels: Pixels::Bgra(captured.pixels),
                    });
                }
                if let Some(&oldest) = history.front().map(|e| &e.meta.t_ms) {
                    saved = saved.split_off(&oldest);
                }
            }
            Packet::Mark { index } => {
                for entry in &history {
                    if !saved.insert(entry.meta.t_ms) {
                        continue;
                    }
                    let mut meta = entry.meta.clone();
                    meta.mark = Some(index);
                    let _ = jobs.send(Job {
                        meta,
                        pixels: Pixels::Qoi(entry.qoi.clone()),
                    });
                }
            }
        }
    }
}

fn encode(dir: &Path, job: &Job) -> std::io::Result<u64> {
    let decoded;
    let bgra: &[u8] = match &job.pixels {
        Pixels::Bgra(pixels) => pixels,
        Pixels::Qoi(qoi) => {
            decoded = qoi::decode_to_vec(qoi)
                .map_err(|e| std::io::Error::other(e.to_string()))?
                .1;
            &decoded
        }
    };
    let rgb: Vec<u8> = bgra
        .chunks_exact(4)
        .flat_map(|p| [p[2], p[1], p[0]])
        .collect();
    let path = dir.join(&job.meta.file);
    let mut encoder = png::Encoder::new(
        BufWriter::new(File::create(&path)?),
        job.meta.width,
        job.meta.height,
    );
    encoder.set_color(png::ColorType::Rgb);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.set_compression(png::Compression::Fast);
    let mut writer = encoder.write_header().map_err(std::io::Error::other)?;
    writer
        .write_image_data(&rgb)
        .map_err(std::io::Error::other)?;
    writer.finish().map_err(std::io::Error::other)?;
    Ok(std::fs::metadata(&path)?.len())
}

fn write_metas(path: &Path, metas: Receiver<FrameMeta>) {
    let mut file = BufWriter::new(
        OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .expect("open frames.jsonl"),
    );
    for meta in metas {
        let line = serde_json::to_string(&meta).expect("serialize frame metadata");
        let _ = writeln!(file, "{line}");
        let _ = file.flush();
    }
}
