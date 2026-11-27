//! Map configuration, reference images and heightmaps.
//!
//! `data/maps.json` is generated from the upstream calculator's `maps.ts`
//! and embedded at build time. Reference images and heightmaps live in a local cache laid out like
//! the official map service (`<cache>/<slug>/user.png`, `heightmap.png`); `download` fills it from
//! the service.
//!
//! Heights are sampled like the upstream calculator: the heightmap's red channel at the rounded
//! pixel of the full-resolution file (x and y scaled separately; some heightmaps are not square),
//! 0–255 mapped linearly onto the map's range in studs. Averaging the largest files down 2×2 was
//! tried and dropped: over 200k random points it is off by up to 5–47 m at walls and cliffs.
//! The first load decodes the PNG row by row into a raw
//! copy beside it (`heightmap.r8`, one byte a pixel: 64 MB for Fulvia Gap's 7993²), which is then
//! mapped into memory: only the pages holding sampled points take memory.

use std::collections::HashMap;
use std::fs::File;
use std::io::{self, BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use memmap2::Mmap;

use serde::Deserialize;

/// Upstream conversion: 1 stud = 0.36 m (`studsToMeters(s) = s / (5 / 1.8)`).
pub const METRES_PER_STUD: f64 = 1.8 / 5.0;

#[derive(Clone, Debug, Deserialize)]
pub struct HeightRange {
    pub min: f64,
    pub max: f64,
}

#[derive(Clone, Debug, Deserialize)]
pub struct MapInfo {
    /// Upstream map id ("fulvia_gap").
    pub id: String,
    /// Upstream key ("Fulvia Gap").
    pub key: String,
    /// Path segment on the map service ("fulvia-gap").
    pub slug: String,
    pub name: String,
    /// Side of the square reference canvas.
    pub size_studs: f64,
    /// Heightmap value range in studs (black..white).
    pub heightmap: Option<HeightRange>,
    /// In the current multiplayer rotation.
    pub in_rotation: bool,
    /// Side of the zoomed-V1 lettered grid cells in metres, `None` where not measured.
    pub grid_cell_m: Option<f64>,
}

impl MapInfo {
    pub fn size_m(&self) -> f64 {
        self.size_studs * METRES_PER_STUD
    }
}

#[derive(Deserialize)]
struct MapsFile {
    maps: Vec<MapInfo>,
}

/// All maps in the upstream configuration.
pub fn all() -> &'static [MapInfo] {
    static MAPS: OnceLock<Vec<MapInfo>> = OnceLock::new();
    MAPS.get_or_init(|| {
        serde_json::from_str::<MapsFile>(include_str!("../../../data/maps.json"))
            .expect("data/maps.json")
            .maps
    })
}

pub fn by_id(id: &str) -> Option<&'static MapInfo> {
    all().iter().find(|m| m.id == id)
}

/// The debug map (a private server's console switches to it): flat, no reference picture or
/// heightmap on the map service (`debug/user.png` and `heightmap.png` answer 403). Its side was
/// measured from the distance signs: the arrow moved 0.20885 map widths from the 1000 m sign to
/// the 2000 m sign, 4788 m (ten test shots fit a range scale of 1.0005 with it).
pub fn debug_map() -> &'static MapInfo {
    static DEBUG: OnceLock<MapInfo> = OnceLock::new();
    DEBUG.get_or_init(|| MapInfo {
        id: "debug".to_owned(),
        key: "Debug".to_owned(),
        slug: "debug".to_owned(),
        name: "Debug map".to_owned(),
        size_studs: 4788.0 / METRES_PER_STUD,
        heightmap: None,
        in_rotation: false,
        grid_cell_m: None,
    })
}

/// A map by id, the debug map included ("debug").
pub fn by_id_or_debug(id: &str) -> Option<&'static MapInfo> {
    by_id(id).or_else(|| (id == "debug").then(debug_map))
}

/// An objective (capture point): its letter and icon centre in normalised map coordinates.
#[derive(Clone, Debug, Deserialize)]
pub struct Objective {
    pub letter: String,
    pub u: f64,
    pub v: f64,
}

#[derive(Deserialize)]
struct ObjectivesFile {
    maps: HashMap<String, Vec<Objective>>,
}

