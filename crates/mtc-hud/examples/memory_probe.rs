//! Memory of each part the HUD holds, loaded one after another: working set, private bytes, the
//! program's own heap and the process's video memory after each step (the HUD's stats report the
//! working set; the budget is 150 MB). The graphics steps tell the driver's share apart: one
//! Direct3D 11 device, a second one (the capture library makes its own), the overlay.
//!   cargo run --release -p mtc-hud --example memory_probe [map-id]
use std::time::Duration;

use mtc_vision::map::zoom::V1Registrar;
use mtc_vision::map::{MapIdentifier, MapReference, Objective};
use windows::Win32::System::ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS, PROCESS_MEMORY_COUNTERS_EX};
use windows::Win32::System::Threading::GetCurrentProcess;

/// The program's own heap (bytes), counted as the HUD counts it.
static HEAP: std::sync::atomic::AtomicIsize = std::sync::atomic::AtomicIsize::new(0);
struct Counting;
// SAFETY: forwards to the system allocator and only counts sizes.
unsafe impl std::alloc::GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: std::alloc::Layout) -> *mut u8 {
        HEAP.fetch_add(l.size() as isize, std::sync::atomic::Ordering::Relaxed);
        unsafe { std::alloc::System.alloc(l) }
    }
    unsafe fn dealloc(&self, p: *mut u8, l: std::alloc::Layout) {
        HEAP.fetch_sub(l.size() as isize, std::sync::atomic::Ordering::Relaxed);
        unsafe { std::alloc::System.dealloc(p, l) }
    }
    unsafe fn realloc(&self, p: *mut u8, l: std::alloc::Layout, n: usize) -> *mut u8 {
        HEAP.fetch_add(n as isize - l.size() as isize, std::sync::atomic::Ordering::Relaxed);
        unsafe { std::alloc::System.realloc(p, l, n) }
    }
}
#[global_allocator]
static A: Counting = Counting;

fn heap_mb() -> f64 {
    HEAP.load(std::sync::atomic::Ordering::Relaxed).max(0) as f64 / 1e6
}

/// Local and shared video memory of this process on the first adapter (MB).
fn video_mb() -> (f64, f64) {
    use windows::Win32::Graphics::Dxgi::{CreateDXGIFactory1, DXGI_MEMORY_SEGMENT_GROUP_LOCAL, DXGI_MEMORY_SEGMENT_GROUP_NON_LOCAL, DXGI_QUERY_VIDEO_MEMORY_INFO, IDXGIAdapter3, IDXGIFactory1};
    use windows::core::Interface;
    unsafe {
        let Ok(factory) = CreateDXGIFactory1::<IDXGIFactory1>() else { return (0.0, 0.0) };
        let Ok(adapter) = factory.EnumAdapters1(0).and_then(|a| a.cast::<IDXGIAdapter3>()) else { return (0.0, 0.0) };
        let q = |g| {
            let mut i = DXGI_QUERY_VIDEO_MEMORY_INFO::default();
            adapter.QueryVideoMemoryInfo(0, g, &mut i).map_or(0.0, |()| i.CurrentUsage as f64 / 1e6)
        };
        (q(DXGI_MEMORY_SEGMENT_GROUP_LOCAL), q(DXGI_MEMORY_SEGMENT_GROUP_NON_LOCAL))
    }
}

/// A Direct3D 11 hardware device, as the overlay and the capture library each make one.
fn d3d_device() -> windows::Win32::Graphics::Direct3D11::ID3D11Device {
    use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_HARDWARE;
    use windows::Win32::Graphics::Direct3D11::{D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_SDK_VERSION, D3D11CreateDevice};
    let mut device = None;
    unsafe { D3D11CreateDevice(None, D3D_DRIVER_TYPE_HARDWARE, Default::default(), D3D11_CREATE_DEVICE_BGRA_SUPPORT, None, D3D11_SDK_VERSION, Some(&mut device), None, None) }.expect("D3D11 device");
    device.expect("device")
}

