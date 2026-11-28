use windows::Win32::Graphics::Direct3D11::{
    D3D11_BOX, D3D11_CPU_ACCESS_READ, D3D11_MAP_READ, D3D11_MAPPED_SUBRESOURCE,
    D3D11_TEXTURE2D_DESC, D3D11_USAGE_STAGING, ID3D11Device, ID3D11Texture2D,
};
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT, DXGI_FORMAT_R16G16B16A16_FLOAT, DXGI_SAMPLE_DESC};
use windows::Win32::Graphics::Dxgi::IDXGIDevice3;
use windows::core::Interface;
use windows_capture::frame::Frame;

/// Copies frame textures to CPU memory. Staging textures are reused: a rectangle goes into the
/// smallest free one that holds it (sizes rounded up to `BUCKET`), and one left unused for
/// `IDLE_DROP` is released, so rectangles of drifting sizes (the readout's columns, a resized
/// window) do not each keep a texture of driver memory.
///
/// Frames captured in 16-bit floating point (with Windows HDR on) come out as the same 8-bit
/// B, G, R, A pixels as an 8-bit capture: each channel divided by the white level of ordinary
/// content (`set_white`) and sRGB-encoded. An 8-bit capture of an HDR display clips everything
/// brighter than its white, which the 16-bit values keep.
#[derive(Default)]
pub struct Readback {
    staging: Option<Staging>,
    regions: Vec<Staging>,
    /// Ordinary content's white in 16-bit frames (1.0 = 80 nits); `None` is 1.
    white: Option<f32>,
    /// The 16-bit-to-8-bit table for that white.
    table: Option<(f32, Box<[u8]>)>,
    /// The brightest channel value (16-bit float bits) in 16-bit frames read since `take_peak`.
    peak: u16,
}

/// Region staging textures are made in multiples of this many pixels a side.
const BUCKET: u32 = 64;
/// A rectangle reuses a staging texture at most this many times its rounded area.
const REUSE_AREA: u64 = 2;
/// A region staging texture unused this long is released.
const IDLE_DROP: std::time::Duration = std::time::Duration::from_secs(10);

/// A rectangle of frame pixels read back from the GPU, 4 bytes per pixel, tightly packed.
#[derive(Clone, Debug, Default)]
pub struct Region {
    /// Frame coordinates of the first pixel.
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>,
}

struct Staging {
    texture: ID3D11Texture2D,
    width: u32,
    height: u32,
    format: DXGI_FORMAT,
    used: std::time::Instant,
}

impl Staging {
    fn new(device: &ID3D11Device, width: u32, height: u32, format: DXGI_FORMAT) -> windows::core::Result<Self> {
        Ok(Self { texture: create_staging(device, width, height, format)?, width, height, format, used: std::time::Instant::now() })
    }
}

impl Readback {
    pub fn new() -> Self {
        Self::default()
    }

    /// The white level of ordinary (SDR) content in 16-bit frames, in units of 80 nits (the
    /// display's SDR white level over 80 with HDR on).
    pub fn set_white(&mut self, white: f32) {
        self.white = Some(white);
    }

    pub fn white(&self) -> f32 {
        self.white.unwrap_or(1.0)
    }

    /// The brightest channel value read from 16-bit frames since the last call (80 nits = 1; 0
    /// when none was read): ordinary content's white as it measures on screen, where white text
    /// shows.
    pub fn take_peak(&mut self) -> f32 {
        half_to_f32(std::mem::take(&mut self.peak))
    }

    /// Copies `rows` rows of `width` pixels from mapped memory (`pitch` bytes a row) to `out` as
    /// 8-bit B, G, R, A.
    ///
    /// # Safety
    /// `source` points at `rows` rows of `pitch` bytes, each holding `width` pixels of `format`.
    unsafe fn copy_rows(&mut self, format: DXGI_FORMAT, source: *const u8, pitch: usize, width: usize, rows: usize, out: &mut [u8]) {
        let row = width * 4;
        if format != DXGI_FORMAT_R16G16B16A16_FLOAT {
            for r in 0..rows {
                unsafe { std::ptr::copy_nonoverlapping(source.add(r * pitch), out.as_mut_ptr().add(r * row), row) };
            }
            return;
        }
        let white = self.white();
        if self.table.as_ref().is_none_or(|(w, _)| *w != white) {
            self.table = Some((white, hdr_table(white)));
        }
        let table = &self.table.as_ref().expect("table made").1;
        let mut peak = self.peak;
        for r in 0..rows {
            let src = unsafe { std::slice::from_raw_parts(source.add(r * pitch).cast::<u16>(), width * 4) };
            let dst = &mut out[r * row..(r + 1) * row];
            for (px, out) in src.chunks_exact(4).zip(dst.chunks_exact_mut(4)) {
                // Non-negative halves order like their bits; negative ones (sign bit set) are left out.
                for &c in &px[..3] {
                    if c < 0x8000 && c > peak {
                        peak = c;
                    }
                }
                out[0] = table[usize::from(px[2])];
                out[1] = table[usize::from(px[1])];
                out[2] = table[usize::from(px[0])];
                out[3] = 255;
            }
        }
        self.peak = peak;
    }