/// Objectives of a map from `data/objectives.json` (measured on recordings; they
/// sit at fixed places on each map). Empty for maps
/// outside the table.
pub fn objectives(map_id: &str) -> &'static [Objective] {
    static TABLE: OnceLock<HashMap<String, Vec<Objective>>> = OnceLock::new();
    TABLE
        .get_or_init(|| {
            serde_json::from_str::<ObjectivesFile>(include_str!("../../../data/objectives.json"))
                .expect("data/objectives.json")
                .maps
        })
        .get(map_id)
        .map_or(&[], Vec::as_slice)
}

/// `%LOCALAPPDATA%/MTC Calculator/maps`, or `MTC_MAP_CACHE` when set.
pub fn cache_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("MTC_MAP_CACHE") {
        return PathBuf::from(dir);
    }
    let base = std::env::var_os("LOCALAPPDATA").expect("LOCALAPPDATA");
    PathBuf::from(base).join("MTC Calculator").join("maps")
}

pub fn reference_path(map: &MapInfo) -> PathBuf {
    cache_dir().join(&map.slug).join(REFERENCE_FILE)
}

/// The map's identification thumbnail as the HUD saves it (made from the reference, next to it).
pub fn thumbnail_path(map: &MapInfo) -> PathBuf {
    cache_dir().join(&map.slug).join("thumbnail.bin")
}

/// A 64-bit FNV-1a hash of `bytes`: the same on every machine and every run (what a saved
/// thumbnail was made from).
pub fn content_hash(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, &b| (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3))
}

pub fn heightmap_path(map: &MapInfo) -> PathBuf {
    cache_dir().join(&map.slug).join(HEIGHTMAP_FILE)
}

/// The official map service (the upstream calculator's image host) and its file names.
const MAP_SERVICE: &str = "https://maps.artillery-calculator.com";
/// The map service's pictures and heightmaps copied to the calculator's data host (same layout;
/// files over the host's size limit are not there). Downloads try both at once.
const MAP_MIRROR: &str = "https://data.mtc-calc.dev/maps/v1";
pub const REFERENCE_FILE: &str = "user.png";
pub const HEIGHTMAP_FILE: &str = "heightmap.png";
/// The heightmap's red channel, made from `HEIGHTMAP_FILE` on first load: `RAW_MAGIC`, width and
/// height (u32, little-endian), padded to `RAW_HEADER`, then `TILE` x `TILE` tiles row by row, each
/// tile's pixels row by row (edge tiles padded). A tile is one 4 KB page, so the heights along a
/// shot's line bring in a page every `TILE` pixels instead of one every row: a 2 km line on a 4 km
/// map touched about 2000 pages (8 MB of working set) a layer in rows.
pub const HEIGHTMAP_RAW_FILE: &str = "heightmap.r8";
/// The terrain layer: the heightmap with everything narrower than ~16 m (trees, small houses, narrow
/// banks) removed, on the heightmap's size and scale. Computed ahead from the map service's
/// heightmaps and published on `TERRAIN_SERVICE` (`fetch_terrain`); without it obstructions are only
/// warned about.
pub const TERRAIN_FILE: &str = "terrain.png";
/// Where the terrain layers are published: `manifest.json` and `<slug>.png`.
const TERRAIN_SERVICE: &str = "https://data.mtc-calc.dev/terrain/v1";
/// The last manifest downloaded, in the cache folder: it stands in while the service cannot be
/// reached, so a layer already here keeps being checked against its heightmap.
const TERRAIN_MANIFEST_FILE: &str = "terrain-manifest.json";
pub const TERRAIN_RAW_FILE: &str = "terrain.r8";
const RAW_MAGIC: &[u8; 4] = b"MTCT";
/// Tile side (pixels): a tile is `TILE_BYTES`, one page.
const TILE: usize = 64;
const TILE_BYTES: usize = TILE * TILE;
/// The header takes a whole tile so the tiles fall on page boundaries.
const RAW_HEADER: usize = TILE_BYTES;

/// Byte offset of pixel (`x`, `y`) in a tiled raw copy `width` pixels wide.
fn tiled_offset(width: usize, x: usize, y: usize) -> usize {
    RAW_HEADER + ((y / TILE) * width.div_ceil(TILE) + x / TILE) * TILE_BYTES + (y % TILE) * TILE + x % TILE
}

