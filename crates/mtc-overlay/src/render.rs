//! Direct3D 11 + DirectComposition swap chain with a Direct2D device context on top, or an
//! offscreen Direct2D bitmap read back to memory (to composite the HUD onto recorded frames).

use std::cell::{Cell, RefCell};
use std::path::Path;
use std::rc::Rc;

use windows::Win32::Foundation::{HWND, POINT};
use windows::Win32::Graphics::Direct2D::Common::{
    D2D_RECT_F, D2D_SIZE_U, D2D1_ALPHA_MODE_PREMULTIPLIED, D2D1_COLOR_F, D2D1_FIGURE_BEGIN_FILLED,
    D2D1_FIGURE_END_CLOSED, D2D1_FIGURE_END_OPEN, D2D1_PIXEL_FORMAT,
};
use windows::Win32::Graphics::Direct2D::{
    D2D1_BITMAP_OPTIONS_CANNOT_DRAW, D2D1_BITMAP_OPTIONS_CPU_READ, D2D1_BITMAP_OPTIONS_TARGET, D2D1_BITMAP_PROPERTIES1,
    D2D1_MAP_OPTIONS_READ, ID2D1Bitmap1,
    D2D1_DEVICE_CONTEXT_OPTIONS_NONE, D2D1_DRAW_TEXT_OPTIONS_NONE, D2D1_ELLIPSE,
    D2D1_FACTORY_TYPE_SINGLE_THREADED, D2D1CreateFactory, ID2D1Device, ID2D1DeviceContext, ID2D1Factory1,
    ID2D1SolidColorBrush,
};
use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_HARDWARE;
use windows::Win32::Graphics::Direct3D11::{
    D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_SDK_VERSION, D3D11CreateDevice, ID3D11Device,
};
use windows::Win32::Graphics::DirectComposition::{
    DCompositionCreateDevice, IDCompositionClip, IDCompositionDevice, IDCompositionSurface, IDCompositionTarget, IDCompositionVisual,
};
use windows::Win32::Graphics::DirectWrite::{
    DWRITE_FACTORY_TYPE_SHARED, DWRITE_FONT_STRETCH_NORMAL, DWRITE_FONT_STYLE_NORMAL, DWRITE_TRIMMING,
    DWRITE_TRIMMING_GRANULARITY_CHARACTER, DWRITE_WORD_WRAPPING_NO_WRAP,
    DWRITE_FONT_WEIGHT, DWRITE_MEASURING_MODE_NATURAL, DWRITE_PARAGRAPH_ALIGNMENT_CENTER,
    DWRITE_TEXT_ALIGNMENT, DWriteCreateFactory, IDWriteFactory5, IDWriteFontCollection,
    IDWriteFontCollection1, IDWriteRenderingParams, IDWriteTextFormat,
};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_ALPHA_MODE_PREMULTIPLIED, DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC,
};
use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory2, DXGI_CREATE_FACTORY_FLAGS, DXGI_PRESENT, DXGI_SCALING_STRETCH,
    DXGI_SWAP_CHAIN_DESC1, DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL, DXGI_USAGE_RENDER_TARGET_OUTPUT,
    IDXGIDevice, IDXGIFactory2, IDXGISurface, IDXGISwapChain1,
};
use windows::core::{HSTRING, Interface, PCWSTR};
use windows_numerics::{Matrix3x2, Vector2};

/// Vertical metrics of a text format (px): line box height, baseline from the line's top, the
/// font's capital height.
#[derive(Clone, Copy, Debug)]
pub struct CapMetrics {
    pub height: f32,
    pub baseline: f32,
    pub cap: f32,
}

/// The game's drop shadow under HUD text and marks: offset (px) and black opacity.
pub const SHADOW_OFFSET: (f32, f32) = (2.0, 2.0);
pub const SHADOW_ALPHA: f32 = 0.35;

pub fn rgba(r: u8, g: u8, b: u8, a: f32) -> D2D1_COLOR_F {
    D2D1_COLOR_F {
        r: f32::from(r) / 255.0,
        g: f32::from(g) / 255.0,
        b: f32::from(b) / 255.0,
        a,
    }
}

/// Objects tied to the graphics device, made again when it is lost.
struct Devices {
    d3d: ID3D11Device,
    dxgi_factory: IDXGIFactory2,
    d2d: ID2D1Device,
    dcomp: IDCompositionDevice,
}

impl Devices {
    fn new(factory: &ID2D1Factory1) -> windows::core::Result<Self> {
        unsafe {
            let mut device: Option<ID3D11Device> = None;
            D3D11CreateDevice(None, D3D_DRIVER_TYPE_HARDWARE, Default::default(), D3D11_CREATE_DEVICE_BGRA_SUPPORT, None, D3D11_SDK_VERSION, Some(&mut device), None, None)?;
            let d3d = device.expect("D3D11 device");
            let dxgi_device: IDXGIDevice = d3d.cast()?;
            Ok(Self {
                dxgi_factory: CreateDXGIFactory2(DXGI_CREATE_FACTORY_FLAGS(0))?,
                d2d: factory.CreateDevice(&dxgi_device)?,
                dcomp: DCompositionCreateDevice(&dxgi_device)?,
                d3d,
            })
        }
    }
}

/// What every surface on the overlay thread shares: one Direct3D 11 device, one Direct2D factory
/// and device, one DirectComposition device, one DirectWrite factory with the game's fonts loaded
/// once, and the text rendering parameters. Each surface keeps its own swap chain (or bitmap),
/// target and device context with its own settings, so what it draws is unchanged.
pub struct Graphics {
    factory: ID2D1Factory1,
    dwrite: IDWriteFactory5,
    fonts: Option<IDWriteFontCollection>,
    /// Rendering for strip labels and values, matched to the readout's (`text_params`).
    label_params: IDWriteRenderingParams,
    value_params: IDWriteRenderingParams,
    /// No grid fitting, anti-aliased both ways, no contrast boost: how the game draws its small UI
    /// text (fitted glyph by glyph on captured frames).
    unhinted_params: IDWriteRenderingParams,
    devices: RefCell<Devices>,
}

