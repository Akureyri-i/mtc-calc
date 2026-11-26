//! Throughput, per-stage cost, latency and process resource use.
//!
//! CPU is counted in cycles (`QueryThreadCycleTime`, `QueryProcessCycleTime`), not in the thread
//! and process times: those are charged per clock tick (15.6 ms) and were off by -45% to +200% at
//! the HUD's loads (`examples/capture_cost.rs`). The tick-charged process time is still
//! printed next to it, to compare with logs from before.

use std::time::{Duration, Instant};

use mtc_vision::aim::AimResult;
use windows::Win32::Foundation::{CloseHandle, FILETIME};
use windows::Win32::System::Performance::{QueryPerformanceCounter, QueryPerformanceFrequency};
use windows::Win32::System::ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS, PROCESS_MEMORY_COUNTERS_EX, PSAPI_WORKING_SET_EX_INFORMATION, QueryWorkingSetEx};
use windows::Win32::System::Diagnostics::ToolHelp::{CreateToolhelp32Snapshot, TH32CS_SNAPTHREAD, THREADENTRY32, Thread32First, Thread32Next};
use windows::Win32::System::WindowsProgramming::{QueryProcessCycleTime, QueryThreadCycleTime};
use windows::Win32::System::Threading::{
    GetCurrentProcess, GetCurrentProcessId, GetCurrentThread, GetProcessTimes, GetThreadDescription, OpenThread, SetThreadDescription,
    THREAD_QUERY_INFORMATION, THREAD_QUERY_LIMITED_INFORMATION,
};

/// Names the calling thread (the frame loop runs on the main thread, which has no name).
pub fn name_this_thread(name: &str) {
    let wide: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
    unsafe {
        let _ = SetThreadDescription(GetCurrentThread(), windows::core::PCWSTR(wide.as_ptr()));
    }
}

/// Cycles per second of the cycle counter (it runs at a fixed rate), from a busy loop at the first
/// call.
fn cycle_hz() -> f64 {
    static HZ: std::sync::OnceLock<f64> = std::sync::OnceLock::new();
    *HZ.get_or_init(|| {
        let start_cycles = this_thread_cycles();
        let start = Instant::now();
        let mut x = 0u64;
        while start.elapsed() < Duration::from_millis(100) {
            x = std::hint::black_box(x.wrapping_mul(6364136223846793005).wrapping_add(1));
        }
        (this_thread_cycles() - start_cycles) as f64 / start.elapsed().as_secs_f64()
    })
}

fn this_thread_cycles() -> u64 {
    let mut c = 0u64;
    unsafe {
        let _ = QueryThreadCycleTime(GetCurrentThread(), &mut c);
    }
    c
}

/// Cycles the frame callback spends inside the HUD's own code (the rest of the capture thread's is
/// the capture library's and the message dispatch's).
pub static HANDLER_CYCLES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Adds the cycles from its creation to its drop to `HANDLER_CYCLES`.
pub struct HandlerCycles(u64);

impl HandlerCycles {
    pub fn start() -> Self {
        Self(this_thread_cycles())
    }

    /// The cycle count when the callback started.
    pub fn started(&self) -> u64 {
        self.0
    }
}

impl Drop for HandlerCycles {
    fn drop(&mut self) {
        HANDLER_CYCLES.fetch_add(this_thread_cycles().saturating_sub(self.0), std::sync::atomic::Ordering::Relaxed);
    }
}

/// The calling thread's cycle count now, to start a `Stats::part`.
pub fn cycles() -> u64 {
    this_thread_cycles()
}

