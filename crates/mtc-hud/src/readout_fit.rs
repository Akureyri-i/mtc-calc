//! The angle readout's text scale and value-line offset found on the player's screen when the
//! configuration's rule does not read it well: Roblox's text size raised (every label and digit
//! about 1.3 times as large and the lines a pixel lower, one step up), or a configuration the rules
//! were not measured on (2560x1440 at 125 %: digits 5 % larger and 2 px lower than the rule, reads
//! costing about 77 against about 25 at the reference). Once reads have been poor for a while, the
//! readout's whole area is copied from a few frames a second apart and a try runs on a background
//! thread: candidates from a coarse search on the newest frame, judged on every copy against what
//! is in use. A candidate is taken when it reads more of the copies, or reads them much more
//! cheaply, and when a second try agrees.

use std::ops::RangeInclusive;
use std::sync::mpsc::{Receiver, TryRecvError, channel};
use std::time::{Duration, Instant};

use mtc_vision::aim::{AimReader, AimResult, ReadoutFit, TextScale};
use mtc_vision::{FrameView, PixelFormat};

/// Multiples of the rule scale searched coarsely (lowest, highest, step): the rule a little off, up
/// to Roblox's text size one step up (about 1.25–1.3); the best is refined by `FINE_STEP` each side.
const COARSE: (f32, f32, f32) = (0.9, 1.45, 0.05);
const FINE_STEP: f32 = 0.025;
/// Coarse results kept as candidates.
const SHORTLIST: usize = 3;
/// Value-line offsets tried (rows).
const DYS: RangeInclusive<i32> = -3..=5;
/// Frames a try judges on, at least this far apart.
const FRAMES: usize = 3;
const FRAME_GAP: Duration = Duration::from_secs(1);
/// How long reads are watched before they are judged, and how many poor windows in a row start a
/// try (a moment of snow behind the digits does not).
const WINDOW: Duration = Duration::from_secs(2);
const POOR_WINDOWS: u32 = 2;
/// Reads costing more than this on average are poor, and at least this many of them.
const POOR_COST: f32 = 45.0;
const POOR_READS: u32 = 3;
/// Found but unreadable this often (and at least a third of what was found) is poor too.
const POOR_UNREADABLE: u32 = 3;
/// A candidate is taken when it reads at least `MIN_READ` of the copies at a mean cost of at most
/// `TAKE_COST`, and reads more of them than what is in use or `BETTER_BY` more cheaply.
const MIN_READ: usize = 2;
const TAKE_COST: f32 = 40.0;
const BETTER_BY: f32 = 10.0;
/// Two tries agree within this factor and offset.
const AGREE_FACTOR: f32 = 0.051;
const AGREE_DY: i32 = 1;
/// After a try that found nothing, before the next; longer after several.
const RETRY: Duration = Duration::from_secs(5);
const RETRY_EMPTY: Duration = Duration::from_secs(30);
const EMPTY_TRIES: u32 = 3;
/// While the readout is absent, the label is first looked for at these multiples (the rule's, and
/// one text size step up) on the newest copy; the try goes on only when one scores this (one step
/// up, the rule's scale scored about 0.50 and 1.3 about 0.67). The rule's own is there for a game
/// set back to the default text size after a fit.
const LOOKS: [f32; 2] = [1.0, 1.3];
const LOOK_LABEL: f32 = 0.6;

/// A fit this many times the rule's or more is Roblox's text size raised (one step up fits about
/// 1.25–1.3), not a rule a little off (2560x1440 at 125 %: 1.05); the loading UI's text then grows
/// as much.
pub const TEXT_SIZE_RAISED: f32 = 1.15;

/// A copy of the readout's area, sent to the try's thread.
pub struct AreaCopy {
    data: Vec<u8>,
    format: PixelFormat,
    origin: (i32, i32),
    width: u32,
    height: u32,
}

impl AreaCopy {
    pub fn of(view: &FrameView) -> Self {
        let bpp = view.stride / view.width.max(1) as usize;
        let mut data = Vec::with_capacity(view.width as usize * view.height as usize * bpp);
        for row in 0..view.height as usize {
            data.extend_from_slice(&view.data[row * view.stride..row * view.stride + view.width as usize * bpp]);
        }
        Self { data, format: view.format, origin: view.origin, width: view.width, height: view.height }
    }

