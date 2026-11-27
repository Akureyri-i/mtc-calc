//! The loading UI's text reader for the display configuration on screen: at the reference the
//! 150 % glyph asset; elsewhere templates drawn as the game draws its small UI text
//! (`mtc_overlay::render_ui_glyphs`: DirectWrite without grid fitting, at the configuration's font
//! sizes from `UiLayout::ui_text_px`, resampled by 1 / the HUD scale into the reference geometry
//! the loading UI is read in). With Roblox's text size raised (`set_factor`, from the angle
//! readout's fit) every size is that many times larger, drawn so at the reference too. Drawing
//! them makes its own graphics device (about half a second), so it runs on a thread of its own once
//! the configuration has held for `STABLE` (a window dragged to a new size passes through many);
//! the loading UI is not read at a configuration until its reader is in.

use std::path::PathBuf;
use std::sync::mpsc::{Receiver, TryRecvError, channel};
use std::time::{Duration, Instant};

use mtc_overlay::{UI_CHARS, UiGlyphStyle, render_ui_glyphs};
use mtc_vision::layout::UiLayout;
use mtc_vision::text::{GlyphImage, GlyphStyle, TextReader};

/// How long a configuration holds before its templates are drawn.
const STABLE: Duration = Duration::from_millis(300);

/// DirectWrite weights of the weapon label, card name and card value (Regular, Medium, Regular).
const WEIGHTS: [i32; 3] = [400, 500, 400];

/// A configuration and the text size multiple its templates are drawn at.
type Key = (UiLayout, u32);

/// The text size multiple in thousandths (a key that compares exactly).
fn milli(factor: f32) -> u32 {
    (factor * 1000.0).round() as u32
}

pub struct UiText {
    font_dir: PathBuf,
    /// Roblox's text size multiple (1: the default), from the angle readout's fit.
    factor: f32,
    /// The configuration and multiple the reader last handed out is for.
    ready: Option<Key>,
    /// The configuration on screen and since when.
    seen: Option<(UiLayout, Instant)>,
    /// Templates being drawn, for this configuration and multiple.
    job: Option<(Key, Receiver<Result<TextReader, String>>)>,
    /// A configuration and multiple whose templates could not be drawn: not tried again.
    failed: Option<Key>,
}

impl UiText {
    pub fn new(font_dir: PathBuf) -> Self {
        Self { font_dir, factor: 1.0, ready: None, seen: None, job: None, failed: None }
    }

    /// The game's text this many times its default size (Roblox's text size raised); templates are
    /// drawn again for it.
    pub fn set_factor(&mut self, factor: f32) {
        self.factor = factor;
    }

    /// Whether the reader handed out is for `layout`.
    pub fn ready_for(&self, layout: UiLayout) -> bool {
        self.ready.is_some_and(|(l, _)| l == layout)
    }

    /// Called every frame with the configuration on screen: a reader for it when one is ready
    /// and not yet handed out.
    pub fn poll(&mut self, layout: UiLayout) -> Option<TextReader> {
        let key = (layout, milli(self.factor));
        if self.ready == Some(key) {
            return None;
        }
        if layout.is_reference() && key.1 == 1000 {
            self.ready = Some(key);
            return Some(TextReader::new());
        }
        if self.seen.is_none_or(|(l, _)| l != layout) {
            self.seen = Some((layout, Instant::now()));
        }
        if let Some((for_key, rx)) = &self.job {
            let for_key = *for_key;
            match rx.try_recv() {
                Err(TryRecvError::Empty) => return None,
                Ok(Ok(reader)) => {
                    self.job = None;
                    if for_key == key {
                        log::info!("UI text templates for {} ready: the loading UI is read now", describe(key));
                        self.ready = Some(key);
                        return Some(reader);
                    }
                }
                Ok(Err(e)) => {
                    self.job = None;
                    log::info!("UI text templates could not be made ({}): {e}; the loading UI is not read at this configuration", describe(for_key));
                    self.failed = Some(for_key);
                }
                Err(TryRecvError::Disconnected) => self.job = None,
            }
        }
        let held = self.seen.is_some_and(|(_, since)| since.elapsed() >= STABLE);
        if self.job.is_none() && held && self.failed != Some(key) {
            let (tx, rx) = channel();
            let (font_dir, factor) = (self.font_dir.clone(), self.factor);
            std::thread::spawn(move || {
                let _ = tx.send(reader_for(&font_dir, layout, factor));
            });
            self.job = Some((key, rx));
        }
        None
    }
}

fn describe((l, milli): Key) -> String {
    let size = if milli == 1000 { String::new() } else { format!(", text {:.2} times its default size", f64::from(milli) / 1000.0) };
    format!("{}x{}, {}%{size}", l.width, l.height, l.dpi * 100 / 96)
}

/// The text reader drawn for a configuration, the game's text `factor` times its default size.
pub fn reader_for(font_dir: &std::path::Path, layout: UiLayout, factor: f32) -> Result<TextReader, String> {
    let px = layout.ui_text_px().map(|p| p * factor);
    let styles = std::array::from_fn(|i| (WEIGHTS[i], px[i]));
    let drawn = render_ui_glyphs(font_dir, styles, 1.0 / layout.hud_scale() as f32, UI_CHARS).map_err(|e| e.to_string())?;
    Ok(TextReader::from_styles(drawn.map(glyph_style)))
}

fn glyph_style(s: UiGlyphStyle) -> GlyphStyle {
    GlyphStyle { space: s.space, glyphs: s.glyphs.into_iter().map(|g| GlyphImage { ch: g.ch, width: g.width, height: g.height, top: g.top, pixels: g.pixels }).collect() }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Needs Montserrat from a Roblox install on this machine (`roblox_font_dir`).
    #[test]
    fn a_configuration_gets_its_reader_once_it_holds() {
        let mut text = UiText::new(crate::roblox_font_dir());
        assert!(text.poll(UiLayout::REFERENCE).is_some(), "the reference's asset at once");
        assert!(text.poll(UiLayout::REFERENCE).is_none(), "handed out once");
        let other = UiLayout { width: 1920, height: 840, dpi: 120 };
        let started = Instant::now();
        let reader = loop {
            if let Some(r) = text.poll(other) {
                break r;
            }
            assert!(!text.ready_for(other));
            assert!(started.elapsed() < Duration::from_secs(10), "templates not drawn in 10 s");
            std::thread::sleep(Duration::from_millis(20));
        };
        assert!(started.elapsed() >= STABLE, "drawn only once the configuration held");
        assert!(text.ready_for(other));
        assert!(text.poll(other).is_none());
        drop(reader);
    }
}