/// Where the frame callback's cycles go.
#[derive(Clone, Copy)]
pub enum Part {
    /// A frame not processed (input, timestamps, the interval check).
    Skipped,
    /// A processed frame before its read-back: input, timestamps, the window geometry.
    Top,
    /// Reading the readout box and the view cue regions back from the GPU.
    Readback,
    /// Reading the readout and the view cues.
    Decode,
    /// View changes, input, the landing prediction.
    Logic,
    /// The panels, the settings window and the map marks sent to the overlay.
    Panels,
    /// Whether a map view is due (with the tracking report).
    MapCheck,
    MapReadback,
    Map,
    LoadingReadback,
    Loading,
    /// Inside the loading read: the card texts, the weapon name, deciding the weapon.
    LoadingTexts,
    LoadingWeapon,
    LoadingDecide,
    SeatReadback,
    Seats,
    /// The overlay's actions (panel clicks, settings).
    Actions,
    Solve,
    Shots,
    Charges,
    Marks,
    /// These stats (every 5 s: the threads' CPU, the lines printed).
    Report,
    /// Another display configuration's regions resampled to the reference geometry (readout,
    /// map view, loading UI, seat menu), out of the parts they are read in.
    Remap,
}

const PARTS: usize = 23;
/// The parts' names in the run record.
const PART_NAMES_EN: [&str; PARTS] = [
    "skipped", "top", "readback", "decode", "logic", "panels", "map_check", "map_readback", "map", "loading_readback", "loading", "loading_texts", "loading_weapon", "loading_decide", "seat_readback", "seats", "actions", "solve", "shots", "charges", "marks", "report", "remap",
];
const PART_NAMES: [&str; PARTS] = [
    "skipped frames", "before readback", "readback", "decode", "views, input, prediction", "panels", "map due", "map readback", "map", "loading readback", "loading other", "loading: card texts", "loading: weapon name", "loading: weapon decision", "seat readback", "seats", "panel actions", "solutions", "shot log", "charge marks", "marks", "stats", "remap",
];

#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "system" {
    fn NtQueryInformationThread(thread: windows::Win32::Foundation::HANDLE, class: u32, info: *mut core::ffi::c_void, len: u32, returned: *mut u32) -> i32;
}

/// The file name of the module a thread started in (`ThreadQuerySetWin32StartAddress`): tells the
/// unnamed threads apart (the graphics driver's, the capture's, the thread pool's).
fn start_module(thread: windows::Win32::Foundation::HANDLE) -> String {
    use windows::Win32::System::LibraryLoader::{GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS, GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT, GetModuleFileNameW, GetModuleHandleExW};
    let mut address: usize = 0;
    let status = unsafe { NtQueryInformationThread(thread, 9, (&raw mut address).cast(), size_of::<usize>() as u32, std::ptr::null_mut()) };
    if status != 0 || address == 0 {
        return "?".into();
    }
    let mut module = windows::Win32::Foundation::HMODULE::default();
    let flags = GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT;
    if unsafe { GetModuleHandleExW(flags, windows::core::PCWSTR(address as *const u16), &mut module) }.is_err() {
        return "?".into();
    }
    let mut buf = [0u16; 260];
    let n = unsafe { GetModuleFileNameW(Some(module), &mut buf) } as usize;
    let path = String::from_utf16_lossy(&buf[..n]);
    path.rsplit('\\').next().unwrap_or(&path).to_owned()
}

/// How often this process's threads are listed again. Listing walks every thread on the system
/// through a Toolhelp snapshot (5721 threads, about 50 ms, `examples/stats_cost.rs`); two listings
/// every 5 s cost the HUD 0.037 cores. Between listings each report only queries the
/// threads already known (44, about 0.1 ms).
const THREAD_LIST_EVERY: Duration = Duration::from_secs(60);

/// A thread's description (its name), if it has one.
fn thread_name(h: windows::Win32::Foundation::HANDLE) -> Option<String> {
    let p = unsafe { GetThreadDescription(h) }.ok()?;
    let s = unsafe { p.to_string() }.unwrap_or_default();
    unsafe {
        let _ = windows::Win32::Foundation::LocalFree(Some(windows::Win32::Foundation::HLOCAL(p.0.cast())));
    }
    (!s.is_empty()).then_some(s)
}

/// A thread of this process: its handle (kept open, stored as an integer so the stats stay `Send`)
/// and its name ("@module" when unnamed: the module it started in).
struct WatchedThread {
    handle: isize,
    name: String,
}

impl Drop for WatchedThread {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(windows::Win32::Foundation::HANDLE(self.handle as *mut _));
        }
    }
}