/// A CPU-readable staging texture, mapped and every page of it read.
fn staging_read(device: &windows::Win32::Graphics::Direct3D11::ID3D11Device, width: u32, height: u32) -> windows::Win32::Graphics::Direct3D11::ID3D11Texture2D {
    use windows::Win32::Graphics::Direct3D11::{D3D11_CPU_ACCESS_READ, D3D11_MAP_READ, D3D11_MAPPED_SUBRESOURCE, D3D11_TEXTURE2D_DESC, D3D11_USAGE_STAGING};
    use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC};
    let desc = D3D11_TEXTURE2D_DESC {
        Width: width,
        Height: height,
        MipLevels: 1,
        ArraySize: 1,
        Format: DXGI_FORMAT_B8G8R8A8_UNORM,
        SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
        Usage: D3D11_USAGE_STAGING,
        CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
        ..Default::default()
    };
    let mut texture = None;
    unsafe { device.CreateTexture2D(&desc, None, Some(&mut texture)) }.expect("staging texture");
    let texture = texture.expect("texture");
    let context = unsafe { device.GetImmediateContext() }.expect("context");
    let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
    unsafe { context.Map(&texture, 0, D3D11_MAP_READ, 0, Some(&mut mapped)) }.expect("map");
    let mut sum = 0u64;
    for k in (0..mapped.RowPitch as usize * height as usize).step_by(4096) {
        sum += u64::from(unsafe { *mapped.pData.cast::<u8>().add(k) });
    }
    std::hint::black_box(sum);
    unsafe { context.Unmap(&texture, 0) };
    texture
}

fn memory() -> (f64, f64) {
    let mut c = PROCESS_MEMORY_COUNTERS_EX { cb: size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32, ..Default::default() };
    unsafe {
        let _ = GetProcessMemoryInfo(GetCurrentProcess(), (&raw mut c).cast::<PROCESS_MEMORY_COUNTERS>(), c.cb);
    }
    (c.WorkingSetSize as f64 / 1e6, c.PrivateUsage as f64 / 1e6)
}

struct Steps {
    last: (f64, f64),
}

impl Steps {
    fn step(&mut self, name: &str) {
        let now = memory();
        let (heap, (local, shared)) = (heap_mb(), video_mb());
        println!("{name:<44} working set {:7.1} MB ({:+6.1})   private {:7.1} MB ({:+6.1})   heap {heap:6.1} MB   video {local:5.1} + {shared:5.1} MB", now.0, now.0 - self.last.0, now.1, now.1 - self.last.1);
        self.last = now;
    }
}