    /// Copies the whole frame into `out` as tightly packed 4-byte B, G, R, A pixels, row by row
    /// from the top.
    pub fn read_full(&mut self, frame: &Frame, out: &mut Vec<u8>) -> windows::core::Result<()> {
        let desc = *frame.desc();
        let texture = self.staging(frame.device(), desc.Width, desc.Height, desc.Format)?.clone();
        let context = frame.device_context();
        out.resize(desc.Width as usize * 4 * desc.Height as usize, 0);
        unsafe {
            context.CopyResource(&texture, frame.as_raw_texture());
            let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
            context.Map(&texture, 0, D3D11_MAP_READ, 0, Some(&mut mapped))?;
            self.copy_rows(desc.Format, mapped.pData.cast::<u8>(), mapped.RowPitch as usize, desc.Width as usize, desc.Height as usize, out);
            context.Unmap(&texture, 0);
        }
        Ok(())
    }

    /// Copies one rectangle of the frame (clipped to it) into `out`. Only that rectangle crosses
    /// from GPU to CPU memory.
    pub fn read_region(
        &mut self,
        frame: &Frame,
        x: i32,
        y: i32,
        width: u32,
        height: u32,
        out: &mut Region,
    ) -> windows::core::Result<()> {
        self.read_regions(frame, &[(x, y, width, height)], std::slice::from_mut(out))
    }

    /// Region staging textures held and their bytes (driver memory, for the memory report).
    pub fn staging_bytes(&self) -> (usize, usize) {
        let all = self.regions.iter().chain(&self.staging);
        (all.clone().count(), all.map(|s| s.width as usize * s.height as usize * bytes_per_pixel(s.format)).sum())
    }

    /// Copies several rectangles (each clipped to the frame) into `outs`, one per rectangle. All
    /// copies are queued before the first map, so the GPU round trip is paid once.
    pub fn read_regions(&mut self, frame: &Frame, rects: &[(i32, i32, u32, u32)], outs: &mut [Region]) -> windows::core::Result<()> {
        let desc = *frame.desc();
        let context = frame.device_context();
        let mut queued = Vec::with_capacity(rects.len());
        for (k, &(x, y, width, height)) in rects.iter().enumerate() {
            let x0 = x.clamp(0, desc.Width as i32) as u32;
            let y0 = y.clamp(0, desc.Height as i32) as u32;
            let x1 = (x + width as i32).clamp(0, desc.Width as i32) as u32;
            let y1 = (y + height as i32).clamp(0, desc.Height as i32) as u32;
            let (w, h) = (x1.saturating_sub(x0), y1.saturating_sub(y0));
            let out = &mut outs[k];
            out.x = x0 as i32;
            out.y = y0 as i32;
            out.width = w;
            out.height = h;
            if w == 0 || h == 0 {
                out.data.clear();
                continue;
            }
            // The smallest staging texture that holds the rectangle, is at most `REUSE_AREA` times
            // its rounded size (a minimap read keeping the full map's texture in use would never
            // let it go) and holds no other rectangle of this call.
            let (bw, bh) = (w.div_ceil(BUCKET) * BUCKET, h.div_ceil(BUCKET) * BUCKET);
            let area = |w: u32, h: u32| u64::from(w) * u64::from(h);
            let index = match self
                .regions
                .iter()
                .enumerate()
                .filter(|(i, s)| {
                    s.width >= w && s.height >= h && area(s.width, s.height) <= REUSE_AREA * area(bw, bh) && s.format == desc.Format && !queued.iter().any(|&(_, j)| j == *i)
                })
                .min_by_key(|(_, s)| area(s.width, s.height))
            {
                Some((i, _)) => i,
                None => {
                    self.regions.push(Staging::new(frame.device(), bw, bh, desc.Format)?);
                    self.regions.len() - 1
                }
            };
            self.regions[index].used = std::time::Instant::now();
            let source_box = D3D11_BOX {
                left: x0,
                top: y0,
                front: 0,
                right: x1,
                bottom: y1,
                back: 1,
            };
            unsafe {
                context.CopySubresourceRegion(&self.regions[index].texture, 0, 0, 0, 0, frame.as_raw_texture(), 0, Some(&source_box));
            }
            queued.push((k, index));
        }
        for (k, index) in queued {
            let texture = self.regions[index].texture.clone();
            let out = &mut outs[k];
            let (width, height) = (out.width as usize, out.height as usize);
            out.data.resize(width * 4 * height, 0);
            unsafe {
                let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
                context.Map(&texture, 0, D3D11_MAP_READ, 0, Some(&mut mapped))?;
                self.copy_rows(desc.Format, mapped.pData.cast::<u8>(), mapped.RowPitch as usize, width, height, &mut out.data);
                context.Unmap(&texture, 0);
            }
        }
        let held = self.regions.len();
        self.regions.retain(|s| s.used.elapsed() < IDLE_DROP);
        if self.regions.len() < held {
            // The driver keeps a released texture's memory pooled for the next one of its size
            // until the device is trimmed (`memory_probe`: a V1-sized one stayed 7.3 MB of working
            // set). Nothing here or in the capture library depends on bound pipeline state.
            unsafe {
                context.ClearState();
                context.Flush();
                if let Ok(device) = frame.device().cast::<IDXGIDevice3>() {
                    device.Trim();
                }
            }
        }
        Ok(())
    }