/// This process's threads, listed every `THREAD_LIST_EVERY`.
#[derive(Default)]
struct ThreadWatch {
    threads: std::collections::HashMap<u32, WatchedThread>,
    listed: Option<Instant>,
}

impl ThreadWatch {
    /// CPU seconds (from cycles) and name of every thread of this process, by thread id.
    fn sample(&mut self) -> std::collections::HashMap<u32, (f64, String)> {
        if self.listed.is_none_or(|t| t.elapsed() >= THREAD_LIST_EVERY) {
            self.list();
        }
        self.threads
            .iter_mut()
            .map(|(&id, t)| {
                let handle = windows::Win32::Foundation::HANDLE(t.handle as *mut _);
                // A thread may be named after it was listed (the capture thread, at its first frame).
                if t.name.starts_with('@')
                    && let Some(name) = thread_name(handle)
                {
                    t.name = name;
                }
                let mut cycles = 0u64;
                let ok = unsafe { QueryThreadCycleTime(handle, &mut cycles) }.is_ok();
                (id, (if ok { cycles as f64 / cycle_hz() } else { 0.0 }, t.name.clone()))
            })
            .collect()
    }

    /// Lists this process's threads: opens the new ones, drops the ones gone.
    fn list(&mut self) {
        self.listed = Some(Instant::now());
        let pid = unsafe { GetCurrentProcessId() };
        let Ok(snap) = (unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) }) else { return };
        let mut entry = THREADENTRY32 { dwSize: size_of::<THREADENTRY32>() as u32, ..Default::default() };
        let mut more = unsafe { Thread32First(snap, &mut entry) }.is_ok();
        let mut seen = std::collections::HashSet::new();
        while more {
            if entry.th32OwnerProcessID == pid {
                seen.insert(entry.th32ThreadID);
            }
            more = unsafe { Thread32Next(snap, &mut entry) }.is_ok();
        }
        unsafe {
            let _ = CloseHandle(snap);
        }
        self.threads.retain(|id, _| seen.contains(id));
        for id in seen {
            if self.threads.contains_key(&id) {
                continue;
            }
            let Ok(h) = (unsafe { OpenThread(THREAD_QUERY_LIMITED_INFORMATION | THREAD_QUERY_INFORMATION, false, id) }) else { continue };
            let name = thread_name(h).unwrap_or_else(|| format!("@{}", start_module(h)));
            self.threads.insert(id, WatchedThread { handle: h.0 as isize, name });
        }
    }
}

/// The process's CPU seconds from cycles.
fn process_cpu_seconds() -> f64 {
    let mut cycles = 0u64;
    unsafe {
        let _ = QueryProcessCycleTime(GetCurrentProcess(), &mut cycles);
    }
    cycles as f64 / cycle_hz()
}

/// The process's CPU seconds as the system charges them (per clock tick), as logged before.
fn process_ticks_seconds() -> f64 {
    let (mut c, mut e, mut k, mut u) = (FILETIME::default(), FILETIME::default(), FILETIME::default(), FILETIME::default());
    unsafe {
        let _ = GetProcessTimes(GetCurrentProcess(), &mut c, &mut e, &mut k, &mut u);
    }
    let t = |f: FILETIME| ((u64::from(f.dwHighDateTime) << 32) | u64::from(f.dwLowDateTime)) as f64 / 1e7;
    t(k) + t(u)
}

/// Milliseconds from a captured frame's system-relative time (QPC, in 100 ns units) to now: how
/// old the picture is when it reaches the HUD.
pub fn frame_age_ms(system_relative_100ns: i64) -> f64 {
    let (mut now, mut freq) = (0i64, 0i64);
    unsafe {
        let _ = QueryPerformanceCounter(&mut now);
        let _ = QueryPerformanceFrequency(&mut freq);
    }
    let now_100ns = now as f64 * 1e7 / freq.max(1) as f64;
    (now_100ns - system_relative_100ns as f64) / 1e4
}

/// Bytes the program's own heap holds now (the counting allocator below): what is the program's
/// out of the private bytes; the rest is the graphics driver's, the capture library's and the
/// system's (in play 130–141 MB private against 80 MB found offline).
static HEAP: std::sync::atomic::AtomicIsize = std::sync::atomic::AtomicIsize::new(0);

