//! Development: the loading UI's glyph templates for display configurations,
//! drawn as `mtc_overlay::render_ui_glyphs` draws them, one glyph file (the asset's format) per
//! configuration, for `mtc-vision`'s `layout_loading` (`MTC_GLYPHS`):
//!   ui_glyphs_json <out dir> <dpi,width,height[:weapon,name,value px]> [...]
//! Font sizes: `UiLayout::ui_text_px` (weapon label Regular, card names Medium, card values
//! Regular); resampled by 1 / the HUD scale.
use mtc_overlay::{UI_CHARS, render_ui_glyphs};
use mtc_vision::layout::UiLayout;

fn roblox_font_dir() -> std::path::PathBuf {
    let versions = std::path::PathBuf::from(std::env::var("LOCALAPPDATA").expect("LOCALAPPDATA")).join("Roblox/Versions");
    std::fs::read_dir(versions)
        .expect("Roblox versions")
        .filter_map(Result::ok)
        .map(|e| e.path().join("content/fonts"))
        .filter(|p| p.join("Montserrat-Medium.ttf").is_file())
        .max_by_key(|p| std::fs::metadata(p).and_then(|m| m.modified()).ok())
        .expect("Montserrat in a Roblox install")
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let dir = std::path::PathBuf::from(&args[0]);
    std::fs::create_dir_all(&dir).expect("out dir");
    let fonts = roblox_font_dir();
    for spec in &args[1..] {
        // `dpi,width,height[:weapon,name,value]`: the text px from the table, or as given.
        let (config, px) = spec.split_once(':').map_or((spec.as_str(), None), |(c, p)| (c, Some(p)));
        let n: Vec<u32> = config.split(',').map(|v| v.parse().expect("dpi,width,height")).collect();
        let layout = UiLayout { width: n[1], height: n[2], dpi: n[0] };
        let k = px.map_or_else(|| layout.ui_text_px(), |p| {
            let v: Vec<f32> = p.split(',').map(|x| x.parse().expect("weapon,name,value px")).collect();
            [v[0], v[1], v[2]]
        });
        let upscale = 1.0 / layout.hud_scale() as f32;
        let styles = render_ui_glyphs(&fonts, [(400, k[0]), (500, k[1]), (400, k[2])], upscale, UI_CHARS).expect("render");
        let style_json = |s: &mtc_overlay::UiGlyphStyle| {
            let glyphs: Vec<String> = s
                .glyphs
                .iter()
                .map(|g| {
                    let hex: String = g.pixels.iter().map(|v| format!("{v:02x}")).collect();
                    // UI_CHARS holds no quote or backslash to escape.
                    let ch = g.ch;
                    format!("{{\"ch\":\"{ch}\",\"width\":{},\"height\":{},\"top\":{},\"pixels\":\"{hex}\"}}", g.width, g.height, g.top)
                })
                .collect();
            format!("{{\"space\":{},\"glyphs\":[{}]}}", s.space, glyphs.join(","))
        };
        let json = format!("{{\"styles\":{{\"weapon\":{},\"name\":{},\"value\":{}}}}}", style_json(&styles[0]), style_json(&styles[1]), style_json(&styles[2]));
        let path = dir.join(format!("{}_{}_{}.json", n[0], n[1], n[2]));
        std::fs::write(&path, json).expect("write");
        println!("{spec}: text px {k:?}, upscale {upscale:.3}, {} / {} / {} glyphs -> {}", styles[0].glyphs.len(), styles[1].glyphs.len(), styles[2].glyphs.len(), path.display());
    }
}