impl Graphics {
    pub fn new(font_dir: &Path) -> windows::core::Result<Rc<Self>> {
        unsafe {
            let factory: ID2D1Factory1 = D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)?;
            let dwrite: IDWriteFactory5 = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)?;
            let fonts = load_fonts(&dwrite, font_dir).ok().and_then(|c| c.cast::<IDWriteFontCollection>().ok());
            let (label_params, value_params) = text_params(&dwrite)?;
            let unhinted_params = {
                use windows::Win32::Graphics::DirectWrite::{DWRITE_GRID_FIT_MODE_DISABLED, DWRITE_PIXEL_GEOMETRY_FLAT, DWRITE_RENDERING_MODE1_NATURAL_SYMMETRIC};
                dwrite.CreateCustomRenderingParams(1.0, 0.0, 0.0, 0.0, DWRITE_PIXEL_GEOMETRY_FLAT, DWRITE_RENDERING_MODE1_NATURAL_SYMMETRIC, DWRITE_GRID_FIT_MODE_DISABLED)?.cast::<IDWriteRenderingParams>()?
            };
            let devices = RefCell::new(Devices::new(&factory)?);
            Ok(Rc::new(Self { factory, dwrite, fonts, label_params, value_params, unhinted_params, devices }))
        }
    }

    /// Whether the graphics device was lost (driver update or reset, adapter removed): the devices
    /// must be made again (`recover`) and every window surface rebuilt (`Surface::rebuild`).
    pub fn lost(&self) -> bool {
        unsafe { self.devices.borrow().d3d.GetDeviceRemovedReason().is_err() }
    }

    /// New devices. The factory, the fonts and the text parameters do not depend on the device.
    pub fn recover(&self) -> windows::core::Result<()> {
        let devices = Devices::new(&self.factory)?;
        *self.devices.borrow_mut() = devices;
        Ok(())
    }
}

/// Where a surface's drawing goes.
enum Output {
    Window {
        swap_chain: IDXGISwapChain1,
        _target: IDCompositionTarget,
        _visual: IDCompositionVisual,
        hwnd: HWND,
        width: u32,
        height: u32,
    },
    Offscreen {
        target: ID2D1Bitmap1,
        readback: ID2D1Bitmap1,
        width: u32,
        height: u32,
    },
    /// A window whose drawing goes on a DirectComposition surface on a visual that is moved
    /// (`set_pan`) and clipped (`set_clip`) without drawing again. A drawing and the visual's place
    /// reach the compositor in one `commit`, so it never shows one without the other (a swap chain's
    /// present and a visual change can land in different compositor frames).
    Layered {
        _target: IDCompositionTarget,
        root: IDCompositionVisual,
        content: IDCompositionVisual,
        surface: IDCompositionSurface,
        hwnd: HWND,
        size: (u32, u32),
        /// Window coordinates of the surface's top left at zero pan; drawing uses window
        /// coordinates.
        origin: (f32, f32),
    },
    /// A window surface whose composition target was let go, to be made again (`layered`: the
    /// layered surface's size and origin).
    Detached { hwnd: HWND, width: u32, height: u32, layered: Option<((u32, u32), (f32, f32))> },
}

/// One composited window surface, or an offscreen one.
pub struct Surface {
    context: ID2D1DeviceContext,
    output: Output,
    graphics: Rc<Graphics>,
    brush: ID2D1SolidColorBrush,
    /// Everything drawn between `begin` (or `start`) and `end` is scaled by this about `pivot`
    /// (surface px): the HUD drawn at the game's HUD scale.
    scale: Cell<f32>,
    pivot: Cell<(f32, f32)>,
}

/// How the readout's text is rasterised, matched on identical strings over a bright backdrop
/// (ink and fully covered pixels against the readout's): labels crisp — anti-aliased only
/// horizontally — values anti-aliased both ways, both with gamma 1.0 (the defaults made labels
/// blurrier and values ~10 % heavier than the readout's).
fn text_params(dwrite: &IDWriteFactory5) -> windows::core::Result<(IDWriteRenderingParams, IDWriteRenderingParams)> {
    use windows::Win32::Graphics::DirectWrite::{
        DWRITE_GRID_FIT_MODE_DEFAULT, DWRITE_PIXEL_GEOMETRY_FLAT, DWRITE_RENDERING_MODE1, DWRITE_RENDERING_MODE1_NATURAL,
        DWRITE_RENDERING_MODE1_NATURAL_SYMMETRIC,
    };
    let make = |mode: DWRITE_RENDERING_MODE1| unsafe {
        dwrite.CreateCustomRenderingParams(1.0, 0.5, 1.0, 0.0, DWRITE_PIXEL_GEOMETRY_FLAT, mode, DWRITE_GRID_FIT_MODE_DEFAULT)?.cast::<IDWriteRenderingParams>()
    };
    Ok((make(DWRITE_RENDERING_MODE1_NATURAL)?, make(DWRITE_RENDERING_MODE1_NATURAL_SYMMETRIC)?))
}

fn bitmap_properties(options: windows::Win32::Graphics::Direct2D::D2D1_BITMAP_OPTIONS) -> D2D1_BITMAP_PROPERTIES1 {
    D2D1_BITMAP_PROPERTIES1 {
        pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED },
        dpiX: 96.0,
        dpiY: 96.0,
        bitmapOptions: options,
        ..Default::default()
    }
}

impl Surface {
    /// A surface drawing into `width` × `height` premultiplied BGRA pixels in memory.
    pub fn offscreen(graphics: &Rc<Graphics>, width: u32, height: u32) -> windows::core::Result<Self> {
        unsafe {
            let context = graphics.devices.borrow().d2d.CreateDeviceContext(D2D1_DEVICE_CONTEXT_OPTIONS_NONE)?;
            context.SetDpi(96.0, 96.0);
            let size = D2D_SIZE_U { width, height };
            let target = context.CreateBitmap(size, None, 0, &bitmap_properties(D2D1_BITMAP_OPTIONS_TARGET))?;
            let readback = context.CreateBitmap(size, None, 0, &bitmap_properties(D2D1_BITMAP_OPTIONS_CPU_READ | D2D1_BITMAP_OPTIONS_CANNOT_DRAW))?;
            context.SetTarget(&target);
            let brush = context.CreateSolidColorBrush(&rgba(255, 255, 255, 1.0), None)?;
            Ok(Self { context, output: Output::Offscreen { target, readback, width, height }, graphics: graphics.clone(), brush, scale: Cell::new(1.0), pivot: Cell::new((0.0, 0.0)) })
        }
    }