pub struct CountingAlloc;

// SAFETY: forwards every call to the system allocator and only counts the sizes.
unsafe impl std::alloc::GlobalAlloc for CountingAlloc {
    unsafe fn alloc(&self, layout: std::alloc::Layout) -> *mut u8 {
        let p = unsafe { std::alloc::System.alloc(layout) };
        if !p.is_null() {
            HEAP.fetch_add(layout.size() as isize, std::sync::atomic::Ordering::Relaxed);
        }
        p
    }
    unsafe fn alloc_zeroed(&self, layout: std::alloc::Layout) -> *mut u8 {
        let p = unsafe { std::alloc::System.alloc_zeroed(layout) };
        if !p.is_null() {
            HEAP.fetch_add(layout.size() as isize, std::sync::atomic::Ordering::Relaxed);
        }
        p
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: std::alloc::Layout) {
        unsafe { std::alloc::System.dealloc(ptr, layout) };
        HEAP.fetch_sub(layout.size() as isize, std::sync::atomic::Ordering::Relaxed);
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: std::alloc::Layout, new_size: usize) -> *mut u8 {
        let p = unsafe { std::alloc::System.realloc(ptr, layout, new_size) };
        if !p.is_null() {
            HEAP.fetch_add(new_size as isize - layout.size() as isize, std::sync::atomic::Ordering::Relaxed);
        }
        p
    }
}

/// The program's heap (MB).
pub fn heap_mb() -> f64 {
    HEAP.load(std::sync::atomic::Ordering::Relaxed).max(0) as f64 / 1e6
}

/// Video memory this process uses on the first adapter (MB): local (the card's own) and non-local
/// (system memory the driver maps for the card, which counts in the private bytes).
fn video_memory_mb() -> Option<(f64, f64)> {
    use windows::Win32::Graphics::Dxgi::{CreateDXGIFactory1, DXGI_MEMORY_SEGMENT_GROUP_LOCAL, DXGI_MEMORY_SEGMENT_GROUP_NON_LOCAL, DXGI_QUERY_VIDEO_MEMORY_INFO, IDXGIAdapter3, IDXGIFactory1};
    use windows::core::Interface;
    static ADAPTER: std::sync::OnceLock<Option<IDXGIAdapter3>> = std::sync::OnceLock::new();
    let adapter = ADAPTER.get_or_init(|| unsafe {
        let factory: IDXGIFactory1 = CreateDXGIFactory1().ok()?;
        factory.EnumAdapters1(0).ok()?.cast::<IDXGIAdapter3>().ok()
    });
    let adapter = adapter.as_ref()?;
    let query = |group| {
        let mut info = DXGI_QUERY_VIDEO_MEMORY_INFO::default();
        unsafe { adapter.QueryVideoMemoryInfo(0, group, &mut info) }.ok().map(|()| info.CurrentUsage as f64 / 1e6)
    };
    Some((query(DXGI_MEMORY_SEGMENT_GROUP_LOCAL)?, query(DXGI_MEMORY_SEGMENT_GROUP_NON_LOCAL)?))
}

/// How much of `bytes` (a memory-mapped file's view) is in the working set (MB): its pages read
/// since the system last trimmed them.
pub fn resident_mb(bytes: &[u8]) -> f64 {
    const PAGE: usize = 4096;
    let start = bytes.as_ptr() as usize & !(PAGE - 1);
    let end = bytes.as_ptr() as usize + bytes.len();
    let mut pages: Vec<PSAPI_WORKING_SET_EX_INFORMATION> =
        (start..end).step_by(PAGE).map(|a| PSAPI_WORKING_SET_EX_INFORMATION { VirtualAddress: a as *mut _, ..Default::default() }).collect();
    let size = (pages.len() * size_of::<PSAPI_WORKING_SET_EX_INFORMATION>()) as u32;
    if unsafe { QueryWorkingSetEx(GetCurrentProcess(), pages.as_mut_ptr().cast(), size) }.is_err() {
        return f64::NAN;
    }
    // Bit 0 of the attributes: the page is valid (in the working set).
    pages.iter().filter(|p| unsafe { p.VirtualAttributes.Flags } & 1 == 1).count() as f64 * PAGE as f64 / 1e6
}