    fn staging(
        &mut self,
        device: &ID3D11Device,
        width: u32,
        height: u32,
        format: DXGI_FORMAT,
    ) -> windows::core::Result<&ID3D11Texture2D> {
        let reusable = self
            .staging
            .as_ref()
            .is_some_and(|s| s.width == width && s.height == height && s.format == format);
        if !reusable {
            self.staging = Some(Staging::new(device, width, height, format)?);
        }
        Ok(&self.staging.as_ref().expect("staging texture exists").texture)
    }
}

fn bytes_per_pixel(format: DXGI_FORMAT) -> usize {
    if format == DXGI_FORMAT_R16G16B16A16_FLOAT { 8 } else { 4 }
}

/// A 16-bit float (IEEE half) as `f32`.
pub fn half_to_f32(bits: u16) -> f32 {
    let sign = if bits & 0x8000 != 0 { -1.0 } else { 1.0 };
    let exponent = i32::from((bits >> 10) & 0x1f);
    let mantissa = f32::from(bits & 0x3ff);
    match exponent {
        0 => sign * mantissa * 2f32.powi(-24),
        31 if mantissa == 0.0 => sign * f32::INFINITY,
        31 => f32::NAN,
        e => sign * (1.0 + mantissa / 1024.0) * 2f32.powi(e - 15),
    }
}

/// The nearest 16-bit float (IEEE half) to `value`, for tests and simulated frames.
pub fn f32_to_half(value: f32) -> u16 {
    let sign = ((value.to_bits() >> 16) & 0x8000) as u16;
    let abs = value.abs();
    if abs.is_nan() {
        return sign | 0x7e00;
    }
    if abs >= 65520.0 {
        return sign | 0x7c00;
    }
    if abs < 2f32.powi(-14) {
        // Subnormal: steps of 2^-24.
        return sign | (abs / 2f32.powi(-24)).round() as u16;
    }
    let mut exponent = abs.log2().floor() as i32;
    let mut mantissa = ((abs / 2f32.powi(exponent) - 1.0) * 1024.0).round() as u32;
    if mantissa == 1024 {
        mantissa = 0;
        exponent += 1;
    }
    sign | (((exponent + 15) as u16) << 10) | mantissa as u16
}

/// The sRGB encoding of a linear value in 0..1 (clamped), in 8 bits.
pub fn srgb_encode(linear: f32) -> u8 {
    let l = if linear.is_nan() { 0.0 } else { linear.clamp(0.0, 1.0) };
    let v = if l <= 0.003_130_8 { 12.92 * l } else { 1.055 * l.powf(1.0 / 2.4) - 0.055 };
    (v * 255.0).round() as u8
}