    /// The offscreen drawing as premultiplied BGRA rows (empty for a window surface).
    pub fn pixels(&self) -> windows::core::Result<Vec<u8>> {
        let Output::Offscreen { target, readback, width, height } = &self.output else { return Ok(Vec::new()) };
        unsafe {
            readback.CopyFromBitmap(None, target, None)?;
            let mapped = readback.Map(D2D1_MAP_OPTIONS_READ)?;
            let row = *width as usize * 4;
            let mut out = Vec::with_capacity(row * *height as usize);
            for y in 0..*height as usize {
                out.extend_from_slice(std::slice::from_raw_parts(mapped.bits.add(y * mapped.pitch as usize), row));
            }
            readback.Unmap()?;
            Ok(out)
        }
    }

    pub fn new(graphics: &Rc<Graphics>, hwnd: HWND, width: u32, height: u32) -> windows::core::Result<Self> {
        let devices = graphics.devices.borrow();
        unsafe {
            let desc = DXGI_SWAP_CHAIN_DESC1 {
                Width: width,
                Height: height,
                Format: DXGI_FORMAT_B8G8R8A8_UNORM,
                SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
                BufferCount: 2,
                Scaling: DXGI_SCALING_STRETCH,
                SwapEffect: DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL,
                AlphaMode: DXGI_ALPHA_MODE_PREMULTIPLIED,
                ..Default::default()
            };
            let swap_chain = devices.dxgi_factory.CreateSwapChainForComposition(&devices.d3d, &desc, None)?;
            let context = devices.d2d.CreateDeviceContext(D2D1_DEVICE_CONTEXT_OPTIONS_NONE)?;
            context.SetDpi(96.0, 96.0);
            let surface: IDXGISurface = swap_chain.GetBuffer(0)?;
            let props = D2D1_BITMAP_PROPERTIES1 {
                pixelFormat: D2D1_PIXEL_FORMAT {
                    format: DXGI_FORMAT_B8G8R8A8_UNORM,
                    alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
                },
                dpiX: 96.0,
                dpiY: 96.0,
                bitmapOptions: D2D1_BITMAP_OPTIONS_TARGET | D2D1_BITMAP_OPTIONS_CANNOT_DRAW,
                ..Default::default()
            };
            let bitmap = context.CreateBitmapFromDxgiSurface(&surface, Some(&props))?;
            context.SetTarget(&bitmap);

            let target = devices.dcomp.CreateTargetForHwnd(hwnd, true)?;
            let visual = devices.dcomp.CreateVisual()?;
            visual.SetContent(&swap_chain)?;
            target.SetRoot(&visual)?;
            devices.dcomp.Commit()?;

            let brush = context.CreateSolidColorBrush(&rgba(255, 255, 255, 1.0), None)?;
            Ok(Self {
                context,
                output: Output::Window { swap_chain, _target: target, _visual: visual, hwnd, width, height },
                graphics: graphics.clone(),
                brush,
                scale: Cell::new(1.0),
                pivot: Cell::new((0.0, 0.0)),
            })
        }
    }

    /// A window surface drawing on a DirectComposition surface `size` big, whose top left sits at
    /// `origin` in the window at zero pan (see `Output::Layered`). Nothing shows before the first
    /// drawing and `commit`.
    pub fn layered(graphics: &Rc<Graphics>, hwnd: HWND, size: (u32, u32), origin: (f32, f32)) -> windows::core::Result<Self> {
        let devices = graphics.devices.borrow();
        unsafe {
            let context = devices.d2d.CreateDeviceContext(D2D1_DEVICE_CONTEXT_OPTIONS_NONE)?;
            context.SetDpi(96.0, 96.0);
            let surface = devices.dcomp.CreateSurface(size.0, size.1, DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_ALPHA_MODE_PREMULTIPLIED)?;
            let target = devices.dcomp.CreateTargetForHwnd(hwnd, true)?;
            let root = devices.dcomp.CreateVisual()?;
            let content = devices.dcomp.CreateVisual()?;
            content.SetContent(&surface)?;
            content.SetOffsetX2(origin.0)?;
            content.SetOffsetY2(origin.1)?;
            root.AddVisual(&content, false, None)?;
            target.SetRoot(&root)?;
            devices.dcomp.Commit()?;
            let brush = context.CreateSolidColorBrush(&rgba(255, 255, 255, 1.0), None)?;
            Ok(Self { context, output: Output::Layered { _target: target, root, content, surface, hwnd, size, origin }, graphics: graphics.clone(), brush, scale: Cell::new(1.0), pivot: Cell::new((0.0, 0.0)) })
        }
    }

    /// A layered surface's content moved by `(dx, dy)` px from its origin; nothing else.
    pub fn set_pan(&self, (dx, dy): (f32, f32)) -> windows::core::Result<()> {
        let Output::Layered { content, origin, .. } = &self.output else { return Ok(()) };
        unsafe {
            content.SetOffsetX2(origin.0 + dx)?;
            content.SetOffsetY2(origin.1 + dy)
        }
    }

    /// A layered surface shown only inside `rect` (window coordinates, fixed on screen whatever
    /// the pan), or all of it.
    pub fn set_clip(&self, rect: Option<D2D_RECT_F>) -> windows::core::Result<()> {
        let Output::Layered { root, .. } = &self.output else { return Ok(()) };
        unsafe {
            match rect {
                Some(rect) => root.SetClip2(&rect),
                None => root.SetClip(None::<&IDCompositionClip>),
            }
        }
    }

    /// Hands a layered surface's drawing, pan and clip to the compositor together.
    pub fn commit(&self) -> windows::core::Result<()> {
        if !matches!(self.output, Output::Layered { .. }) {
            return Ok(());
        }
        unsafe { self.graphics.devices.borrow().dcomp.Commit() }
    }