/// The working set (what the 150 MB memory budget is measured on) and the private
/// bytes (MB): the working set also holds the pages of memory-mapped files read, the heightmaps
/// among them, which the private bytes leave out.
pub fn memory_mb() -> (f64, f64) {
    let mut counters = PROCESS_MEMORY_COUNTERS_EX {
        cb: size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32,
        ..Default::default()
    };
    unsafe {
        let _ = GetProcessMemoryInfo(GetCurrentProcess(), (&raw mut counters).cast::<PROCESS_MEMORY_COUNTERS>(), counters.cb);
    }
    (counters.WorkingSetSize as f64 / 1e6, counters.PrivateUsage as f64 / 1e6)
}

pub struct Stats {
    pub frames_arrived: u64,
    /// Time spent in the callback on frames that are not processed (input, timestamps).
    pub skipped_work: Duration,
    processed: u64,
    read: u64,
    unreadable: u64,
    absent: u64,
    readback: Duration,
    decode: Duration,
    bytes: usize,
    latencies: Vec<f64>,
    /// Age of each processed frame on arrival (ms).
    pub frame_ages: Vec<f64>,
    /// Whole processing of each frame, and the map container's read-back and processing.
    pub frame_work: Duration,
    pub map_reads: u32,
    pub map_work: Duration,
    /// Map marks window redraws on the overlay thread and their time.
    pub map_draws: (u64, Duration),
    /// Firing solutions computed and their time.
    pub solves: (u32, Duration),
    /// Loading UI reads (read-back included) and seat menu reads, and their time.
    pub loading: (u32, Duration),
    pub seats: (u32, Duration),
    /// Intervals between frame arrivals (ms): the game's frame interval.
    pub game_intervals: Vec<f64>,
    /// Cycles and calls of each part of the frame callback.
    parts: [(u64, u32); PARTS],
    since: Instant,
    cpu_at: f64,
    ticks_at: f64,
    handler_at: u64,
    /// Each thread's CPU seconds at the last report.
    threads_at: std::collections::HashMap<u32, (f64, String)>,
    watch: ThreadWatch,
}

impl Stats {
    pub fn new() -> Self {
        let mut watch = ThreadWatch::default();
        let threads_at = watch.sample();
        Self::with(watch, threads_at)
    }

    fn with(watch: ThreadWatch, threads_at: std::collections::HashMap<u32, (f64, String)>) -> Self {
        Self {
            frames_arrived: 0,
            skipped_work: Duration::ZERO,
            processed: 0,
            read: 0,
            unreadable: 0,
            absent: 0,
            readback: Duration::ZERO,
            decode: Duration::ZERO,
            bytes: 0,
            latencies: Vec::new(),
            frame_ages: Vec::new(),
            frame_work: Duration::ZERO,
            map_reads: 0,
            map_work: Duration::ZERO,
            map_draws: (0, Duration::ZERO),
            solves: (0, Duration::ZERO),
            loading: (0, Duration::ZERO),
            seats: (0, Duration::ZERO),
            game_intervals: Vec::new(),
            parts: [(0, 0); PARTS],
            since: Instant::now(),
            cpu_at: process_cpu_seconds(),
            ticks_at: process_ticks_seconds(),
            handler_at: HANDLER_CYCLES.load(std::sync::atomic::Ordering::Relaxed),
            threads_at,
            watch,
        }
    }

    pub fn record(&mut self, readback: Duration, decode: Duration, bytes: usize, latency_ms: f64, result: &AimResult) {
        self.processed += 1;
        self.readback += readback;
        self.decode += decode;
        self.bytes += bytes;
        self.latencies.push(latency_ms);
        match result {
            AimResult::Read(_) => self.read += 1,
            AimResult::Unreadable { .. } => self.unreadable += 1,
            AimResult::Absent => self.absent += 1,
        }
    }

    /// Adds the cycles since `since` to `part` as one call; returns the cycle count now.
    pub fn part(&mut self, part: Part, since: u64) -> u64 {
        self.part_n(part, since, 1)
    }