fn main() {
    let map_id = std::env::args().nth(1).unwrap_or_else(|| "fulvia_gap".to_owned());
    let mut s = Steps { last: memory() };
    s.step("start");

    let readers = (mtc_vision::aim::AimReader::new(), mtc_vision::map::CueReader::new(), mtc_vision::text::TextReader::new(), mtc_vision::map::markers::MarkerReader::new(), mtc_vision::bar::BarReader::new());
    s.step("readers (aim, cues, text, markers, bar)");

    let json = std::fs::read_to_string("reference/wiki/production-1.0.0/20261117/shells.json").expect("shells.json");
    let ammo = mtc_ammo::AmmoData::from_json(&json).expect("ammo data");
    drop(json);
    s.step("ammo data (shells.json parsed, text dropped)");

    let mut identifier = MapIdentifier::new(&[]);
    for info in mtc_maps::all() {
        if let Ok(img) = mtc_maps::load_reference(info) {
            let layout: Vec<Objective> = mtc_maps::objectives(&info.id).iter().map(|o| Objective { letter: o.letter.chars().next().expect("letter"), u: o.u, v: o.v }).collect();
            identifier.push(&MapReference::from_rgba(&info.id, info.size_m(), img.width, img.height, &img.data), &layout);
        }
    }
    s.step("map library (all references, thumbnails kept)");

    let info = mtc_maps::by_id(&map_id).expect("map id");
    let img = mtc_maps::load_reference(info).expect("reference");
    let reference = MapReference::from_rgba(&info.id, info.size_m(), img.width, img.height, &img.data);
    drop(img);
    let registrar = V1Registrar::new(&reference, info.grid_cell_m);
    s.step(&format!("V1 registrar ({map_id})"));
    // With a game-made reference the registrar holds a second one for the zoomed searches (the
    // same picture stands in for it here: the size is what counts).
    let registrar_game = V1Registrar::with_zoom_reference(&reference, &reference, info.grid_cell_m);
    drop(reference);
    s.step("V1 registrar with a zoomed reference (game-made)");

    let heights = mtc_maps::load_heightmap(info).expect("heightmap");
    s.step(&format!("heightmap ({map_id})"));

    let shells: Vec<&mtc_ammo::Shell> = ammo.shells("155mm M284").iter().collect();
    let traced: Vec<mtc_ballistics::Ballistics> = shells.iter().take(3).map(|sh| mtc_ballistics::Ballistics::new(sh.projectile())).collect();
    s.step("trajectories, 3 shells (one family)");
    let started = std::time::Instant::now();
    let more: Vec<mtc_ballistics::ReachTable> = shells.iter().skip(3).take(9).map(|sh| mtc_ballistics::ReachTable::new(&mtc_ballistics::Ballistics::new(sh.projectile()))).collect();
    let per = started.elapsed().as_secs_f64() * 1000.0 / more.len().max(1) as f64;
    s.step(&format!("reach tables, {} more ({per:.0} ms each)", more.len()));

    let device_a = d3d_device();
    s.step("one Direct3D 11 device");
    let device_b = d3d_device();
    s.step("a second Direct3D 11 device (capture's own)");

    // A readback staging texture the size of a V1 container (1344 x 1344 on a 2560 x 1368 game),
    // mapped and read once as the HUD reads one.
    let staging = staging_read(&device_b, 1344, 1344);
    s.step("a V1-sized staging texture, read once");
    drop(staging);
    s.step("  released");
    unsafe { device_b.GetImmediateContext().expect("context").Flush() };
    std::thread::sleep(Duration::from_millis(200));
    s.step("  released, context flushed");
    let again = staging_read(&device_b, 1344, 1344);
    s.step("  the same size made and read again");
    drop(again);
    let trim_started = std::time::Instant::now();
    unsafe {
        use windows::core::Interface;
        let context = device_b.GetImmediateContext().expect("context");
        context.ClearState();
        context.Flush();
        device_b.cast::<windows::Win32::Graphics::Dxgi::IDXGIDevice3>().expect("IDXGIDevice3").Trim();
    }
    let trim_ms = trim_started.elapsed().as_secs_f64() * 1000.0;
    std::thread::sleep(Duration::from_millis(200));
    s.step(&format!("  released, state cleared, trimmed ({trim_ms:.2} ms)"));

    let font_dir = std::path::PathBuf::from(std::env::var("LOCALAPPDATA").expect("LOCALAPPDATA")).join("Roblox/Versions");
    let fonts = std::fs::read_dir(font_dir)
        .expect("Roblox versions")
        .filter_map(Result::ok)
        .map(|e| e.path().join("content/fonts"))
        .find(|p| p.join("Montserrat-Medium.ttf").is_file())
        .expect("fonts");
    let overlay = mtc_overlay::Overlay::spawn(fonts);
    std::thread::sleep(Duration::from_millis(1500));
    s.step("overlay windows (strip, panel, map, charges, drag)");

    std::hint::black_box((&readers, &ammo, &identifier, &registrar, &registrar_game, &heights, &traced, &more, &overlay, &device_a, &device_b));
}
