//! The files the release program carries inside it (the `bundle` feature): whatever the bundle
//! folder holds of the game's fonts, the wiki's ammunition data and weapon table, and the maps'
//! pictures, heightmaps and terrain layers with the terrain manifest. On start they are written where
//! the program would otherwise download them (the data cache, the map cache, and a fonts folder next
//! to them), each only when missing there, so what is carried needs no download and the data's own
//! updates are kept.

use std::path::PathBuf;

include!(concat!(env!("OUT_DIR"), "/bundle.rs"));

/// Where the bundled fonts are written (`%LOCALAPPDATA%\MTC Calculator\fonts`).
pub fn font_dir() -> Option<PathBuf> {
    Some(PathBuf::from(std::env::var_os("LOCALAPPDATA")?).join("MTC Calculator").join("fonts"))
}

/// Writes every bundled file the cache lacks; logs what it wrote.
pub fn unpack() {
    if FILES.is_empty() {
        return;
    }
    let started = std::time::Instant::now();
    let (mut written, mut bytes) = (0usize, 0usize);
    for (relative, data) in FILES {
        let path = if let Some(rest) = relative.strip_prefix("maps/") {
            mtc_maps::cache_dir().join(rest)
        } else if let Some(rest) = relative.strip_prefix("data/") {
            mtc_ammo::fetch::cache_dir().join(rest)
        } else if let Some(rest) = relative.strip_prefix("fonts/") {
            let Some(dir) = font_dir() else { continue };
            dir.join(rest)
        } else {
            continue;
        };
        if path.is_file() {
            continue;
        }
        let result = (|| {
            std::fs::create_dir_all(path.parent().expect("file in a folder"))?;
            let partial = path.with_extension("part");
            std::fs::write(&partial, data)?;
            std::fs::rename(&partial, &path)
        })();
        match result {
            Ok(()) => {
                written += 1;
                bytes += data.len();
            }
            Err(e) => log::warn!("Bundled file {relative} could not be written to {}: {e}", path.display()),
        }
    }
    if written > 0 {
        log::info!("Bundled data and fonts: {written} files written ({:.0} MB, {:.1} s)", bytes as f64 / 1e6, started.elapsed().as_secs_f64());
    }
}