    /// As `part`, less `less` cycles counted elsewhere (a step's own parts).
    pub fn part_less(&mut self, part: Part, since: u64, less: u64) -> u64 {
        let now = this_thread_cycles();
        let p = &mut self.parts[part as usize];
        p.0 += now.saturating_sub(since).saturating_sub(less);
        p.1 += 1;
        now
    }

    /// Adds cycles and calls counted elsewhere to `part`.
    pub fn add(&mut self, part: Part, (cycles, calls): (u64, u32)) {
        let p = &mut self.parts[part as usize];
        p.0 += cycles;
        p.1 += calls;
    }

    /// As `part`, counting `calls` calls (0: the time counts, the call did no work).
    pub fn part_n(&mut self, part: Part, since: u64, calls: u32) -> u64 {
        let now = this_thread_cycles();
        let p = &mut self.parts[part as usize];
        p.0 += now.saturating_sub(since);
        p.1 += calls;
        now
    }

    /// The report every 5 s; what it measured when it printed (the caller adds its own lines then).
    pub fn maybe_report(&mut self) -> Option<crate::diag::Report> {
        let elapsed = self.since.elapsed().as_secs_f64();
        if elapsed < 5.0 || self.processed == 0 {
            return None;
        }
        let cpu = process_cpu_seconds();
        let ticks = process_ticks_seconds();
        let handler = HANDLER_CYCLES.load(std::sync::atomic::Ordering::Relaxed);
        let n = self.processed as f64;
        self.latencies.sort_by(f64::total_cmp);
        let p = |q: f64| self.latencies[((self.latencies.len() - 1) as f64 * q) as usize];
        self.frame_ages.sort_by(f64::total_cmp);
        let age = if self.frame_ages.is_empty() { f64::NAN } else { self.frame_ages[self.frame_ages.len() / 2] };
        self.game_intervals.sort_by(f64::total_cmp);
        let game_frame = self.game_intervals.get(self.game_intervals.len() / 2).copied().unwrap_or(f64::NAN);
        let memory = memory_mb();
        let video = video_memory_mb();
        println!(
            "[stats] game frame median {game_frame:.1} ms · arrived {:.0} fps · processed {:.0} fps · readout read {} unreadable {} absent {} · readback {:.2} ms ({:.0} KB) · decode {:.2} ms · latency median {:.1} ms / p95 {:.1} ms · frame age median {age:.1} ms · per frame {:.2} ms · skipped frame {:.3} ms · map {:.1}/s x {:.2} ms · mark redraws {:.1}/s x {:.2} ms · solutions {:.1}/s x {:.0} ms · loading UI {:.1}/s x {:.2} ms · seats {:.1}/s x {:.2} ms · CPU {:.3} cores (by clock ticks {:.3}) · memory {:.0} MB (private {:.0} MB: heap {:.0} MB, the rest the driver's, the capture library's and the system's; video memory local {} MB, shared {} MB)",
            self.frames_arrived as f64 / elapsed,
            n / elapsed,
            self.read,
            self.unreadable,
            self.absent,
            self.readback.as_secs_f64() * 1000.0 / n,
            self.bytes as f64 / n / 1024.0,
            self.decode.as_secs_f64() * 1000.0 / n,
            p(0.5),
            p(0.95),
            self.frame_work.as_secs_f64() * 1000.0 / n,
            self.skipped_work.as_secs_f64() * 1000.0 / (self.frames_arrived.saturating_sub(self.processed)).max(1) as f64,
            f64::from(self.map_reads) / elapsed,
            self.map_work.as_secs_f64() * 1000.0 / f64::from(self.map_reads.max(1)),
            self.map_draws.0 as f64 / elapsed,
            self.map_draws.1.as_secs_f64() * 1000.0 / (self.map_draws.0.max(1)) as f64,
            f64::from(self.solves.0) / elapsed,
            self.solves.1.as_secs_f64() * 1000.0 / f64::from(self.solves.0.max(1)),
            f64::from(self.loading.0) / elapsed,
            self.loading.1.as_secs_f64() * 1000.0 / f64::from(self.loading.0.max(1)),
            f64::from(self.seats.0) / elapsed,
            self.seats.1.as_secs_f64() * 1000.0 / f64::from(self.seats.0.max(1)),
            (cpu - self.cpu_at) / elapsed,
            (ticks - self.ticks_at) / elapsed,
            memory.0,
            memory.1,
            heap_mb(),
            video.map_or("?".to_owned(), |v| format!("{:.0}", v.0)),
            video.map_or("?".to_owned(), |v| format!("{:.0}", v.1)),
        );
        // Per thread (named ones by name; the rest are the capture library's and the system's).
        let threads = self.watch.sample();
        let handler_cores = handler.saturating_sub(self.handler_at) as f64 / cycle_hz() / elapsed;
        // The frame callback by part (cycles): cores, and calls a second × ms a call.
        let hz = cycle_hz();
        let mut counted = 0.0;
        let parts: Vec<String> = self
            .parts
            .iter()
            .zip(PART_NAMES)
            .filter(|((c, _), _)| *c > 0)
            .map(|(&(c, n), name)| {
                let cores = c as f64 / hz / elapsed;
                counted += cores;
                if n > 0 { format!("{name} {cores:.4} ({:.1}/s × {:.2} ms)", f64::from(n) / elapsed, c as f64 / hz * 1000.0 / f64::from(n)) } else { format!("{name} {cores:.4}") }
            })
            .collect();
        println!("[CPU parts] {} · other {:.4} · frame callback total {handler_cores:.4} cores", parts.join(" · "), (handler_cores - counted).max(0.0));
        let mut named: Vec<(String, f64)> = Vec::new();
        let mut unnamed: Vec<(String, f64, u32)> = Vec::new();
        let (mut other, mut others) = (0.0, 0);
        for (id, (cpu_s, name)) in &threads {
            let before = self.threads_at.get(id).map_or(0.0, |t| t.0);
            let cores = (cpu_s - before).max(0.0) / elapsed;
            if let Some(module) = name.strip_prefix('@') {
                other += cores;
                others += 1;
                match unnamed.iter_mut().find(|(m, _, _)| m == module) {
                    Some(entry) => {
                        entry.1 += cores;
                        entry.2 += 1;
                    }
                    None => unnamed.push((module.to_owned(), cores, 1)),
                }
            } else {
                match named.iter_mut().find(|(n, _)| n == name) {
                    Some(entry) => entry.1 += cores,
                    None => named.push((name.clone(), cores)),
                }
            }
        }
        unnamed.sort_by(|a, b| b.1.total_cmp(&a.1));
        let by_module: Vec<String> = unnamed.iter().filter(|(_, c, _)| *c >= 0.0005).map(|(m, c, n)| format!("{m} {n} threads {c:.3}")).collect();
        named.sort_by(|a, b| b.1.total_cmp(&a.1));
        let parts: Vec<String> = named
            .iter()
            .filter(|(_, c)| *c >= 0.0005)
            .map(|(n, c)| if n == "capture" { format!("capture {c:.3} (HUD work {handler_cores:.3}, capture library and messages {:.3})", (c - handler_cores).max(0.0)) } else { format!("{n} {c:.3}") })
            .collect();
        println!("[threads] {} · {others} unnamed {other:.3} cores (by start module: {})", parts.join(" · "), by_module.join(" · "));
        let report = crate::diag::Report {
            cpu_cores: (cpu - self.cpu_at) / elapsed,
            working_set_mb: memory.0,
            heap_mb: heap_mb(),
            game_frame_ms: game_frame,
            latency_p50_ms: p(0.5),
            read: self.read,
            unreadable: self.unreadable,
            absent: self.absent,
            parts: self.parts.iter().zip(PART_NAMES_EN).filter(|((c, _), _)| *c > 0).map(|(&(c, _), name)| (name, c as f64 / hz / elapsed)).collect(),
        };
        let watch = std::mem::take(&mut self.watch);
        *self = Self {
            cpu_at: cpu,
            ticks_at: ticks,
            handler_at: handler,
            ..Self::with(watch, threads)
        };
        Some(report)
    }
}