/// Size of a tiled raw copy.
fn tiled_len(width: usize, height: usize) -> usize {
    RAW_HEADER + width.div_ceil(TILE) * height.div_ceil(TILE) * TILE_BYTES
}

/// Downloads `file` (`REFERENCE_FILE`, `HEIGHTMAP_FILE`) of `map` into the cache, from the map
/// service and its copy on the calculator's data host at once: the first whole PNG is kept, the
/// other download stops. The cached copy is replaced only once a whole file arrived.
pub fn download(map: &MapInfo, file: &str) -> io::Result<PathBuf> {
    let bytes = download_first(&[MAP_MIRROR, MAP_SERVICE].map(|host| format!("{host}/{}/{file}", map.slug)))?;
    let path = cache_dir().join(&map.slug).join(file);
    std::fs::create_dir_all(path.parent().expect("cache folder"))?;
    let partial = path.with_extension("part");
    std::fs::write(&partial, &bytes)?;
    std::fs::rename(&partial, &path)?;
    Ok(path)
}

/// The first of `urls` to deliver a whole PNG, all fetched at once; the rest stop at their next
/// read once one has it. The error names every URL's failure.
fn download_first(urls: &[String]) -> io::Result<Vec<u8>> {
    let done = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let (tx, rx) = std::sync::mpsc::channel();
    for url in urls {
        let (url, done, tx) = (url.clone(), done.clone(), tx.clone());
        std::thread::Builder::new()
            .name("map-download".into())
            .spawn(move || {
                let _ = tx.send(fetch_png(&url, &done).map_err(|e| format!("{url}: {e}")));
            })
            .map_err(io::Error::other)?;
    }
    drop(tx);
    let mut errors = Vec::new();
    for result in rx {
        match result {
            Ok(bytes) => {
                done.store(true, std::sync::atomic::Ordering::Relaxed);
                return Ok(bytes);
            }
            Err(e) => errors.push(e),
        }
    }
    Err(io::Error::other(errors.join("; ")))
}

/// One whole PNG from `url`, read in chunks so it can give up once `done` is set (another host
/// delivered first).
fn fetch_png(url: &str, done: &std::sync::atomic::AtomicBool) -> io::Result<Vec<u8>> {
    // A stalled connection gives up instead of holding the library back for the session.
    let agent = ureq::AgentBuilder::new().timeout_connect(Duration::from_secs(15)).timeout_read(Duration::from_secs(30)).build();
    let response = agent.get(url).call().map_err(io::Error::other)?;
    let expected: Option<usize> = response.header("Content-Length").and_then(|v| v.parse().ok());
    let mut reader = response.into_reader().take(64 << 20);
    let (mut bytes, mut chunk) = (Vec::new(), vec![0u8; 64 << 10]);
    loop {
        if done.load(std::sync::atomic::Ordering::Relaxed) {
            return Err(io::Error::other("another host delivered it first"));
        }
        match reader.read(&mut chunk)? {
            0 => break,
            n => bytes.extend_from_slice(&chunk[..n]),
        }
    }
    if expected.is_some_and(|n| n != bytes.len()) {
        return Err(io::Error::other(format!("cut short: {} of {} bytes", bytes.len(), expected.unwrap_or(0))));
    }
    if !bytes.starts_with(b"\x89PNG") {
        return Err(io::Error::other("not a PNG"));
    }
    Ok(bytes)
}

/// A map's heightmap: the file's red channel at full resolution, mapped from the raw copy.
#[derive(Clone, Debug)]
pub struct Heightmap {
    /// Size of the heightmap file.
    pub width: u32,
    pub height: u32,
    /// The raw copy (`RAW_HEADER` bytes, then the tiles).
    raw: Arc<Mmap>,
    range: HeightRange,
}

impl Heightmap {
    /// Ground height in metres at normalised map position (`u`, `v`): the heightmap's pixels are
    /// square and its longer side spans the map, so one narrower than the map's square covers the
    /// middle of it, its edge standing in beyond. Rohkshort's (4096 x 2377) is the playable band
    /// between the picture's black grid, Waterloo's (4096 x 2048) and Dustbowl's (2972 x 3442)
    /// likewise; stretched over the whole square as the upstream calculator does, its trees fell on
    /// open fields and a prediction stopped on one.
    pub fn height_m(&self, u: f64, v: f64) -> f64 {
        let (x, y) = pixel(self.width, self.height, u, v);
        let value = f64::from(self.raw[tiled_offset(self.width as usize, x, y)]);
        let studs = self.range.min + (value / 255.0) * (self.range.max - self.range.min);
        studs * METRES_PER_STUD
    }

