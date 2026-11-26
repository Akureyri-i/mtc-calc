//! Minimal RGB image type for recorded frames.

use std::fs::File;
use std::io::{self, BufReader, BufWriter};
use std::path::Path;

#[derive(Clone, Debug)]
pub struct RgbImage {
    pub width: u32,
    pub height: u32,
    /// Row-major RGB bytes.
    pub data: Vec<u8>,
}

impl RgbImage {
    pub fn new(width: u32, height: u32) -> Self {
        Self {
            width,
            height,
            data: vec![0; (width * height * 3) as usize],
        }
    }

    pub fn load_png(path: &Path) -> io::Result<Self> {
        let mut decoder = png::Decoder::new(BufReader::new(File::open(path)?));
        decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
        let mut reader = decoder.read_info().map_err(io::Error::other)?;
        let mut buffer = vec![0; reader.output_buffer_size().ok_or_else(|| io::Error::other("image too large"))?];
        let info = reader.next_frame(&mut buffer).map_err(io::Error::other)?;
        buffer.truncate(info.buffer_size());
        let channels = info.color_type.samples();
        let data = match channels {
            3 => buffer,
            4 => buffer.as_chunks::<4>().0.iter().flat_map(|p| [p[0], p[1], p[2]]).collect(),
            1 => buffer.iter().flat_map(|&v| [v, v, v]).collect(),
            2 => buffer.as_chunks::<2>().0.iter().flat_map(|p| [p[0], p[0], p[0]]).collect(),
            _ => return Err(io::Error::other("unsupported PNG layout")),
        };
        Ok(Self {
            width: info.width,
            height: info.height,
            data,
        })
    }

    pub fn save_png(&self, path: &Path) -> io::Result<()> {
        let mut encoder = png::Encoder::new(BufWriter::new(File::create(path)?), self.width, self.height);
        encoder.set_color(png::ColorType::Rgb);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().map_err(io::Error::other)?;
        writer.write_image_data(&self.data).map_err(io::Error::other)
    }

    pub fn pixel(&self, x: u32, y: u32) -> [u8; 3] {
        let i = ((y * self.width + x) * 3) as usize;
        [self.data[i], self.data[i + 1], self.data[i + 2]]
    }

    /// Copy of the rectangle, clipped to the image.
    pub fn crop(&self, x: u32, y: u32, width: u32, height: u32) -> Self {
        let width = width.min(self.width.saturating_sub(x));
        let height = height.min(self.height.saturating_sub(y));
        let mut out = Self::new(width, height);
        for row in 0..height {
            let src = (((y + row) * self.width + x) * 3) as usize;
            let dst = (row * width * 3) as usize;
            out.data[dst..dst + (width * 3) as usize]
                .copy_from_slice(&self.data[src..src + (width * 3) as usize]);
        }
        out
    }

    /// Nearest-neighbour enlargement, keeps pixels crisp for inspection.
    pub fn scale_nearest(&self, factor: u32) -> Self {
        let mut out = Self::new(self.width * factor, self.height * factor);
        for y in 0..out.height {
            for x in 0..out.width {
                let p = self.pixel(x / factor, y / factor);
                let i = ((y * out.width + x) * 3) as usize;
                out.data[i..i + 3].copy_from_slice(&p);
            }
        }
        out
    }

    pub fn paste(&mut self, other: &Self, x: u32, y: u32) {
        for row in 0..other.height.min(self.height.saturating_sub(y)) {
            let w = other.width.min(self.width.saturating_sub(x));
            let src = (row * other.width * 3) as usize;
            let dst = (((y + row) * self.width + x) * 3) as usize;
            self.data[dst..dst + (w * 3) as usize]
                .copy_from_slice(&other.data[src..src + (w * 3) as usize]);
        }
    }
}
