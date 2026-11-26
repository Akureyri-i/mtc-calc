/// Byte layout of a pixel buffer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PixelFormat {
    Rgb,
    Bgra,
    /// BGRA resampled from another configuration to the reference geometry (`layout::remap`), the
    /// fourth byte holding the brightest luma among the source pixels each was made from: thin
    /// bright strokes lose their peak in the resampling (`peak_luma`).
    BgraPeak,
}

/// A rectangle of frame pixels backed by a byte buffer.
#[derive(Clone, Copy)]
pub struct FrameView<'a> {
    pub data: &'a [u8],
    pub format: PixelFormat,
    /// Frame coordinate of the buffer's first pixel.
    pub origin: (i32, i32),
    pub width: u32,
    pub height: u32,
    /// Bytes per row.
    pub stride: usize,
}

impl<'a> FrameView<'a> {
    pub fn rgb(data: &'a [u8], width: u32, height: u32) -> Self {
        Self {
            data,
            format: PixelFormat::Rgb,
            origin: (0, 0),
            width,
            height,
            stride: width as usize * 3,
        }
    }

    /// The part of this view inside the frame rectangle (which it must cover): what a read-back
    /// of that region holds.
    pub fn sub(&self, x: i32, y: i32, width: u32, height: u32) -> Self {
        assert!(self.covers(x, y, width as i32, height as i32), "sub-view outside the view");
        let bytes = match self.format {
            PixelFormat::Rgb => 3,
            PixelFormat::Bgra | PixelFormat::BgraPeak => 4,
        };
        let start = (y - self.origin.1) as usize * self.stride + (x - self.origin.0) as usize * bytes;
        Self { data: &self.data[start..], format: self.format, origin: (x, y), width, height, stride: self.stride }
    }

    /// Whether the frame rectangle lies inside this view.
    pub fn covers(&self, x: i32, y: i32, width: i32, height: i32) -> bool {
        x >= self.origin.0
            && y >= self.origin.1
            && x + width <= self.origin.0 + self.width as i32
            && y + height <= self.origin.1 + self.height as i32
    }

    /// RGB at a frame coordinate. The caller keeps coordinates inside the view.
    #[inline]
    pub fn px(&self, x: i32, y: i32) -> [u8; 3] {
        let (bx, by) = ((x - self.origin.0) as usize, (y - self.origin.1) as usize);
        match self.format {
            PixelFormat::Rgb => {
                let i = by * self.stride + bx * 3;
                [self.data[i], self.data[i + 1], self.data[i + 2]]
            }
            PixelFormat::Bgra | PixelFormat::BgraPeak => {
                let i = by * self.stride + bx * 4;
                [self.data[i + 2], self.data[i + 1], self.data[i]]
            }
        }
    }
}

impl FrameView<'_> {
    /// BT.601 luma at a frame coordinate, 0..255.
    #[inline]
    pub fn luma(&self, x: i32, y: i32) -> f32 {
        let [r, g, b] = self.px(x, y);
        0.299 * f32::from(r) + 0.587 * f32::from(g) + 0.114 * f32::from(b)
    }

    /// The brightest luma the pixel was made from: its own luma, or on a resampled view the
    /// brightest of its source pixels (`PixelFormat::BgraPeak`).
    #[inline]
    pub fn peak_luma(&self, x: i32, y: i32) -> f32 {
        match self.format {
            PixelFormat::BgraPeak => {
                let (bx, by) = ((x - self.origin.0) as usize, (y - self.origin.1) as usize);
                f32::from(self.data[by * self.stride + bx * 4 + 3])
            }
            _ => self.luma(x, y),
        }
    }
}

/// Bright, unsaturated pixel: HSV value above `min_value` and saturation below 45/255.
#[inline]
pub fn is_light_grey(p: [u8; 3], min_value: u8) -> bool {
    let max = p[0].max(p[1]).max(p[2]);
    let min = p[0].min(p[1]).min(p[2]);
    max > min_value && (u32::from(max - min) * 255) < 45 * u32::from(max)
}
