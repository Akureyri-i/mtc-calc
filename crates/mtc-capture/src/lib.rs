//! Capture foundation shared by the recorder and the HUD.
//!
//! - [`device`]: the Direct3D device the capture runs on, described for the log.
//! - [`window`]: finds the Roblox player window and reads its geometry in physical pixels.
//! - [`readback`]: copies Windows Graphics Capture frame textures to CPU memory through
//!   persistent staging textures.
//!
//! All coordinates are physical pixels. The process must call
//! [`window::enable_per_monitor_dpi_awareness`] before touching any window, otherwise Windows
//! reports DPI-virtualised coordinates (at 150% scaling a maximized 2560×1368 window reads as
//! 1714×920).

pub mod device;
pub mod readback;
pub mod window;

pub use windows_capture;