/// The linear value (0..1) of an 8-bit sRGB value.
pub fn srgb_decode(value: u8) -> f32 {
    let v = f32::from(value) / 255.0;
    if v <= 0.040_45 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
}

/// Every 16-bit float channel value's 8-bit sRGB value with ordinary content's white at `white`.
/// Windows composes an HDR desktop in linear light where 1.0 is 80 nits and puts SDR content,
/// linearised with the sRGB curve, at the SDR white level.
fn hdr_table(white: f32) -> Box<[u8]> {
    (0..=u16::MAX).map(|bits| srgb_encode(half_to_f32(bits) / white)).collect()
}

fn create_staging(
    device: &ID3D11Device,
    width: u32,
    height: u32,
    format: DXGI_FORMAT,
) -> windows::core::Result<ID3D11Texture2D> {
    let desc = D3D11_TEXTURE2D_DESC {
        Width: width,
        Height: height,
        MipLevels: 1,
        ArraySize: 1,
        Format: format,
        SampleDesc: DXGI_SAMPLE_DESC {
            Count: 1,
            Quality: 0,
        },
        Usage: D3D11_USAGE_STAGING,
        BindFlags: 0,
        CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
        MiscFlags: 0,
    };
    let mut texture = None;
    unsafe { device.CreateTexture2D(&desc, None, Some(&mut texture))? };
    Ok(texture.expect("CreateTexture2D returned a texture"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Halves convert both ways exactly for the values a frame holds.
    #[test]
    fn halves_round_trip() {
        for bits in [0u16, 1, 0x03ff, 0x0400, 0x3c00, 0x3555, 0x4300, 0x7bff] {
            assert_eq!(f32_to_half(half_to_f32(bits)), bits, "{bits:#06x}");
        }
        assert_eq!(half_to_f32(0x3c00), 1.0);
        assert_eq!(half_to_f32(0x4300), 3.5);
    }

    /// An 8-bit value lifted to an HDR desktop's linear light at its white and stored as a half
    /// comes back as itself, at the white levels seen on players' displays (178, 280, 480 nits).
    #[test]
    fn sdr_values_survive_an_hdr_desktop() {
        for white in [1.0, 178.0 / 80.0, 3.5, 6.0] {
            // The white as given, and as measured on screen (a full channel's half).
            for used in [white, half_to_f32(f32_to_half(white))] {
                let table = hdr_table(used);
                for v in 0..=255u8 {
                    let half = f32_to_half(srgb_decode(v) * white);
                    assert_eq!(table[usize::from(half)], v, "white {white} taken as {used}, value {v}");
                }
            }
        }
    }

    /// The cost of bringing a 16-bit frame back to 8 bits (a measurement, not a check):
    /// `cargo test --release -p mtc-capture conversion_cost -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn conversion_cost() {
        let (w, h) = (1920usize, 1080usize);
        let source: Vec<u16> = (0..w * h * 4).map(|i| f32_to_half(srgb_decode((i % 251) as u8) * 3.5)).collect();
        let mut out = vec![0u8; w * h * 4];
        let mut readback = Readback::new();
        readback.set_white(3.5);
        let runs = 20;
        let start = std::time::Instant::now();
        for _ in 0..runs {
            unsafe { readback.copy_rows(DXGI_FORMAT_R16G16B16A16_FLOAT, source.as_ptr().cast(), w * 8, w, h, &mut out) };
        }
        let converted = start.elapsed().as_nanos() as f64 / (runs * w * h) as f64;
        let eight = vec![7u8; w * h * 4];
        let start = std::time::Instant::now();
        for _ in 0..runs {
            unsafe { readback.copy_rows(DXGI_FORMAT::default(), eight.as_ptr(), w * 4, w, h, &mut out) };
        }
        let copied = start.elapsed().as_nanos() as f64 / (runs * w * h) as f64;
        println!("16-bit brought back to 8-bit: {converted:.2} ns a pixel; an 8-bit copy: {copied:.2} ns a pixel");
    }

    /// What an 8-bit capture of an HDR desktop gives (everything above white clipped): white text
    /// on grey turns white on white.
    #[test]
    fn an_8_bit_capture_of_an_hdr_desktop_clips() {
        let clipped = |v: u8| srgb_encode(srgb_decode(v) * 3.5);
        assert_eq!((clipped(255), clipped(180), clipped(128), clipped(60)), (255, 255, 225, 111));
    }
}