    /// The mapped raw copy (for the memory report: how much of it is resident).
    pub fn mapped(&self) -> &[u8] {
        &self.raw
    }

    /// Takes the copy's pages read so far out of the process's working set. They stay in the
    /// system's file cache, so reading them again is a soft fault; the working set keeps only the
    /// tiles of the lines read since.
    pub fn release_pages(&self) {
        #[cfg(windows)]
        // Safety: the range is this mapping; unlocking pages that are not locked only takes them
        // out of the working set (documented for `VirtualUnlock`), it fails with ERROR_NOT_LOCKED.
        unsafe {
            let _ = windows::Win32::System::Memory::VirtualUnlock(self.raw.as_ptr().cast(), self.raw.len());
        }
    }

    /// One step of the heightmap's 256 levels, in metres: how far off a height read from it can be.
    pub fn step_m(&self) -> f64 {
        (self.range.max - self.range.min) / 255.0 * METRES_PER_STUD
    }
}

/// The pixel of a `width` x `height` heightmap under normalised map position (`u`, `v`): square
/// pixels, the longer side over the whole map, the shorter one centred (`Heightmap::height_m`).
fn pixel(width: u32, height: u32, u: f64, v: f64) -> (usize, usize) {
    let side = f64::from(width.max(height));
    let at = |t: f64, n: u32| ((t * side - (side - f64::from(n)) / 2.0).round() as i64).clamp(0, i64::from(n) - 1) as usize;
    (at(u, width), at(v, height))
}

/// The cached heightmap of a map, `None` for maps without one. The first load makes the raw copy
/// (Fulvia Gap: about a second); later loads only map it.
pub fn load_heightmap(map: &MapInfo) -> io::Result<Option<Heightmap>> {
    load_layer(map, &heightmap_path(map), HEIGHTMAP_RAW_FILE)
}

/// The map's terrain layer (`TERRAIN_FILE`) when it is in the cache, read as the heightmap is.
pub fn load_terrain(map: &MapInfo) -> io::Result<Option<Heightmap>> {
    let png = heightmap_path(map).with_file_name(TERRAIN_FILE);
    if !png.is_file() {
        return Ok(None);
    }
    load_layer(map, &png, TERRAIN_RAW_FILE)
}

/// Where a map's terrain layer stands after `fetch_terrain`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TerrainStatus {
    /// In the cache, published for the cached heightmap: `load_terrain` reads it.
    Ready,
    /// None is published for the cached heightmap (the map changed and the layer is not made yet,
    /// or the map has none); a layer in the cache is not to be used.
    Unpublished,
    /// The service could not be reached and the cache has no layer known to fit; worth trying again.
    Unreachable(String),
}

#[derive(Debug, Deserialize, serde::Serialize)]
struct TerrainManifest {
    maps: HashMap<String, TerrainEntry>,
}

#[derive(Debug, Deserialize, serde::Serialize)]
struct TerrainEntry {
    /// The heightmap the layer was computed from.
    heightmap_sha256: String,
    terrain_sha256: String,
}

/// Makes the cache hold the terrain layer published for the map's cached heightmap: reads the
/// manifest (the cached copy when the service cannot be reached), and downloads the layer when the
/// cached one is missing or another. A layer is only ever used with the heightmap it was made from.
pub fn fetch_terrain(map: &MapInfo) -> TerrainStatus {
    fetch_terrain_in(&cache_dir(), TERRAIN_SERVICE, &map.slug)
}