    /// `begin` for any surface; a layered one opens its DirectComposition surface for drawing.
    pub fn start(&self) -> windows::core::Result<()> {
        let Output::Layered { surface, size, origin, .. } = &self.output else {
            self.begin();
            return Ok(());
        };
        unsafe {
            let mut offset = POINT::default();
            let texture: IDXGISurface = surface.BeginDraw(None, &mut offset)?;
            let bitmap = match self.context.CreateBitmapFromDxgiSurface(&texture, Some(&bitmap_properties(D2D1_BITMAP_OPTIONS_TARGET | D2D1_BITMAP_OPTIONS_CANNOT_DRAW))) {
                Ok(bitmap) => bitmap,
                Err(error) => {
                    let _ = surface.EndDraw();
                    return Err(error);
                }
            };
            self.context.SetTarget(&bitmap);
            self.context.BeginDraw();
            // The surface's place in the texture DirectComposition hands out, from window
            // coordinates; only its own pixels are cleared (the texture may hold others).
            let (tx, ty) = (offset.x as f32 - origin.0, offset.y as f32 - origin.1);
            self.context.SetTransform(&Matrix3x2::translation(tx, ty));
            let own = D2D_RECT_F { left: origin.0, top: origin.1, right: origin.0 + size.0 as f32, bottom: origin.1 + size.1 as f32 };
            self.context.PushAxisAlignedClip(&own, windows::Win32::Graphics::Direct2D::D2D1_ANTIALIAS_MODE_ALIASED);
            self.context.Clear(Some(&rgba(0, 0, 0, 0.0)));
            // The scale about the pivot after the clip (which stays in window px).
            if self.scale.get() != 1.0 {
                self.context.SetTransform(&self.scaled_transform((tx, ty)));
            }
        }
        Ok(())
    }

    /// A window surface made again on the graphics' current devices, for the same window and size
    /// (after `Graphics::recover`); the caller redraws. Offscreen surfaces are left as they are.
    pub fn rebuild(&mut self) -> windows::core::Result<()> {
        let (hwnd, width, height, layered) = match &self.output {
            Output::Window { hwnd, width, height, .. } => (*hwnd, *width, *height, None),
            Output::Layered { hwnd, size, origin, .. } => (*hwnd, size.0, size.1, Some((*size, *origin))),
            Output::Detached { hwnd, width, height, layered } => (*hwnd, *width, *height, *layered),
            Output::Offscreen { .. } => return Ok(()),
        };
        // A window takes one composition target: the old one goes first (kept detached, so a
        // failed attempt can be retried).
        unsafe { self.context.SetTarget(None) };
        self.output = Output::Detached { hwnd, width, height, layered };
        let graphics = self.graphics.clone();
        *self = match layered {
            Some((size, origin)) => Self::layered(&graphics, hwnd, size, origin)?,
            None => Self::new(&graphics, hwnd, width, height)?,
        };
        Ok(())
    }

    /// A layered surface made again at another size for the same window (the old composition
    /// target goes first, as in `rebuild`); the caller redraws.
    pub fn resize_layered(&mut self, size: (u32, u32)) -> windows::core::Result<()> {
        let Output::Layered { hwnd, origin, .. } = &self.output else { return Ok(()) };
        let (hwnd, origin) = (*hwnd, *origin);
        unsafe { self.context.SetTarget(None) };
        self.output = Output::Detached { hwnd, width: size.0, height: size.1, layered: Some((size, origin)) };
        let graphics = self.graphics.clone();
        *self = Self::layered(&graphics, hwnd, size, origin)?;
        Ok(())
    }

    pub fn text_format(
        &self,
        size: f32,
        weight: DWRITE_FONT_WEIGHT,
        align: DWRITE_TEXT_ALIGNMENT,
    ) -> windows::core::Result<IDWriteTextFormat> {
        self.family_format(if self.graphics.fonts.is_some() { "Montserrat" } else { "Segoe UI" }, size, weight, align)
    }

    /// A text format in the game's map label font (Titillium Web).
    pub fn map_label_format(&self, size: f32, weight: DWRITE_FONT_WEIGHT, align: DWRITE_TEXT_ALIGNMENT) -> windows::core::Result<IDWriteTextFormat> {
        self.family_format(if self.graphics.fonts.is_some() { "Titillium Web" } else { "Segoe UI" }, size, weight, align)
    }