    fn view(&self) -> FrameView<'_> {
        let bpp = self.data.len() / (self.width as usize * self.height as usize).max(1);
        FrameView { data: &self.data, format: self.format, origin: self.origin, width: self.width, height: self.height, stride: self.width as usize * bpp }
    }
}

/// How one candidate reads the copies: how many decode and their mean cost.
struct Score {
    read: usize,
    cost: f32,
}

fn score(base: TextScale, copies: &[AreaCopy], origin: (i32, i32), factor: f32, dy: i32) -> Score {
    let costs: Vec<u32> = copies.iter().filter_map(|c| AimReader::fit(base, &c.view(), origin, &[factor], dy..=dy)).map(|f| f.cost).collect();
    Score { read: costs.len(), cost: if costs.is_empty() { f32::MAX } else { costs.iter().sum::<u32>() as f32 / costs.len() as f32 } }
}

/// One try on `copies` (newest last): the candidate to take, if any.
fn attempt(base: TextScale, copies: &[AreaCopy], origin: (i32, i32), current: ReadoutFit, probe_first: bool) -> Option<ReadoutFit> {
    let newest = copies.last()?.view();
    if probe_first {
        let looks = LOOKS.iter().filter(|&&f| (f - current.factor).abs() > 0.05);
        if !looks.into_iter().any(|&f| AimReader::label_score_at(base.times(f), &newest, origin).is_some_and(|s| s >= LOOK_LABEL)) {
            return None;
        }
    }
    let (lo, hi, step) = COARSE;
    let mut coarse: Vec<ReadoutFit> = (0..).map(|i| lo + i as f32 * step).take_while(|f| *f <= hi + 1e-4).filter_map(|f| AimReader::fit(base, &newest, origin, &[f], DYS)).collect();
    coarse.sort_by_key(|f| f.cost);
    let mut candidates: Vec<(f32, i32)> = coarse.iter().take(SHORTLIST).map(|f| (f.factor, f.dy)).collect();
    if let Some(best) = coarse.first() {
        for f in [best.factor - FINE_STEP, best.factor + FINE_STEP] {
            if let Some(fine) = AimReader::fit(base, &newest, origin, &[f], DYS) {
                candidates.push((fine.factor, fine.dy));
            }
        }
    }
    let now = score(base, copies, origin, current.factor, current.dy);
    let (factor, dy, best) = candidates
        .into_iter()
        .filter(|&(f, d)| (f - current.factor).abs() > 0.012 || d != current.dy)
        .map(|(f, d)| (f, d, score(base, copies, origin, f, d)))
        .max_by(|a, b| a.2.read.cmp(&b.2.read).then(b.2.cost.total_cmp(&a.2.cost)))?;
    let better = best.read > now.read || best.cost + BETTER_BY <= now.cost;
    (best.read >= MIN_READ && best.cost <= TAKE_COST && better).then(|| ReadoutFit { factor, dy, cost: best.cost.round() as u32 })
}

pub struct ReadoutFitter {
    base: TextScale,
    applied: Option<ReadoutFit>,
    pending: Option<ReadoutFit>,
    job: Option<Receiver<Option<ReadoutFit>>>,
    /// A try's result taken by `block` (replays), handed out by the next `poll`.
    ready: Option<Option<ReadoutFit>>,
    copies: Vec<AreaCopy>,
    last_copy: Option<Instant>,
    next_try: Instant,
    empty_tries: u32,
    window_start: Instant,
    reads: u32,
    cost: u64,
    unreadable: u32,
    absent: u32,
    /// Poor windows in a row, and whether the last one saw no readout at all (the label is then
    /// looked for first).
    poor_windows: u32,
    absent_only: bool,
}

impl ReadoutFitter {
    pub fn new(base: TextScale) -> Self {
        let now = Instant::now();
        Self {
            base,
            applied: None,
            pending: None,
            job: None,
            ready: None,
            copies: Vec::new(),
            last_copy: None,
            next_try: now,
            empty_tries: 0,
            window_start: now,
            reads: 0,
            cost: 0,
            unreadable: 0,
            absent: 0,
            poor_windows: 0,
            absent_only: false,
        }
    }