fn fetch_terrain_in(cache: &Path, service: &str, slug: &str) -> TerrainStatus {
    let manifest_path = cache.join(TERRAIN_MANIFEST_FILE);
    let (manifest, unreachable) = match download_terrain_manifest(service, &manifest_path) {
        Ok(m) => (Some(m), None),
        Err(e) => (std::fs::read(&manifest_path).ok().and_then(|b| serde_json::from_slice::<TerrainManifest>(&b).ok()), Some(e.to_string())),
    };
    let Some(manifest) = manifest else {
        return TerrainStatus::Unreachable(unreachable.unwrap_or_default());
    };
    let Some(entry) = manifest.maps.get(slug) else { return TerrainStatus::Unpublished };
    let folder = cache.join(slug);
    match sha256_file(&folder.join(HEIGHTMAP_FILE)) {
        Ok(sha) if sha == entry.heightmap_sha256 => {}
        _ => return TerrainStatus::Unpublished,
    }
    let png = folder.join(TERRAIN_FILE);
    if sha256_file(&png).is_ok_and(|sha| sha == entry.terrain_sha256) {
        return TerrainStatus::Ready;
    }
    if let Some(e) = unreachable {
        return TerrainStatus::Unreachable(e);
    }
    match download_terrain(&format!("{service}/{slug}.png"), &entry.terrain_sha256, &png) {
        Ok(()) => TerrainStatus::Ready,
        Err(e) => TerrainStatus::Unreachable(e.to_string()),
    }
}

fn service_agent() -> ureq::Agent {
    ureq::AgentBuilder::new().timeout_connect(Duration::from_secs(15)).timeout_read(Duration::from_secs(30)).build()
}

fn download_terrain_manifest(service: &str, path: &Path) -> io::Result<TerrainManifest> {
    let mut bytes = Vec::new();
    service_agent().get(&format!("{service}/manifest.json")).call().map_err(io::Error::other)?.into_reader().take(1 << 20).read_to_end(&mut bytes)?;
    let manifest = serde_json::from_slice::<TerrainManifest>(&bytes).map_err(io::Error::other)?;
    std::fs::create_dir_all(path.parent().expect("cache folder"))?;
    let partial = path.with_extension("part");
    std::fs::write(&partial, &bytes)?;
    std::fs::rename(&partial, path)?;
    Ok(manifest)
}

/// Downloads the layer and puts it in place only when it is the published one (its raw copy, older
/// now, is rebuilt on the next load).
fn download_terrain(url: &str, sha256: &str, png: &Path) -> io::Result<()> {
    let mut bytes = Vec::new();
    service_agent().get(url).call().map_err(io::Error::other)?.into_reader().take(64 << 20).read_to_end(&mut bytes)?;
    if sha256_hex(&bytes) != sha256 {
        return Err(io::Error::other(format!("{url}: not the published file")));
    }
    let partial = png.with_extension("part");
    std::fs::write(&partial, &bytes)?;
    std::fs::rename(&partial, png)
}

fn sha256_file(path: &Path) -> io::Result<String> {
    let mut context = ring::digest::Context::new(&ring::digest::SHA256);
    let mut file = File::open(path)?;
    let mut buf = vec![0u8; 1 << 16];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        context.update(&buf[..n]);
    }
    Ok(hex(context.finish().as_ref()))
}