    fn family_format(&self, family: &str, size: f32, weight: DWRITE_FONT_WEIGHT, align: DWRITE_TEXT_ALIGNMENT) -> windows::core::Result<IDWriteTextFormat> {
        unsafe {
            let format = self.graphics.dwrite.CreateTextFormat(
                &HSTRING::from(family),
                self.graphics.fonts.as_ref(),
                weight,
                DWRITE_FONT_STYLE_NORMAL,
                DWRITE_FONT_STRETCH_NORMAL,
                size,
                &HSTRING::from("en-us"),
            )?;
            format.SetTextAlignment(align)?;
            format.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER)?;
            // One line; text too long for its box ends in an ellipsis.
            format.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP)?;
            let ellipsis = self.graphics.dwrite.CreateEllipsisTrimmingSign(&format)?;
            let trimming = DWRITE_TRIMMING { granularity: DWRITE_TRIMMING_GRANULARITY_CHARACTER, delimiter: 0, delimiterCount: 0 };
            format.SetTrimming(&trimming, &ellipsis)?;
            Ok(format)
        }
    }

    pub fn begin(&self) {
        unsafe {
            self.context.BeginDraw();
            self.context.Clear(Some(&rgba(0, 0, 0, 0.0)));
            self.context.SetTransform(&self.scaled_transform((0.0, 0.0)));
        }
    }

    /// `scale` about `pivot`, then moved by `shift`.
    fn scaled_transform(&self, (sx, sy): (f32, f32)) -> Matrix3x2 {
        let (k, (px, py)) = (self.scale.get(), self.pivot.get());
        Matrix3x2 { M11: k, M12: 0.0, M21: 0.0, M22: k, M31: px * (1.0 - k) + sx, M32: py * (1.0 - k) + sy }
    }

    /// The scale the next drawings (from `begin`) are made at, about the surface's top-left.
    pub fn set_scale(&self, k: f32) {
        self.scale.set(k);
        self.pivot.set((0.0, 0.0));
    }

    /// The scale the next drawings are made at, about `pivot` (surface px), which stays put.
    pub fn set_scale_about(&self, k: f32, pivot: (f32, f32)) {
        self.scale.set(k);
        self.pivot.set(pivot);
    }

    pub fn end(&self) -> windows::core::Result<()> {
        unsafe {
            if let Output::Layered { surface, .. } = &self.output {
                self.context.PopAxisAlignedClip();
                self.context.SetTransform(&Matrix3x2::identity());
                let drawn = self.context.EndDraw(None, None);
                self.context.SetTarget(None);
                surface.EndDraw()?;
                return drawn;
            }
            self.context.EndDraw(None, None)?;
            match &self.output {
                // Not waiting for a vertical blank: the compositor takes the newest buffer at its
                // next pass. Waiting put the marks one to three screen refreshes behind the game's
                // own steps in 35 % of them while dragging V1 (measured with `--latency`:
                // 1280 steps, 837 in the same refresh, 265 one late, 112 two, 66 three or more),
                // the windows presenting one after another each waiting their turn.
                Output::Window { swap_chain, .. } => swap_chain.Present(0, DXGI_PRESENT(0)).ok(),
                Output::Offscreen { .. } | Output::Detached { .. } | Output::Layered { .. } => Ok(()),
            }
        }
    }

    /// Text with the game's drop shadow: the same text 2 px right and 2 px down in 35 % black
    /// (fitted on the angle readout over a bright green backdrop: correlation 1.00, no blur).
    pub fn text(&self, s: &str, format: &IDWriteTextFormat, rect: D2D_RECT_F, color: D2D1_COLOR_F) {
        self.shadowed(s, format, rect, color, &self.graphics.value_params);
    }

    /// `text` rasterised like the readout's labels (crisper).
    pub fn label_text(&self, s: &str, format: &IDWriteTextFormat, rect: D2D_RECT_F, color: D2D1_COLOR_F) {
        self.shadowed(s, format, rect, color, &self.graphics.label_params);
    }

    fn shadowed(&self, s: &str, format: &IDWriteTextFormat, rect: D2D_RECT_F, color: D2D1_COLOR_F, params: &IDWriteRenderingParams) {
        let wide: Vec<u16> = s.encode_utf16().collect();
        let (dx, dy) = SHADOW_OFFSET;
        unsafe {
            self.context.SetTextRenderingParams(params);
            self.brush.SetColor(&rgba(0, 0, 0, SHADOW_ALPHA * color.a));
            let shadow = D2D_RECT_F { left: rect.left + dx, top: rect.top + dy, right: rect.right + dx, bottom: rect.bottom + dy };
            self.context.DrawText(&wide, format, &shadow, &self.brush, D2D1_DRAW_TEXT_OPTIONS_NONE, DWRITE_MEASURING_MODE_NATURAL);
            self.brush.SetColor(&color);
            self.context.DrawText(&wide, format, &rect, &self.brush, D2D1_DRAW_TEXT_OPTIONS_NONE, DWRITE_MEASURING_MODE_NATURAL);
            self.context.SetTextRenderingParams(None);
        }
    }

    /// Text with a dark outline, like the game's map labels: the text drawn 1 px around in
    /// `outline`, then filled.
    pub fn outlined_text(&self, s: &str, format: &IDWriteTextFormat, rect: D2D_RECT_F, color: D2D1_COLOR_F, outline: D2D1_COLOR_F) {
        let wide: Vec<u16> = s.encode_utf16().collect();
        unsafe {
            self.context.SetTextRenderingParams(&self.graphics.label_params);
            self.brush.SetColor(&outline);
            for (dx, dy) in [(-1.0, 0.0), (1.0, 0.0), (0.0, -1.0), (0.0, 1.0), (-1.0, -1.0), (1.0, -1.0), (-1.0, 1.0), (1.0, 1.0)] {
                let r = D2D_RECT_F { left: rect.left + dx, top: rect.top + dy, right: rect.right + dx, bottom: rect.bottom + dy };
                self.context.DrawText(&wide, format, &r, &self.brush, D2D1_DRAW_TEXT_OPTIONS_NONE, DWRITE_MEASURING_MODE_NATURAL);
            }
            self.brush.SetColor(&color);
            self.context.DrawText(&wide, format, &rect, &self.brush, D2D1_DRAW_TEXT_OPTIONS_NONE, DWRITE_MEASURING_MODE_NATURAL);
            self.context.SetTextRenderingParams(None);
        }
    }

    /// Text without a shadow in the readout values' rendering (`mode` 0: anti-aliased both ways),
    /// the labels' (1: horizontally only) or without grid fitting (2: the game's small UI text).
    pub fn plain_text(&self, s: &str, format: &IDWriteTextFormat, rect: D2D_RECT_F, color: D2D1_COLOR_F, mode: u8) {
        let wide: Vec<u16> = s.encode_utf16().collect();
        unsafe {
            self.context.SetTextRenderingParams(match mode {
                1 => &self.graphics.label_params,
                2 => &self.graphics.unhinted_params,
                _ => &self.graphics.value_params,
            });
            self.brush.SetColor(&color);
            self.context.DrawText(&wide, format, &rect, &self.brush, D2D1_DRAW_TEXT_OPTIONS_NONE, DWRITE_MEASURING_MODE_NATURAL);
            self.context.SetTextRenderingParams(None);
        }
    }

    /// Text without a shadow (on a card background, like the game's card text).
    pub fn flat_text(&self, s: &str, format: &IDWriteTextFormat, rect: D2D_RECT_F, color: D2D1_COLOR_F) {
        let wide: Vec<u16> = s.encode_utf16().collect();
        unsafe {
            self.brush.SetColor(&color);
            self.context.DrawText(&wide, format, &rect, &self.brush, D2D1_DRAW_TEXT_OPTIONS_NONE, DWRITE_MEASURING_MODE_NATURAL);
        }
    }

    /// Width (px) of `s` laid out in `format`, trailing spaces excluded.
    pub fn text_width(&self, s: &str, format: &IDWriteTextFormat) -> windows::core::Result<f32> {
        let wide: Vec<u16> = s.encode_utf16().collect();
        unsafe {
            let layout = self.graphics.dwrite.CreateTextLayout(&wide, format, 10_000.0, 100.0)?;
            let mut metrics = Default::default();
            layout.GetMetrics(&mut metrics)?;
            Ok(metrics.width)
        }
    }

    /// Line box height, baseline (from the line's top) and capital height (px) of `format`, for
    /// `text_on_caps`.
    pub fn cap_metrics(&self, format: &IDWriteTextFormat) -> windows::core::Result<CapMetrics> {
        use windows::Win32::Graphics::DirectWrite::{DWRITE_FONT_METRICS, DWRITE_LINE_METRICS};
        unsafe {
            let wide: Vec<u16> = "0".encode_utf16().collect();
            let layout = self.graphics.dwrite.CreateTextLayout(&wide, format, 1000.0, 1000.0)?;
            let mut lines = [DWRITE_LINE_METRICS::default()];
            let mut count = 0u32;
            layout.GetLineMetrics(Some(&mut lines), &mut count)?;
            let size = format.GetFontSize();
            // The capital height from the font itself; 0.7 em if the font cannot be found.
            let cap = (|| -> windows::core::Result<f32> {
                let collection = format.GetFontCollection()?;
                let mut name = vec![0u16; format.GetFontFamilyNameLength() as usize + 1];
                format.GetFontFamilyName(&mut name)?;
                let (mut index, mut exists) = (0u32, windows::core::BOOL(0));
                collection.FindFamilyName(PCWSTR(name.as_ptr()), &mut index, &mut exists)?;
                let font = collection.GetFontFamily(index)?.GetFirstMatchingFont(format.GetFontWeight(), format.GetFontStretch(), format.GetFontStyle())?;
                let mut m = DWRITE_FONT_METRICS::default();
                font.GetMetrics(&mut m);
                Ok(f32::from(m.capHeight) / f32::from(m.designUnitsPerEm) * size)
            })()
            .unwrap_or(0.7 * size);
            Ok(CapMetrics { height: lines[0].height, baseline: lines[0].baseline, cap })
        }
    }

    /// `s` in `format` between `left` and `right` (the format's own alignment), its capitals and
    /// digits centred on `cy` and 0.5 px above it: the line box's centre puts text low, and the
    /// lowercase and symbols weigh it down further (optical centring).
    #[allow(clippy::too_many_arguments)]
    pub fn text_on_caps(&self, s: &str, format: &IDWriteTextFormat, m: &CapMetrics, left: f32, right: f32, cy: f32, color: D2D1_COLOR_F) {
        let h = m.height + 8.0;
        let top = cy - 0.5 + m.cap / 2.0 - m.baseline - (h - m.height) / 2.0;
        self.flat_text(s, format, D2D_RECT_F { left, top, right, bottom: top + h }, color);
    }

    /// Rounded rectangle (the game's buttons and key hints), filled or outlined.
    pub fn rounded(&self, rect: D2D_RECT_F, radius: f32, fill: Option<D2D1_COLOR_F>, stroke: Option<(D2D1_COLOR_F, f32)>) {
        use windows::Win32::Graphics::Direct2D::D2D1_ROUNDED_RECT;
        let r = D2D1_ROUNDED_RECT { rect, radiusX: radius, radiusY: radius };
        unsafe {
            if let Some(color) = fill {
                self.brush.SetColor(&color);
                self.context.FillRoundedRectangle(&r, &self.brush);
            }
            if let Some((color, width)) = stroke {
                self.brush.SetColor(&color);
                self.context.DrawRoundedRectangle(&r, &self.brush, width, None);
            }
        }
    }

    /// Circle outline of stroke `width`, plain or dashed: `dash` is (dash, gap) in px.
    pub fn styled_ring(&self, cx: f32, cy: f32, r: f32, color: D2D1_COLOR_F, width: f32, dash: Option<(f32, f32)>) -> windows::core::Result<()> {
        use windows::Win32::Graphics::Direct2D::{D2D1_CAP_STYLE_FLAT, D2D1_DASH_STYLE_CUSTOM, D2D1_DASH_STYLE_SOLID, D2D1_LINE_JOIN_MITER, D2D1_STROKE_STYLE_PROPERTIES1, D2D1_STROKE_TRANSFORM_TYPE_NORMAL};
        unsafe {
            self.brush.SetColor(&color);
            let e = D2D1_ELLIPSE { point: Vector2 { X: cx, Y: cy }, radiusX: r, radiusY: r };
            let props = D2D1_STROKE_STYLE_PROPERTIES1 {
                startCap: D2D1_CAP_STYLE_FLAT,
                endCap: D2D1_CAP_STYLE_FLAT,
                dashCap: D2D1_CAP_STYLE_FLAT,
                lineJoin: D2D1_LINE_JOIN_MITER,
                miterLimit: 10.0,
                dashStyle: if dash.is_some() { D2D1_DASH_STYLE_CUSTOM } else { D2D1_DASH_STYLE_SOLID },
                dashOffset: 0.0,
                transformType: D2D1_STROKE_TRANSFORM_TYPE_NORMAL,
            };
            // Dash lengths are in stroke widths.
            let pattern = dash.map(|(on, off)| [on / width, off / width]);
            let style = self.graphics.factory.CreateStrokeStyle(&props, pattern.as_ref().map(|p| p.as_slice()))?;
            self.context.DrawEllipse(&e, &self.brush, width, &style);
        }
        Ok(())
    }

    /// Circle outline like `styled_ring`, drawn only where it crosses `clip`: a range ring around
    /// a gun far off the view is thousands of px round, and dashing all of it took 5.5 ms a
    /// frame at 2.6x (`examples/map_draw_bench.rs`). The dashes keep their places on the circle
    /// (the pattern counts from angle 0), so a pan does not make them crawl.
    pub fn styled_ring_in(&self, cx: f32, cy: f32, r: f32, clip: D2D_RECT_F, color: D2D1_COLOR_F, width: f32, dash: Option<(f32, f32)>) -> windows::core::Result<()> {
        use std::f32::consts::{PI, TAU};
        use windows::Win32::Graphics::Direct2D::Common::{D2D_SIZE_F, D2D1_FIGURE_BEGIN_HOLLOW};
        use windows::Win32::Graphics::Direct2D::{D2D1_ARC_SEGMENT, D2D1_ARC_SIZE_LARGE, D2D1_ARC_SIZE_SMALL, D2D1_SWEEP_DIRECTION_CLOCKWISE, D2D1_CAP_STYLE_FLAT, D2D1_DASH_STYLE_CUSTOM, D2D1_DASH_STYLE_SOLID, D2D1_LINE_JOIN_MITER, D2D1_STROKE_STYLE_PROPERTIES1, D2D1_STROKE_TRANSFORM_TYPE_NORMAL};
        // Grown by the stroke so its edge is not cut short at the clip.
        let (l, t, rr, b) = (clip.left - width, clip.top - width, clip.right + width, clip.bottom + width);
        if cx - r >= l && cx + r <= rr && cy - r >= t && cy + r <= b {
            return self.styled_ring(cx, cy, r, color, width, dash);
        }
        let inside = |a: f32| {
            let (x, y) = (cx + r * a.cos(), cy + r * a.sin());
            x >= l && x <= rr && y >= t && y <= b
        };
        let mut cuts: Vec<f32> = Vec::new();
        for x in [l, rr] {
            let c = (x - cx) / r;
            if c.abs() <= 1.0 {
                cuts.extend([c.acos(), -c.acos()]);
            }
        }
        for y in [t, b] {
            let s = (y - cy) / r;
            if s.abs() <= 1.0 {
                cuts.extend([s.asin(), PI - s.asin()]);
            }
        }
        let mut cuts: Vec<f32> = cuts.into_iter().map(|a| a.rem_euclid(TAU)).collect();
        cuts.sort_by(f32::total_cmp);
        cuts.dedup_by(|a, b| (*a - *b).abs() < 1e-6);
        if cuts.is_empty() {
            // No crossing: wholly inside (handled above) or wholly outside.
            return Ok(());
        }
        let arcs: Vec<(f32, f32)> = (0..cuts.len())
            .map(|i| (cuts[i], if i + 1 < cuts.len() { cuts[i + 1] } else { cuts[0] + TAU }))
            .filter(|&(a0, a1)| a1 - a0 > 1e-5 && inside((a0 + a1) / 2.0))
            .collect();
        unsafe {
            self.brush.SetColor(&color);
            for (a0, a1) in arcs {
                let geometry = self.graphics.factory.CreatePathGeometry()?;
                let sink = geometry.Open()?;
                sink.BeginFigure(Vector2 { X: cx + r * a0.cos(), Y: cy + r * a0.sin() }, D2D1_FIGURE_BEGIN_HOLLOW);
                sink.AddArc(&D2D1_ARC_SEGMENT {
                    point: Vector2 { X: cx + r * a1.cos(), Y: cy + r * a1.sin() },
                    size: D2D_SIZE_F { width: r, height: r },
                    rotationAngle: 0.0,
                    sweepDirection: D2D1_SWEEP_DIRECTION_CLOCKWISE,
                    arcSize: if a1 - a0 > PI { D2D1_ARC_SIZE_LARGE } else { D2D1_ARC_SIZE_SMALL },
                });
                sink.EndFigure(D2D1_FIGURE_END_OPEN);
                sink.Close()?;
                // Dash lengths and offset are in stroke widths; the offset carries the pattern
                // on from angle 0 to where this arc starts.
                let pattern = dash.map(|(on, off)| [on / width, off / width]);
                let offset = pattern.map_or(0.0, |p| (a0 * r / width).rem_euclid(p[0] + p[1]));
                let props = D2D1_STROKE_STYLE_PROPERTIES1 {
                    startCap: D2D1_CAP_STYLE_FLAT,
                    endCap: D2D1_CAP_STYLE_FLAT,
                    dashCap: D2D1_CAP_STYLE_FLAT,
                    lineJoin: D2D1_LINE_JOIN_MITER,
                    miterLimit: 10.0,
                    dashStyle: if dash.is_some() { D2D1_DASH_STYLE_CUSTOM } else { D2D1_DASH_STYLE_SOLID },
                    dashOffset: offset,
                    transformType: D2D1_STROKE_TRANSFORM_TYPE_NORMAL,
                };
                let style = self.graphics.factory.CreateStrokeStyle(&props, pattern.as_ref().map(|p| p.as_slice()))?;
                self.context.DrawGeometry(&geometry, &self.brush, width, &style);
            }
        }
        Ok(())
    }

    /// Clips drawing to `rect` until `pop_clip`.
    pub fn push_clip(&self, rect: D2D_RECT_F) {
        unsafe { self.context.PushAxisAlignedClip(&rect, windows::Win32::Graphics::Direct2D::D2D1_ANTIALIAS_MODE_ALIASED) }
    }

    pub fn pop_clip(&self) {
        unsafe { self.context.PopAxisAlignedClip() }
    }

    /// Dashed circle: `dashes` arcs (as short chords) with gaps of the same length.
    pub fn dashed_ring(&self, cx: f32, cy: f32, r: f32, color: D2D1_COLOR_F, width: f32, dashes: u32) {
        let step = std::f32::consts::TAU / (2 * dashes) as f32;
        let segments = 3;
        unsafe {
            self.brush.SetColor(&color);
            for k in 0..dashes {
                let start = 2.0 * k as f32 * step;
                for j in 0..segments {
                    let (a, b) = (start + step * j as f32 / segments as f32, start + step * (j + 1) as f32 / segments as f32);
                    self.context.DrawLine(Vector2 { X: cx + r * a.cos(), Y: cy + r * a.sin() }, Vector2 { X: cx + r * b.cos(), Y: cy + r * b.sin() }, &self.brush, width, None);
                }
            }
        }
    }

    /// Circle outline of stroke `width`, centred on the radius.
    pub fn ring(&self, cx: f32, cy: f32, r: f32, color: D2D1_COLOR_F, width: f32) {
        unsafe {
            self.brush.SetColor(&color);
            let e = D2D1_ELLIPSE { point: Vector2 { X: cx, Y: cy }, radiusX: r, radiusY: r };
            self.context.DrawEllipse(&e, &self.brush, width, None);
        }
    }

    pub fn circle(&self, cx: f32, cy: f32, r: f32, color: D2D1_COLOR_F) {
        unsafe {
            self.brush.SetColor(&color);
            let e = D2D1_ELLIPSE {
                point: Vector2 { X: cx, Y: cy },
                radiusX: r,
                radiusY: r,
            };
            self.context.FillEllipse(&e, &self.brush);
        }
    }

    pub fn fill_rect(&self, rect: D2D_RECT_F, color: D2D1_COLOR_F) {
        unsafe {
            self.brush.SetColor(&color);
            self.context.FillRectangle(&rect, &self.brush);
        }
    }

    /// Rectangle outline of `width` px drawn inside `rect`.
    pub fn stroke_rect(&self, rect: D2D_RECT_F, color: D2D1_COLOR_F, width: f32) {
        let h = width / 2.0;
        let inner = D2D_RECT_F { left: rect.left + h, top: rect.top + h, right: rect.right - h, bottom: rect.bottom - h };
        unsafe {
            self.brush.SetColor(&color);
            self.context.DrawRectangle(&inner, &self.brush, width, None);
        }
    }

    /// Closed polygon, filled and/or outlined.
    pub fn polygon(&self, points: &[(f32, f32)], fill: Option<D2D1_COLOR_F>, stroke: Option<(D2D1_COLOR_F, f32)>) -> windows::core::Result<()> {
        unsafe {
            let geometry = self.graphics.factory.CreatePathGeometry()?;
            let sink = geometry.Open()?;
            sink.BeginFigure(Vector2 { X: points[0].0, Y: points[0].1 }, D2D1_FIGURE_BEGIN_FILLED);
            for p in &points[1..] {
                sink.AddLine(Vector2 { X: p.0, Y: p.1 });
            }
            sink.EndFigure(D2D1_FIGURE_END_CLOSED);
            sink.Close()?;
            if let Some(color) = fill {
                self.brush.SetColor(&color);
                self.context.FillGeometry(&geometry, &self.brush, None);
            }
            if let Some((color, width)) = stroke {
                self.brush.SetColor(&color);
                self.context.DrawGeometry(&geometry, &self.brush, width, None);
            }
        }
        Ok(())
    }

    /// Open line through the points.
    pub fn polyline(&self, points: &[(f32, f32)], color: D2D1_COLOR_F, width: f32) -> windows::core::Result<()> {
        unsafe {
            let geometry = self.graphics.factory.CreatePathGeometry()?;
            let sink = geometry.Open()?;
            sink.BeginFigure(Vector2 { X: points[0].0, Y: points[0].1 }, D2D1_FIGURE_BEGIN_FILLED);
            for p in &points[1..] {
                sink.AddLine(Vector2 { X: p.0, Y: p.1 });
            }
            sink.EndFigure(D2D1_FIGURE_END_OPEN);
            sink.Close()?;
            self.brush.SetColor(&color);
            self.context.DrawGeometry(&geometry, &self.brush, width, None);
        }
        Ok(())
    }

    /// Open line through the points, dashed: `dash` is (dash, gap) in px.
    pub fn dashed_polyline(&self, points: &[(f32, f32)], color: D2D1_COLOR_F, width: f32, dash: (f32, f32)) -> windows::core::Result<()> {
        use windows::Win32::Graphics::Direct2D::Common::D2D1_FIGURE_BEGIN_HOLLOW;
        use windows::Win32::Graphics::Direct2D::{D2D1_CAP_STYLE_FLAT, D2D1_DASH_STYLE_CUSTOM, D2D1_LINE_JOIN_ROUND, D2D1_STROKE_STYLE_PROPERTIES1, D2D1_STROKE_TRANSFORM_TYPE_NORMAL};
        unsafe {
            let geometry = self.graphics.factory.CreatePathGeometry()?;
            let sink = geometry.Open()?;
            sink.BeginFigure(Vector2 { X: points[0].0, Y: points[0].1 }, D2D1_FIGURE_BEGIN_HOLLOW);
            for p in &points[1..] {
                sink.AddLine(Vector2 { X: p.0, Y: p.1 });
            }
            sink.EndFigure(D2D1_FIGURE_END_OPEN);
            sink.Close()?;
            let props = D2D1_STROKE_STYLE_PROPERTIES1 {
                startCap: D2D1_CAP_STYLE_FLAT,
                endCap: D2D1_CAP_STYLE_FLAT,
                dashCap: D2D1_CAP_STYLE_FLAT,
                lineJoin: D2D1_LINE_JOIN_ROUND,
                miterLimit: 10.0,
                dashStyle: D2D1_DASH_STYLE_CUSTOM,
                dashOffset: 0.0,
                transformType: D2D1_STROKE_TRANSFORM_TYPE_NORMAL,
            };
            // Dash lengths are in stroke widths.
            let pattern = [dash.0 / width, dash.1 / width];
            let style = self.graphics.factory.CreateStrokeStyle(&props, Some(&pattern))?;
            self.brush.SetColor(&color);
            self.context.DrawGeometry(&geometry, &self.brush, width, &style);
        }
        Ok(())
    }

    /// Filled triangle through three points.
    pub fn triangle(&self, p: [(f32, f32); 3], color: D2D1_COLOR_F) -> windows::core::Result<()> {
        unsafe {
            let geometry = self.graphics.factory.CreatePathGeometry()?;
            let sink = geometry.Open()?;
            sink.BeginFigure(Vector2 { X: p[0].0, Y: p[0].1 }, D2D1_FIGURE_BEGIN_FILLED);
            sink.AddLine(Vector2 { X: p[1].0, Y: p[1].1 });
            sink.AddLine(Vector2 { X: p[2].0, Y: p[2].1 });
            sink.EndFigure(D2D1_FIGURE_END_CLOSED);
            sink.Close()?;
            self.brush.SetColor(&color);
            self.context.FillGeometry(&geometry, &self.brush, None);
        }
        Ok(())
    }
}

/// Montserrat from `font_dir` as a private DirectWrite collection.
fn load_fonts(dwrite: &IDWriteFactory5, font_dir: &Path) -> windows::core::Result<IDWriteFontCollection1> {
    unsafe {
        let builder = dwrite.CreateFontSetBuilder()?;
        let mut added = 0;
        // Montserrat (the game's UI) and Titillium Web (its map labels and grid letters).
        for name in ["Montserrat-Regular.ttf", "Montserrat-Medium.ttf", "Montserrat-Bold.ttf", "TitilliumWeb-Regular.ttf", "TitilliumWeb-Bold.ttf"] {
            let path = font_dir.join(name);
            if !path.is_file() {
                continue;
            }
            let file = dwrite.CreateFontFileReference(PCWSTR(HSTRING::from(path.as_os_str()).as_ptr()), None)?;
            builder.AddFontFile(&file)?;
            added += 1;
        }
        if added == 0 {
            return Err(windows::core::Error::from_hresult(windows::Win32::Foundation::E_FAIL));
        }
        let set = builder.CreateFontSet()?;
        dwrite.CreateFontCollectionFromFontSet(&set)
    }
}
