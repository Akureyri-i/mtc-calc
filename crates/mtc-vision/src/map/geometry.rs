//! Map containers and the frame ↔ map transform.
//!
//! Map coordinates are normalised to the reference canvas: (0, 0) is its top-left corner and
//! (1, 1) its bottom-right, x to the right, y down — the coordinates the calculator works in.
//! Frame coordinates are continuous: pixel (i, j) covers [i, i+1) × [j, j+1), so a blob centroid
//! is the mean pixel index plus 0.5.

/// The three places the game draws the map, called V1, V2 and V3 throughout.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MapView {
    /// V1: full map opened with M, zoomable, markers editable.
    Full,
    /// V2: map opened with M while scoped.
    Scope,
    /// V3: resident minimap, top right.
    Mini,
}

/// Square container in client coordinates (2560×1334 client at 150 % scale), fitted to the
/// reference images by gradient correlation over the recorded sessions (V1 on 12 maps; V2 and V3
/// averaged over four maps, since each reference carries its own few-tenths-of-a-pixel offset).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Container {
    pub x: f64,
    pub y: f64,
    pub size: f64,
}

impl MapView {
    pub const ALL: [MapView; 3] = [MapView::Full, MapView::Scope, MapView::Mini];

    pub fn container(self) -> Container {
        match self {
            MapView::Full => Container { x: 839.0, y: 226.0, size: 882.0 },
            MapView::Scope => Container { x: 1873.15, y: 334.74, size: 664.55 },
            MapView::Mini => Container { x: 2183.1, y: 109.85, size: 354.35 },
        }
    }

    /// Integer frame rectangle (x, y, w, h) covering the container.
    pub fn frame_rect(self, client_origin: (i32, i32)) -> (i32, i32, i32, i32) {
        let c = self.container();
        let x0 = (c.x + f64::from(client_origin.0)).floor() as i32;
        let y0 = (c.y + f64::from(client_origin.1)).floor() as i32;
        let x1 = (c.x + c.size + f64::from(client_origin.0)).ceil() as i32;
        let y1 = (c.y + c.size + f64::from(client_origin.1)).ceil() as i32;
        (x0, y0, x1 - x0, y1 - y0)
    }
}

/// Frame ↔ map transform of one view: a scale and an offset, no rotation.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MapTransform {
    pub view: MapView,
    /// Frame coordinate of the container's top-left corner.
    pub left: f64,
    pub top: f64,
    /// Container side in frame pixels.
    pub size: f64,
    /// Content zoom: 1 shows the whole canvas, V1 goes up to 5 in steps of 0.2.
    pub zoom: f64,
    /// Map coordinate drawn at the container's top-left corner.
    pub origin: (f64, f64),
}

impl MapTransform {
    /// Unzoomed view: the container shows the whole canvas.
    pub fn fixed(view: MapView, client_origin: (i32, i32)) -> Self {
        Self::zoomed(view, client_origin, 1.0, (0.0, 0.0))
    }

    pub fn zoomed(view: MapView, client_origin: (i32, i32), zoom: f64, origin: (f64, f64)) -> Self {
        let c = view.container();
        Self {
            view,
            left: c.x + f64::from(client_origin.0),
            top: c.y + f64::from(client_origin.1),
            size: c.size,
            zoom,
            origin,
        }
    }

    /// Frame pixels per unit of map coordinate.
    pub fn scale(&self) -> f64 {
        self.size * self.zoom
    }

    pub fn to_map(&self, x: f64, y: f64) -> (f64, f64) {
        let s = self.scale();
        (self.origin.0 + (x - self.left) / s, self.origin.1 + (y - self.top) / s)
    }

    pub fn to_frame(&self, u: f64, v: f64) -> (f64, f64) {
        let s = self.scale();
        (self.left + (u - self.origin.0) * s, self.top + (v - self.origin.1) * s)
    }

    /// Whether a frame point lies inside the container.
    pub fn contains_frame(&self, x: f64, y: f64) -> bool {
        x >= self.left && y >= self.top && x < self.left + self.size && y < self.top + self.size
    }

    /// Metres per frame pixel on a map whose canvas is `map_size_m` wide.
    pub fn metres_per_px(&self, map_size_m: f64) -> f64 {
        map_size_m / self.scale()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transform_round_trips() {
        let t = MapTransform::zoomed(MapView::Full, (0, 34), 3.2, (0.62303, 0.46196));
        let (u, v) = t.to_map(1000.5, 700.25);
        let (x, y) = t.to_frame(u, v);
        assert!((x - 1000.5).abs() < 1e-9 && (y - 700.25).abs() < 1e-9);
        let fixed = MapTransform::fixed(MapView::Full, (0, 34));
        assert_eq!(fixed.to_map(839.0, 260.0), (0.0, 0.0));
        assert_eq!(fixed.to_map(839.0 + 882.0, 260.0 + 882.0), (1.0, 1.0));
    }
}
