//! Wiki data downloaded at run time, as `mtc-maps` downloads the map images: the MTC wiki data
//! service's `shells.json` (the same data the upstream calculator fetches) and the artillery
//! weapon table built from its `vehicles.json` and `placeables.json` (`WeaponTable`), kept in a
//! cache folder on the player's machine and refreshed once a day. A refresh downloads a file only
//! when the service's copy differs from the cached one (by ETag).

use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use crate::{AmmoData, WeaponTable};

/// The wiki data service.
pub const WIKI_DATA: &str = "https://data.multicrew.wiki/production/1.0.0/";
/// A cached copy older than this is refreshed (in the background: the next start uses it).
pub const REFRESH_AFTER: Duration = Duration::from_secs(24 * 3600);

const SHELLS: &str = "shells.json";
const VEHICLES: &str = "vehicles.json";
const PLACEABLES: &str = "placeables.json";
const WEAPONS: &str = "artillery_weapons.json";

/// Where the downloaded data is kept: `MTC_DATA_CACHE`, else `%LOCALAPPDATA%/MTC Calculator/data`.
pub fn cache_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("MTC_DATA_CACHE") {
        return PathBuf::from(dir);
    }
    let base = std::env::var_os("LOCALAPPDATA").expect("LOCALAPPDATA");
    PathBuf::from(base).join("MTC Calculator").join("data")
}

/// The cached `shells.json`.
pub fn shells_path() -> PathBuf {
    cache_dir().join(SHELLS)
}

/// The cached weapon table, built from the downloaded `vehicles.json` and `placeables.json`.
pub fn weapons_path() -> PathBuf {
    cache_dir().join(WEAPONS)
}

/// Whether the cached copy at `path` is missing or older than `REFRESH_AFTER`.
pub fn stale(path: &Path) -> bool {
    std::fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| SystemTime::now().duration_since(t).ok())
        .is_none_or(|age| age > REFRESH_AFTER)
}

/// Downloads `shells.json` into the cache when the service has another one, replacing the cached
/// copy only once the whole file arrived and reads as ammunition data. `Ok(true)`: a new copy.
pub fn download_shells() -> io::Result<bool> {
    refresh(SHELLS, |text| AmmoData::from_json(text).map(drop).map_err(|e| e.to_string()))
}

/// Downloads `vehicles.json` and `placeables.json` when the service has other ones and builds the
/// weapon table from them into the cache. `Ok(true)`: a new table.
pub fn download_weapons() -> io::Result<bool> {
    let json = |text: &str| serde_json::from_str::<serde::de::IgnoredAny>(text).map(drop).map_err(|e| e.to_string());
    let vehicles = refresh(VEHICLES, json)?;
    let placeables = refresh(PLACEABLES, json)?;
    let path = weapons_path();
    if !vehicles && !placeables && path.is_file() {
        touch(&path)?;
        return Ok(false);
    }
    let read = |file: &str| std::fs::read_to_string(cache_dir().join(file));
    let table = WeaponTable::from_wiki(&[&read(VEHICLES)?, &read(PLACEABLES)?]).map_err(io::Error::other)?;
    write_whole(&path, serde_json::to_string_pretty(&table).expect("weapon table").as_bytes())?;
    Ok(true)
}

/// The cached weapon table, when there is one that reads.
pub fn load_weapons() -> Option<WeaponTable> {
    let table: WeaponTable = serde_json::from_str(&std::fs::read_to_string(weapons_path()).ok()?).ok()?;
    // A table built before it listed the vehicles: built again from the downloaded data.
    if table.weapons.iter().all(|w| w.vehicles.is_empty()) {
        let read = |file: &str| std::fs::read_to_string(cache_dir().join(file));
        if let (Ok(vehicles), Ok(placeables)) = (read(VEHICLES), read(PLACEABLES))
            && let Ok(fresh) = WeaponTable::from_wiki(&[&vehicles, &placeables])
        {
            let _ = write_whole(&weapons_path(), serde_json::to_string_pretty(&fresh).expect("weapon table").as_bytes());
            return Some(fresh);
        }
    }
    Some(table)
}

/// Fetches `file` from the service into the cache unless the cached copy is the service's current
/// one (its ETag, kept beside it); `check` decides whether the text arrived whole before it
/// replaces the cached copy. An unchanged copy counts as fresh again.
fn refresh(file: &str, check: impl Fn(&str) -> Result<(), String>) -> io::Result<bool> {
    let path = cache_dir().join(file);
    let etag_path = cache_dir().join(format!("{file}.etag"));
    let url = format!("{WIKI_DATA}{file}");
    // A stalled connection gives up: a first start waits on the shells before the HUD shows.
    let agent = ureq::AgentBuilder::new().timeout_connect(Duration::from_secs(15)).timeout_read(Duration::from_secs(30)).build();
    let mut request = agent.get(&url);
    if let (true, Ok(etag)) = (path.is_file(), std::fs::read_to_string(&etag_path)) {
        request = request.set("If-None-Match", etag.trim());
    }
    let response = request.call().map_err(io::Error::other)?;
    if response.status() == 304 {
        touch(&path)?;
        return Ok(false);
    }
    let etag = response.header("ETag").map(str::to_string);
    let mut text = String::new();
    response.into_reader().take(64 << 20).read_to_string(&mut text)?;
    check(&text).map_err(|e| io::Error::other(format!("{url}: {e}")))?;
    write_whole(&path, text.as_bytes())?;
    match etag {
        Some(etag) => std::fs::write(&etag_path, etag)?,
        None => drop(std::fs::remove_file(&etag_path)),
    }
    Ok(true)
}

/// Writes `bytes` to `path` through a temporary file, so a reader never sees half a file.
fn write_whole(path: &Path, bytes: &[u8]) -> io::Result<()> {
    std::fs::create_dir_all(path.parent().expect("cache folder"))?;
    let partial = path.with_extension(format!("{}.part", std::process::id()));
    std::fs::write(&partial, bytes)?;
    std::fs::rename(&partial, path)
}

/// Marks the cached copy at `path` as checked now.
fn touch(path: &Path) -> io::Result<()> {
    std::fs::File::options().write(true).open(path)?.set_modified(SystemTime::now())
}
