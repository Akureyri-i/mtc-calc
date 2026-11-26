//! Charge marks on the game's loading menu: at the right end of each charge row of each
//! card, a green dot when that shell at that charge reaches the selected target, a red one when
//! it does not — known before loading. Dots carry a dark rim so they read on the card's dark rows
//! and on the amber selected row alike. The window is click-through and covers only the cards'
//! charge panels.

use std::rc::Rc;

use windows::Win32::Foundation::HWND;

use crate::render::{Graphics, Surface, rgba};
use crate::window;

/// Largest marked area: four cards' charge panels.
const MAX_WIDTH: i32 = 640;
const MAX_HEIGHT: i32 = 140;
const DOT_R: f32 = 5.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ChargeDot {
    /// Screen position of the dot's centre.
    pub x: f32,
    pub y: f32,
    pub reaches: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ChargeMarks {
    /// Screen rectangle the dots lie in (the charge panels).
    pub left: i32,
    pub top: i32,
    pub width: i32,
    pub height: i32,
    pub dots: Vec<ChargeDot>,
    /// The game HUD's scale (`UiLayout::overlay_scales().1`; 1 at the reference): the dots are
    /// drawn that much smaller with the cards.
    pub scale: f32,
}

pub struct ChargeWindow {
    hwnd: HWND,
    surface: Surface,
    shown: Option<ChargeMarks>,
    placed: Option<(i32, i32, i32, i32)>,
}

impl ChargeWindow {
    pub fn new(graphics: &Rc<Graphics>) -> windows::core::Result<Self> {
        let hwnd = window::create(MAX_WIDTH, MAX_HEIGHT)?;
        let surface = Surface::new(graphics, hwnd, MAX_WIDTH as u32, MAX_HEIGHT as u32)?;
        Ok(Self { hwnd, surface, shown: None, placed: None })
    }

    /// The dots drawn offscreen where they would show.
    pub fn render_offscreen(graphics: &Rc<Graphics>, marks: &ChargeMarks) -> windows::core::Result<crate::Layer> {
        let window = Self { hwnd: HWND::default(), surface: Surface::offscreen(graphics, MAX_WIDTH as u32, MAX_HEIGHT as u32)?, shown: None, placed: None };
        window.draw(marks)?;
        let (w, h) = (marks.width.min(MAX_WIDTH) as usize, marks.height.min(MAX_HEIGHT) as usize);
        let full = window.surface.pixels()?;
        let row = MAX_WIDTH as usize * 4;
        let bgra = (0..h).flat_map(|y| full[y * row..y * row + w * 4].iter().copied()).collect();
        Ok(crate::Layer { x: marks.left, y: marks.top, width: w as u32, height: h as u32, bgra })
    }

    /// The surface made again after a device loss (`Graphics::recover`), the shown dots redrawn.
    pub fn rebuild(&mut self) -> windows::core::Result<()> {
        self.surface.rebuild()?;
        match self.shown.take() {
            Some(shown) => self.show(Some(&shown)),
            None => Ok(()),
        }
    }

    pub fn show(&mut self, marks: Option<&ChargeMarks>) -> windows::core::Result<()> {
        let Some(marks) = marks else {
            if self.shown.take().is_some() {
                window::hide(self.hwnd);
                self.placed = None;
            }
            return Ok(());
        };
        if self.shown.as_ref() == Some(marks) {
            return Ok(());
        }
        self.draw(marks)?;
        let at = (marks.left, marks.top, marks.width.min(MAX_WIDTH), marks.height.min(MAX_HEIGHT));
        if self.placed != Some(at) {
            window::place_sized(self.hwnd, at.0, at.1, at.2, at.3);
            self.placed = Some(at);
        }
        self.shown = Some(marks.clone());
        Ok(())
    }

    fn draw(&self, marks: &ChargeMarks) -> windows::core::Result<()> {
        let s = &self.surface;
        let green = rgba(118, 214, 128, 1.0);
        let red = rgba(232, 62, 50, 1.0);
        let rim = rgba(0, 0, 0, 0.6);
        s.begin();
        for d in &marks.dots {
            let (x, y) = (d.x - marks.left as f32, d.y - marks.top as f32);
            s.circle(x, y, (DOT_R + 1.0) * marks.scale, rim);
            s.circle(x, y, DOT_R * marks.scale, if d.reaches { green } else { red });
        }
        s.end()
    }
}
