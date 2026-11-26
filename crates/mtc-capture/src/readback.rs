use windows::Win32::Graphics::Direct3D11::{
    D3D11_BOX, D3D11_CPU_ACCESS_READ, D3D11_MAP_READ, D3D11_MAPPED_SUBRESOURCE,
    D3D11_TEXTURE2D_DESC, D3D11_USAGE_STAGING, ID3D11Device, ID3D11Texture2D,
};
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT, DXGI_SAMPLE_DESC};
use windows::Win32::Graphics::Dxgi::IDXGIDevice3;
use windows::core::Interface;
use windows_capture::frame::Frame;

/// Copies frame textures to CPU memory. Staging textures are reused: a rectangle goes into the
/// smallest free one that holds it (sizes rounded up to `BUCKET`), and one left unused for
/// `IDLE_DROP` is released, so rectangles of drifting sizes (the readout's columns, a resized
/// window) do not each keep a texture of driver memory.
#[derive(Default)]
pub struct Readback {
    staging: Option<Staging>,
    regions: Vec<Staging>,
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

    /// Copies the whole frame into `out` as tightly packed 4-byte pixels in the capture's colour
    /// format, row by row from the top.
    pub fn read_full(&mut self, frame: &Frame, out: &mut Vec<u8>) -> windows::core::Result<()> {
        let desc = *frame.desc();
        let texture = self.staging(frame.device(), desc.Width, desc.Height, desc.Format)?;
        let context = frame.device_context();
        let row = desc.Width as usize * 4;
        out.resize(row * desc.Height as usize, 0);
        unsafe {
            context.CopyResource(texture, frame.as_raw_texture());
            let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
            context.Map(texture, 0, D3D11_MAP_READ, 0, Some(&mut mapped))?;
            let source = mapped.pData.cast::<u8>();
            let pitch = mapped.RowPitch as usize;
            for y in 0..desc.Height as usize {
                std::ptr::copy_nonoverlapping(
                    source.add(y * pitch),
                    out.as_mut_ptr().add(y * row),
                    row,
                );
            }
            context.Unmap(texture, 0);
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
        (all.clone().count(), all.map(|s| s.width as usize * s.height as usize * 4).sum())
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
            let staging = &self.regions[index];
            let out = &mut outs[k];
            let row = out.width as usize * 4;
            out.data.resize(row * out.height as usize, 0);
            unsafe {
                let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
                context.Map(&staging.texture, 0, D3D11_MAP_READ, 0, Some(&mut mapped))?;
                let source = mapped.pData.cast::<u8>();
                let pitch = mapped.RowPitch as usize;
                for r in 0..out.height as usize {
                    std::ptr::copy_nonoverlapping(source.add(r * pitch), out.data.as_mut_ptr().add(r * row), row);
                }
                context.Unmap(&staging.texture, 0);
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
