//! Development: time `layout::remap` on the loading region at a 125 % 1920x840 configuration.
use mtc_vision::FrameView;
use mtc_vision::layout::{Placement, UiLayout};

fn main() {
    let layout = UiLayout { width: 1920, height: 840, dpi: 120 };
    let placement = Placement::new((0, 34), layout);
    let (vx, vy) = Placement::VIRTUAL_ORIGIN;
    let (x, y, w, h) = mtc_vision::loading::LOADING_REGION;
    let rect = (x + vx, y + vy, w, h);
    let (sx, sy, sw, sh) = placement.bottom_source(rect, (1920, 900));
    let data: Vec<u8> = (0..sw * sh * 4).map(|i| (i * 7 % 251) as u8).collect();
    let view = FrameView { data: &data, format: mtc_vision::PixelFormat::Bgra, origin: (sx, sy), width: sw as u32, height: sh as u32, stride: sw as usize * 4 };
    let n = 200;
    let t = std::time::Instant::now();
    let mut sum = 0u64;
    for _ in 0..n {
        let r = placement.bottom_remap(&view, rect);
        sum += u64::from(r.data[r.data.len() / 2]);
    }
    println!("source {sw}x{sh}, out {w}x{h}: {:.2} ms per remap ({sum})", t.elapsed().as_secs_f64() * 1000.0 / f64::from(n));
    // The three map views at 100 %, 1920 x 1080.
    let placement = Placement::new((0, 34), UiLayout { width: 1920, height: 1080, dpi: 96 });
    for view in mtc_vision::map::geometry::MapView::ALL {
        let rect = view.frame_rect(Placement::VIRTUAL_ORIGIN);
        let (sx, sy, sw, sh) = placement.map_source(view, rect, (1920, 1114));
        let data: Vec<u8> = (0..sw * sh * 4).map(|i| (i * 7 % 251) as u8).collect();
        let src = FrameView { data: &data, format: mtc_vision::PixelFormat::Bgra, origin: (sx, sy), width: sw as u32, height: sh as u32, stride: sw as usize * 4 };
        let t = std::time::Instant::now();
        for _ in 0..n {
            let r = placement.map_remap(view, &src, rect);
            sum += u64::from(r.data[r.data.len() / 2]);
        }
        println!("{view:?} 1920x1080 100 %: source {sw}x{sh}, out {}x{}: {:.2} ms per remap ({sum})", rect.2, rect.3, t.elapsed().as_secs_f64() * 1000.0 / f64::from(n));
    }
    // The seat menu at 100 %, 1920 x 1080.
    let client = placement.seats_client((0, 34, 1920, 1080));
    let (rx, ry, rw, rh) = mtc_vision::seats::SeatReader::region(client);
    let rect = (rx, ry, rw as i32, rh as i32);
    let (sx, sy, sw, sh) = placement.seats_source(rect, (1920, 1114));
    let data: Vec<u8> = (0..sw * sh * 4).map(|i| (i * 7 % 251) as u8).collect();
    let src = FrameView { data: &data, format: mtc_vision::PixelFormat::Bgra, origin: (sx, sy), width: sw as u32, height: sh as u32, stride: sw as usize * 4 };
    let t = std::time::Instant::now();
    for _ in 0..n {
        let r = placement.seats_remap(&src, rect);
        sum += u64::from(r.data[r.data.len() / 2]);
    }
    println!("seats 1920x1080 100 %: source {sw}x{sh}, out {rw}x{rh}: {:.2} ms per remap ({sum})", t.elapsed().as_secs_f64() * 1000.0 / f64::from(n));
}
