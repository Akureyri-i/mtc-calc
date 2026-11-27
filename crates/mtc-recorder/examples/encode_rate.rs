//! Development: how many frames a second the recorder's PNG writer keeps up with, on a recorded
//! frame encoded over and over the way the writer does it, with 1 to `max` encoder threads.
//!   cargo run --release -p mtc-recorder --example encode_rate -- <frame.png> [max threads]
use std::io::BufWriter;
use std::time::Instant;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let path = &args[1];
    let max: usize = args.get(2).and_then(|v| v.parse().ok()).unwrap_or(6);
    let decoder = png::Decoder::new(std::io::BufReader::new(std::fs::File::open(path).expect("frame")));
    let mut reader = decoder.read_info().expect("png");
    let mut rgb = vec![0; reader.output_buffer_size().expect("size")];
    let info = reader.next_frame(&mut rgb).expect("decode");
    let (w, h) = (info.width, info.height);
    rgb.truncate(info.buffer_size());
    let encode = |rgb: &[u8]| {
        let mut out = Vec::with_capacity(8 << 20);
        let mut encoder = png::Encoder::new(BufWriter::new(&mut out), w, h);
        encoder.set_color(png::ColorType::Rgb);
        encoder.set_depth(png::BitDepth::Eight);
        encoder.set_compression(png::Compression::Fast);
        let mut writer = encoder.write_header().expect("header");
        writer.write_image_data(rgb).expect("data");
        writer.finish().expect("finish");
        out.len()
    };
    let bytes = encode(&rgb);
    println!("{w}x{h}, {:.1} MB a frame", bytes as f64 / 1e6);
    for threads in [1, 2, 3, 4, 6, 8].into_iter().filter(|&t| t <= max) {
        let per_thread = 6;
        let start = Instant::now();
        std::thread::scope(|s| {
            for _ in 0..threads {
                s.spawn(|| {
                    for _ in 0..per_thread {
                        encode(&rgb);
                    }
                });
            }
        });
        let secs = start.elapsed().as_secs_f64();
        let fps = (threads * per_thread) as f64 / secs;
        println!("{threads} threads: {fps:.1} frames/s, {:.0} MB/s to disk", fps * bytes as f64 / 1e6);
    }
}
