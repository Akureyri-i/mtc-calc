//! Map views: which views are on screen, which map is loaded, and where things are on it.

pub mod arrow;
pub mod canvas;
pub mod cues;
pub mod geometry;
pub mod ident;
pub mod impacts;
pub mod labels;
pub mod markers;
pub mod objectives;
pub mod raster;
pub mod reference;
pub mod targets;
pub mod zoom;

pub use cues::{CueReader, ViewCues};
pub use geometry::{MapTransform, MapView};
pub use ident::{MapIdentifier, MapMatch, MapVote, THUMBNAIL_FORMAT, Thumbnail};
pub use objectives::{Objective, ObjectiveReader, ObjectiveSighting};
pub use reference::MapReference;
