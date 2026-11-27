//! Development: the Ping icon and M box on every frame of the recorded sessions, with the spread
//! (standard deviation, luma levels) of the window each best matched. Cues are read where
//! `CueReader::with_layout` puts them for each frame's display configuration, as `cue_scan` does.
//! One CSV line per frame: session, file, width, height, dpi, labelled view (V1/V2/V3, "none" for
//! a labelled black screen, "-" unlabelled), labelled scoped, ping, ping window spread, ping
//! template spread, M key, M window spread, M template spread, scope black, scope lit, primary.
//!   cargo run --release -p mtc-vision --example cue_contrast [-- <session> ...] > contrast.csv
use std::collections::{BTreeMap, HashMap};
use std::sync::Mutex;

use mtc_dataset::image::RgbImage;
use mtc_dataset::labels::MapView as LabelView;
use mtc_dataset::{Session, labels, sessions};
use mtc_vision::FrameView;
use mtc_vision::layout::{UiLayout, picture_shift, shift_probes};
use mtc_vision::map::CueReader;

fn scan(session: &Session) -> Vec<String> {
    let id = &session.info.id;
    let labelled: HashMap<String, (String, String)> = labels(id)
        .unwrap_or_default()
        .into_iter()
        .map(|l| {
            let view = match (&l.map, &l.scene) {
                (Some(m), _) => match m.view {
                    LabelView::V1 => "V1",
                    LabelView::V2 => "V2",
                    LabelView::V3 => "V3",
                }
                .to_owned(),
                (None, Some(s)) => s.map_view.map_or("none".to_owned(), |v| format!("{v:?}")),
                (None, None) => "-".to_owned(),
            };
            let scoped = l.scene.as_ref().map_or("-".to_owned(), |s| s.scoped.to_string());
            (l.file, (view, scoped))
        })
        .collect();
    let mut readers: BTreeMap<(u32, u32, u32), CueReader> = BTreeMap::new();
    let mut out = Vec::new();
    let mut shift = (0, 0);
    for meta in &session.frames {
        let g = &meta.geometry;
        let (cw, ch) = (g.client.width as u32, g.client.height as u32);
        let Ok(image) = RgbImage::load_png(&session.frame_path(meta)) else { continue };
        let view = FrameView::rgb(&image.data, image.width, image.height);
        let c = g.client_in_frame();
        if image.width > cw {
            let [(bx, by, bw, bh), (rx, ry, rw, rh)] = shift_probes((image.width, image.height));
            shift = picture_shift(&view.sub(bx, by, bw, bh), &view.sub(rx, ry, rw, rh)).unwrap_or(shift);
        } else {
            shift = (0, 0);
        }
        let origin = (c.x - shift.0, c.y - shift.1);
        let layout = UiLayout { width: cw, height: ch, dpi: g.dpi };
        let reader = readers.entry((g.dpi, cw, ch)).or_insert_with(|| CueReader::with_layout(&layout));
        let views = std::slice::from_ref(&view);
        let cues = reader.read_views(views, origin);
        let (p, ps, pt) = reader.ping_detail(views, origin);
        let (m, ms, mt) = reader.m_key_detail(views, origin);
        let (lv, ls) = labelled.get(&meta.file).cloned().unwrap_or(("-".to_owned(), "-".to_owned()));
        let primary = cues.primary().map_or("-".to_owned(), |v| format!("{v:?}"));
        out.push(format!(
            "{id},{},{cw},{ch},{},{lv},{ls},{:.3},{:.2},{:.2},{:.3},{:.2},{:.2},{:.3},{:.3},{primary}",
            meta.file,
            g.dpi,
            p.max(-9.0),
            ps,
            pt,
            m.max(-9.0),
            ms,
            mt,
            cues.scope_black,
            cues.scope_lit
        ));
    }
    out
}

fn main() {
    let only: Vec<String> = std::env::args().skip(1).collect();
    let all: Vec<Session> = sessions().expect("sessions").into_iter().filter(|s| only.is_empty() || only.contains(&s.info.id)).collect();
    let queue = Mutex::new(all.into_iter());
    let lines = Mutex::new(Vec::new());
    let workers = std::thread::available_parallelism().map_or(4, |n| n.get()).min(8);
    std::thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| loop {
                let Some(session) = queue.lock().unwrap().next() else { break };
                let found = scan(&session);
                eprintln!("{} {} frames", session.info.id, found.len());
                lines.lock().unwrap().extend(found);
            });
        }
    });
    println!("session,file,width,height,dpi,label,label_scoped,ping,ping_spread,ping_tspread,mkey,mkey_spread,mkey_tspread,scope_black,scope_lit,primary");
    for line in lines.into_inner().unwrap() {
        println!("{line}");
    }
}
