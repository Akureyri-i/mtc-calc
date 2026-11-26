//! Checks the map marks window on screen: a selected marker shown through the overlay (drawn on a
//! DirectComposition surface, `Surface::layered`) must land where it is asked for, standing still
//! and while a drag is followed with the cursor at rest. Captures the composed desktop with GDI
//! and finds the red diamond's centre.
//!   cargo run --release -p mtc-overlay --example layered_check
//!
//! Shows a red diamond on the desktop for about two seconds. Do not move the mouse meanwhile.

use std::time::{Duration, Instant};

use mtc_overlay::{MapMark, MapMarks, Overlay, PanFollow};
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BitBlt, CAPTUREBLT, CreateCompatibleBitmap, CreateCompatibleDC, DIB_RGB_COLORS, DeleteDC, DeleteObject,
    GetDC, GetDIBits, ReleaseDC, SRCCOPY, SelectObject,
};
use windows::Win32::UI::HiDpi::{DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext};

/// Screen pixels of `(x, y, w, h)` as RGB.
fn capture(x: i32, y: i32, w: i32, h: i32) -> Vec<[u8; 3]> {
    unsafe {
        let screen = GetDC(None);
        let dc = CreateCompatibleDC(Some(screen));
        let bitmap = CreateCompatibleBitmap(screen, w, h);
        let old = SelectObject(dc, bitmap.into());
        BitBlt(dc, 0, 0, w, h, Some(screen), x, y, SRCCOPY | CAPTUREBLT).expect("bitblt");
        let mut info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER { biSize: size_of::<BITMAPINFOHEADER>() as u32, biWidth: w, biHeight: -h, biPlanes: 1, biBitCount: 32, biCompression: BI_RGB.0, ..Default::default() },
            ..Default::default()
        };
        let mut bgra = vec![0u8; (w * h * 4) as usize];
        GetDIBits(dc, bitmap, 0, h as u32, Some(bgra.as_mut_ptr().cast()), &mut info, DIB_RGB_COLORS);
        SelectObject(dc, old);
        let _ = DeleteObject(bitmap.into());
        let _ = DeleteDC(dc);
        ReleaseDC(None, screen);
        bgra.chunks_exact(4).map(|p| [p[2], p[1], p[0]]).collect()
    }
}

/// Centre (screen px, pixel centres) of the red diamond's pixels in the box around `(x, y)`.
fn red_centre(x: i32, y: i32) -> Option<(f64, f64, usize)> {
    let (bx, by, side) = (x - 40, y - 40, 80);
    let pixels = capture(bx, by, side, side);
    let (mut sx, mut sy, mut n) = (0.0, 0.0, 0usize);
    for (i, p) in pixels.iter().enumerate() {
        let (r, g, b) = (i32::from(p[0]), i32::from(p[1]), i32::from(p[2]));
        if r > 180 && g < 110 && b < 100 && r - g > 100 {
            sx += (i as i32 % side) as f64 + 0.5;
            sy += (i as i32 / side) as f64 + 0.5;
            n += 1;
        }
    }
    (n > 0).then(|| (f64::from(bx) + sx / n as f64, f64::from(by) + sy / n as f64, n))
}

fn main() {
    let _ = unsafe { SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
    let overlay = Overlay::spawn(std::path::PathBuf::from("no-fonts"));
    let point = (700.0f32, 500.0f32);
    let mark = |label: &str, x: f32, y: f32, selected: bool| MapMark { label: label.into(), x, y, selected, objective: false, precision_m: None, covered: false };
    let marks = MapMarks {
        left: 300,
        top: 200,
        size: 882,
        small: false,
        marks: vec![mark("A1", point.0, point.1, true), mark("A2", 900.0, 650.0, false)],
        coarse_m: 10.0,
        blast: None,
        planning: None,
        follow: None,
        pred: None,
        scale: 1.0,
    };
    // The selected diamond is centred on the game's (the marker point's pixel centroid offset).
    let expected = (f64::from(point.0) - 1.1, f64::from(point.1) - 0.7);
    overlay.show_map(Some(marks.clone()));
    std::thread::sleep(Duration::from_millis(700));
    let still = red_centre(point.0 as i32, point.1 as i32);
    let now = Instant::now();
    overlay.show_map(Some(MapMarks { follow: Some(PanFollow { at: now, limits: (-5000.0, -5000.0, 5000.0, 5000.0), press: (0.0, 0.0), pressed_at: now }), ..marks.clone() }));
    std::thread::sleep(Duration::from_millis(500));
    let following = red_centre(point.0 as i32, point.1 as i32);
    overlay.show_map(None);
    std::thread::sleep(Duration::from_millis(100));
    for (name, found) in [("standing still", still), ("following, cursor at rest", following)] {
        match found {
            Some((x, y, n)) => println!("{name}: red diamond centre ({x:.2}, {y:.2}), {n} px; expected ({:.2}, {:.2}); off by ({:+.2}, {:+.2})", expected.0, expected.1, x - expected.0, y - expected.1),
            None => println!("{name}: no red diamond found"),
        }
    }
}