fn sha256_hex(bytes: &[u8]) -> String {
    hex(ring::digest::digest(&ring::digest::SHA256, bytes).as_ref())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn load_layer(map: &MapInfo, png: &Path, raw_name: &str) -> io::Result<Option<Heightmap>> {
    let Some(range) = map.heightmap.clone() else { return Ok(None) };
    let png = png.to_path_buf();
    let raw = png.with_file_name(raw_name);
    let (file, width, height) = match open_raw(&raw, &png)? {
        Some(opened) => opened,
        None => write_raw(&png, &raw)?,
    };
    // Safety: the copy is only ever written whole under another name and renamed into place.
    let raw = unsafe { Mmap::map(&file)? };
    Ok(Some(Heightmap { width, height, raw: Arc::new(raw), range }))
}

/// The raw copy, when it is whole and not older than the PNG.
fn open_raw(raw: &Path, png: &Path) -> io::Result<Option<(File, u32, u32)>> {
    let Ok(mut file) = File::open(raw) else { return Ok(None) };
    let meta = file.metadata()?;
    if meta.modified()? < std::fs::metadata(png)?.modified()? {
        return Ok(None);
    }
    let mut header = [0u8; 12];
    if file.read_exact(&mut header).is_err() || &header[..4] != RAW_MAGIC {
        return Ok(None);
    }
    let word = |k: usize| u32::from_le_bytes([header[k], header[k + 1], header[k + 2], header[k + 3]]);
    let (width, height) = (word(4), word(8));
    Ok((meta.len() == tiled_len(width as usize, height as usize) as u64).then_some((file, width, height)))
}

/// Decodes the PNG's red channel row by row into the raw copy.
fn write_raw(png: &Path, raw: &Path) -> io::Result<(File, u32, u32)> {
    let mut decoder = png::Decoder::new(BufReader::new(File::open(png)?));
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = decoder.read_info().map_err(io::Error::other)?;
    let (width, height) = (reader.info().width, reader.info().height);
    let channels = reader.output_color_type().0.samples();
    let part = raw.with_extension(format!("{}.part", std::process::id()));
    let mut out = BufWriter::new(File::create(&part)?);
    let mut header = [0u8; RAW_HEADER];
    header[..4].copy_from_slice(RAW_MAGIC);
    header[4..8].copy_from_slice(&width.to_le_bytes());
    header[8..12].copy_from_slice(&height.to_le_bytes());
    out.write_all(&header)?;
    // A band of `TILE` rows at a time, written out as its row of tiles.
    let tiles_x = (width as usize).div_ceil(TILE);
    let mut band = vec![0u8; tiles_x * TILE * TILE];
    let write_band = |band: &[u8], out: &mut BufWriter<File>| -> io::Result<()> {
        for tx in 0..tiles_x {
            for r in 0..TILE {
                let start = r * tiles_x * TILE + tx * TILE;
                out.write_all(&band[start..start + TILE])?;
            }
        }
        Ok(())
    };
    let mut rows = 0u32;
    while let Some(row) = reader.next_row().map_err(io::Error::other)? {
        let r = rows as usize % TILE;
        let line = &mut band[r * tiles_x * TILE..(r + 1) * tiles_x * TILE];
        for (to, p) in line.iter_mut().zip(row.data().chunks_exact(channels)) {
            *to = p[0];
        }
        rows += 1;
        if rows as usize % TILE == 0 {
            write_band(&band, &mut out)?;
        }
    }
    if rows as usize % TILE != 0 {
        band[(rows as usize % TILE) * tiles_x * TILE..].fill(0);
        write_band(&band, &mut out)?;
    }
    out.into_inner().map_err(io::IntoInnerError::into_error)?.sync_all()?;
    if rows != height {
        let _ = std::fs::remove_file(&part);
        return Err(io::Error::other(format!("{}: {rows} of {height} rows", png.display())));
    }
    // An older copy mapped by another process cannot be replaced: the new one is used where it is.
    let path = match std::fs::rename(&part, raw) {
        Ok(()) => raw.to_path_buf(),
        Err(_) => part,
    };
    Ok((File::open(path)?, width, height))
}

/// Row-major RGBA image.
#[derive(Clone, Debug)]
pub struct RgbaImage {
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>,
}

/// The reference image of `map` in the cache, downloaded from the map service first when the cache
/// lacks it. `Ok(true)` when it was downloaded now.
pub fn ensure_reference(map: &MapInfo) -> io::Result<bool> {
    if reference_path(map).is_file() {
        return Ok(false);
    }
    download(map, REFERENCE_FILE).map(|_| true)
}

/// The cached reference image (`user.png`) of a map.
pub fn load_reference(map: &MapInfo) -> io::Result<RgbaImage> {
    decode_png(&reference_path(map))
}

/// The reference the HUD composited from the game's own 1× V1 (the median of frames from several
/// openings), next to the downloaded one.
pub fn game_reference_path(map: &MapInfo) -> PathBuf {
    cache_dir().join(&map.slug).join("game.png")
}

/// The game-made reference of `map`, if one was composited.
pub fn load_game_reference(map: &MapInfo) -> Option<RgbaImage> {
    let path = game_reference_path(map);
    path.exists().then(|| decode_png(&path).ok()).flatten()
}

/// Saves a game-made reference (RGB, `side`²) for `map`.
pub fn save_game_reference(map: &MapInfo, side: u32, rgb: &[u8]) -> io::Result<PathBuf> {
    let path = game_reference_path(map);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut encoder = png::Encoder::new(io::BufWriter::new(File::create(&path)?), side, side);
    encoder.set_color(png::ColorType::Rgb);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.write_header().map_err(io::Error::other)?.write_image_data(rgb).map_err(io::Error::other)?;
    Ok(path)
}

/// Deletes the game-made reference of `map` (the game's picture changed).
pub fn remove_game_reference(map: &MapInfo) -> io::Result<()> {
    std::fs::remove_file(game_reference_path(map))
}

fn decode_png(path: &Path) -> io::Result<RgbaImage> {
    let mut decoder = png::Decoder::new(BufReader::new(File::open(path)?));
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = decoder.read_info().map_err(io::Error::other)?;
    let mut buffer = vec![0; reader.output_buffer_size().ok_or_else(|| io::Error::other("image too large"))?];
    let info = reader.next_frame(&mut buffer).map_err(io::Error::other)?;
    buffer.truncate(info.buffer_size());
    let data = match info.color_type.samples() {
        4 => buffer,
        3 => buffer.as_chunks::<3>().0.iter().flat_map(|p| [p[0], p[1], p[2], 255]).collect(),
        2 => buffer.as_chunks::<2>().0.iter().flat_map(|p| [p[0], p[0], p[0], p[1]]).collect(),
        1 => buffer.iter().flat_map(|&v| [v, v, v, 255]).collect(),
        _ => return Err(io::Error::other("unsupported PNG layout")),
    };
    Ok(RgbaImage {
        width: info.width,
        height: info.height,
        data,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_loads_with_twelve_rotation_maps() {
        assert_eq!(all().len(), 25);
        assert_eq!(all().iter().filter(|m| m.in_rotation).count(), 12);
        let fulvia = by_id("fulvia_gap").unwrap();
        assert_eq!(fulvia.slug, "fulvia-gap");
        assert!((fulvia.size_m() - 2877.12).abs() < 0.01);
    }

    #[test]
    fn every_rotation_map_has_its_objectives() {
        for map in all().iter().filter(|m| m.in_rotation) {
            let objectives = objectives(&map.id);
            assert!(objectives.len() >= 3, "{}: {} objectives", map.id, objectives.len());
            for (k, o) in objectives.iter().enumerate() {
                assert_eq!(o.letter, char::from(b'A' + k as u8).to_string(), "{}: letters run A, B, C…", map.id);
                assert!((0.0..=1.0).contains(&o.u) && (0.0..=1.0).contains(&o.v), "{} {}", map.id, o.letter);
            }
        }
        assert_eq!(objectives("rohkstov").len(), 6);
        assert!(objectives("dustbowl").is_empty());
    }

    #[test]
    fn the_tiled_raw_copy_holds_every_pixel() {
        // Odd sizes, so the last tiles of a row and the last band are partial.
        let (width, height) = (150u32, 70u32);
        let value = |x: u32, y: u32| ((x * 7 + y * 13) % 251) as u8;
        let dir = std::env::temp_dir().join(format!("mtc-tiles-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let png_path = dir.join("h.png");
        {
            let mut encoder = png::Encoder::new(BufWriter::new(File::create(&png_path).expect("png")), width, height);
            encoder.set_color(png::ColorType::Rgb);
            encoder.set_depth(png::BitDepth::Eight);
            let data: Vec<u8> = (0..height).flat_map(|y| (0..width).flat_map(move |x| [value(x, y), 9, 9])).collect();
            encoder.write_header().expect("header").write_image_data(&data).expect("data");
        }
        let (file, w, h) = write_raw(&png_path, &dir.join("h.r8")).expect("raw copy");
        assert_eq!((w, h), (width, height));
        let raw = unsafe { Mmap::map(&file).expect("map") };
        assert_eq!(raw.len(), tiled_len(150, 70));
        assert_eq!(raw.len(), RAW_HEADER + 3 * 2 * TILE_BYTES);
        for y in 0..height {
            for x in 0..width {
                assert_eq!(raw[tiled_offset(150, x as usize, y as usize)], value(x, y), "({x}, {y})");
            }
        }
        drop(raw);
        drop(file);
        let reopened = open_raw(&dir.join("h.r8"), &png_path).expect("open").expect("whole copy");
        assert_eq!((reopened.1, reopened.2), (width, height));
        drop(reopened);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_heightmap_narrower_than_the_map_covers_its_middle() {
        // Square: the whole map, as before.
        assert_eq!(pixel(4096, 4096, 0.5, 0.25), (2048, 1024));
        // Rohkshort, 4096 x 2377: the full width, the middle 58 % of the height.
        assert_eq!(pixel(4096, 2377, 0.25, 0.5), (1024, 1189));
        assert_eq!(pixel(4096, 2377, 0.0, 0.2099), (0, 0));
        assert_eq!(pixel(4096, 2377, 1.0, 0.7901), (4095, 2376));
        // Beyond the band: its edge.
        assert_eq!(pixel(4096, 2377, 0.5, 0.05), (2048, 0));
        // Dustbowl, 2972 x 3442: the full height, the middle of the width.
        assert_eq!(pixel(2972, 3442, 0.5, 0.5), (1486, 1721));
        assert_eq!(pixel(2972, 3442, 0.0683, 0.0), (0, 0));
    }

    /// A test service answering `requests` GETs from `files` (path -> body), 404 otherwise.
    fn serve(files: Vec<(&'static str, Vec<u8>)>, requests: usize) -> (String, std::thread::JoinHandle<Vec<String>>) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let server = std::thread::spawn(move || {
            let mut asked = Vec::new();
            for _ in 0..requests {
                let (mut stream, _) = listener.accept().unwrap();
                let mut head = Vec::new();
                let mut byte = [0u8; 1];
                while !head.ends_with(b"\r\n\r\n") && stream.read(&mut byte).unwrap_or(0) == 1 {
                    head.push(byte[0]);
                }
                let path = String::from_utf8_lossy(&head).split_whitespace().nth(1).unwrap_or("").to_owned();
                match files.iter().find(|(p, _)| *p == path) {
                    Some((_, body)) => {
                        let _ = stream.write_all(format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len()).as_bytes());
                        let _ = stream.write_all(body);
                    }
                    None => {
                        let _ = stream.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
                    }
                }
                asked.push(path);
            }
            asked
        });
        (url, server)
    }

    #[test]
    fn a_terrain_layer_is_used_only_with_the_heightmap_it_was_made_from() {
        let cache = std::env::temp_dir().join(format!("mtc-terrain-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&cache);
        let folder = cache.join("hill");
        std::fs::create_dir_all(&folder).unwrap();
        let heightmap = folder.join(HEIGHTMAP_FILE);
        let terrain = folder.join(TERRAIN_FILE);
        std::fs::write(&heightmap, b"heightmap one").unwrap();
        let manifest = |h: &[u8], t: &[u8]| {
            format!(r#"{{"version":1,"maps":{{"hill":{{"heightmap_sha256":"{}","heightmap_md5":"","terrain_sha256":"{}","terrain_bytes":0}}}}}}"#, sha256_hex(h), sha256_hex(t)).into_bytes()
        };
        let offline = "http://127.0.0.1:1";

        // Nothing here and no service: worth trying again.
        assert!(matches!(fetch_terrain_in(&cache, offline, "hill"), TerrainStatus::Unreachable(_)));

        // The service has the layer: downloaded, checked, kept with the manifest.
        let (url, server) = serve(vec![("/manifest.json", manifest(b"heightmap one", b"layer one")), ("/hill.png", b"layer one".to_vec())], 2);
        assert_eq!(fetch_terrain_in(&cache, &url, "hill"), TerrainStatus::Ready);
        assert_eq!(server.join().unwrap(), ["/manifest.json", "/hill.png"]);
        assert_eq!(std::fs::read(&terrain).unwrap(), b"layer one");

        // Offline the kept manifest still vouches for the layer.
        assert_eq!(fetch_terrain_in(&cache, offline, "hill"), TerrainStatus::Ready);

        // Another heightmap: the layer is not for it.
        std::fs::write(&heightmap, b"heightmap two").unwrap();
        assert_eq!(fetch_terrain_in(&cache, offline, "hill"), TerrainStatus::Unpublished);
        std::fs::write(&heightmap, b"heightmap one").unwrap();

        // A file that is not the published one is not put in place.
        let (url, server) = serve(vec![("/manifest.json", manifest(b"heightmap one", b"layer two")), ("/hill.png", b"layer three".to_vec())], 2);
        assert!(matches!(fetch_terrain_in(&cache, &url, "hill"), TerrainStatus::Unreachable(_)));
        server.join().unwrap();
        assert_eq!(std::fs::read(&terrain).unwrap(), b"layer one");

        // A map the manifest does not list.
        let (url, server) = serve(vec![("/manifest.json", manifest(b"heightmap one", b"layer one"))], 1);
        assert_eq!(fetch_terrain_in(&cache, &url, "valley"), TerrainStatus::Unpublished);
        server.join().unwrap();
        let _ = std::fs::remove_dir_all(&cache);
    }
}
