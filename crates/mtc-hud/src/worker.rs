//! Work that can take long runs off the frame loop: V1 registration (25–50 ms), map identification
//! (about 33 ms) and fits of arrows cut by the map's edge (150–200 ms). Each kind has one
//! long-lived thread in the live HUD; replays and tests run the job inline, so their results stay
//! the same frame for frame. One job at a time: the loop hands one over, carries on with its
//! frames, and takes the result once it is back.
use std::sync::mpsc::{Receiver, Sender, TryRecvError, channel};

use mtc_vision::{FrameView, PixelFormat};

pub struct Worker<J, R> {
    mode: Mode<J, R>,
    busy: bool,
}

enum Mode<J, R> {
    Thread { jobs: Sender<J>, results: Receiver<R> },
    Inline { run: Box<dyn FnMut(J) -> R + Send>, ready: Option<R> },
}

impl<J: Send + 'static, R: Send + 'static> Worker<J, R> {
    /// `background`: on its own thread (live); otherwise inline (replay, tests).
    pub fn new(background: bool, name: &str, mut run: impl FnMut(J) -> R + Send + 'static) -> Self {
        let mode = if background {
            let (jobs, inbox) = channel::<J>();
            let (outbox, results) = channel::<R>();
            std::thread::Builder::new()
                .name(name.to_owned())
                .spawn(move || {
                    for job in inbox {
                        if outbox.send(run(job)).is_err() {
                            return;
                        }
                    }
                })
                .expect("worker thread");
            Mode::Thread { jobs, results }
        } else {
            Mode::Inline { run: Box::new(run), ready: None }
        };
        Self { mode, busy: false }
    }

    /// A job is out.
    pub fn busy(&self) -> bool {
        self.busy
    }

    /// Hands `job` over unless one is out; whether it went.
    pub fn send(&mut self, job: J) -> bool {
        if self.busy {
            return false;
        }
        match &mut self.mode {
            Mode::Thread { jobs, .. } => {
                if jobs.send(job).is_err() {
                    return false;
                }
            }
            Mode::Inline { run, ready } => *ready = Some(run(job)),
        }
        self.busy = true;
        true
    }

    /// The job's result, once it is back.
    pub fn take(&mut self) -> Option<R> {
        if !self.busy {
            return None;
        }
        let result = match &mut self.mode {
            Mode::Thread { results, .. } => match results.try_recv() {
                Ok(r) => Some(r),
                Err(TryRecvError::Empty) => None,
                Err(TryRecvError::Disconnected) => {
                    self.busy = false;
                    None
                }
            },
            Mode::Inline { ready, .. } => ready.take(),
        };
        if result.is_some() {
            self.busy = false;
        }
        result
    }
}

/// A frame region copied for a worker.
pub struct OwnedFrame {
    data: Vec<u8>,
    format: PixelFormat,
    origin: (i32, i32),
    width: u32,
    height: u32,
    stride: usize,
}

impl OwnedFrame {
    pub fn copy(v: &FrameView) -> Self {
        Self { data: v.data.to_vec(), format: v.format, origin: v.origin, width: v.width, height: v.height, stride: v.stride }
    }

    pub fn view(&self) -> FrameView<'_> {
        FrameView { data: &self.data, format: self.format, origin: self.origin, width: self.width, height: self.height, stride: self.stride }
    }
}
