//! Recognizers for MTC's game UI.
//!
//! Every recognizer reads pixels through [`FrameView`], addressed in captured-frame coordinates
//! (the Roblox window's visible bounds, title bar included). Layout constants are expressed in
//! client coordinates and shifted by the client origin, so the same code works whether the
//! frame comes from a full capture, a recorded PNG or a set of GPU read-back regions.

pub mod aim;
pub mod bar;
pub mod layout;
pub mod loading;
pub mod loading_state;
pub mod locate;
pub mod map;
pub mod seats;
pub mod text;
pub mod view;

pub use view::{FrameView, PixelFormat};