    /// Another configuration: back to its rule.
    pub fn reset(&mut self, base: TextScale) {
        *self = Self::new(base);
    }

    /// One frame's readout result.
    pub fn note(&mut self, result: &AimResult, now: Instant) {
        match result {
            AimResult::Read(r) => {
                self.reads += 1;
                self.cost += u64::from(r.cost);
            }
            AimResult::Unreadable { .. } => self.unreadable += 1,
            AimResult::Absent => self.absent += 1,
        }
        if now.saturating_duration_since(self.window_start) < WINDOW {
            return;
        }
        let poor_reads = self.reads >= POOR_READS && self.cost as f32 / self.reads as f32 > POOR_COST;
        let poor_unreadable = self.unreadable >= POOR_UNREADABLE && self.unreadable * 2 >= self.reads;
        let absent_only = self.reads == 0 && self.unreadable == 0 && self.absent > 0;
        if poor_reads || poor_unreadable || absent_only {
            self.poor_windows += 1;
            self.absent_only = absent_only;
        } else {
            self.poor_windows = 0;
            self.copies.clear();
        }
        (self.reads, self.cost, self.unreadable, self.absent) = (0, 0, 0, 0);
        self.window_start = now;
    }

    /// Whether this frame's readout area should be copied (`offer`) for a try.
    pub fn wants_area(&self, now: Instant) -> bool {
        let due = self.poor_windows >= POOR_WINDOWS || self.pending.is_some();
        due && self.job.is_none() && now >= self.next_try && self.last_copy.is_none_or(|t| now.saturating_duration_since(t) >= FRAME_GAP)
    }

    /// One frame's readout area; the try starts once `FRAMES` are in.
    pub fn offer(&mut self, area: AreaCopy, client_origin: (i32, i32), now: Instant) {
        self.copies.push(area);
        self.last_copy = Some(now);
        if self.copies.len() < FRAMES {
            return;
        }
        let copies = std::mem::take(&mut self.copies);
        let (base, probe_first) = (self.base, self.absent_only && self.pending.is_none());
        let current = self.applied.unwrap_or(ReadoutFit { factor: 1.0, dy: 0, cost: 0 });
        let (tx, rx) = channel();
        let spawned = std::thread::Builder::new().name("readout-fit".into()).spawn(move || {
            let _ = tx.send(attempt(base, &copies, client_origin, current, probe_first));
        });
        if spawned.is_ok() {
            self.job = Some(rx);
        }
    }

    /// A finished try: the fit to put in use now, if any (the caller sets the reader to it).
    pub fn poll(&mut self, now: Instant) -> Option<ReadoutFit> {
        let result = match self.ready.take() {
            Some(r) => r,
            None => match self.job.as_ref().map(Receiver::try_recv) {
                Some(Ok(r)) => r,
                Some(Err(TryRecvError::Empty)) | None => return None,
                Some(Err(TryRecvError::Disconnected)) => None,
            },
        };
        self.job = None;
        let Some(fit) = result else {
            self.empty_tries += 1;
            self.pending = None;
            self.next_try = now + if self.empty_tries >= EMPTY_TRIES { RETRY_EMPTY } else { RETRY };
            return None;
        };
        self.empty_tries = 0;
        let agrees = |a: ReadoutFit, b: ReadoutFit| (a.factor - b.factor).abs() <= AGREE_FACTOR && (a.dy - b.dy).abs() <= AGREE_DY;
        if self.pending.is_some_and(|p| agrees(p, fit)) {
            self.pending = None;
            self.applied = Some(fit);
            return Some(fit);
        }
        self.pending = Some(fit);
        None
    }

    /// Waits for a running try (replays: the result lands on the frame it was started on).
    pub fn block(&mut self) {
        if let Some(rx) = &self.job {
            self.ready = Some(rx.recv().ok().flatten());
        }
    }

    /// The scale a fit puts the reader at.
    pub fn scale_for(&self, fit: ReadoutFit) -> TextScale {
        self.base.times(fit.factor)
    }
}
