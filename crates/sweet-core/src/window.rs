/// Edge or corner of a borderless window where pointer presses should resize.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResizeEdge {
    East,
    North,
    NorthEast,
    NorthWest,
    South,
    SouthEast,
    SouthWest,
    West,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ResizeAxes {
    pub horizontal: bool,
    pub vertical: bool,
}

/// Return the active resize edge for a pointer in logical window coordinates.
///
/// Keeping this hit test in Sweet Core lets each host use the same border
/// width and corner precedence while delegating the actual resize to its
/// window system.
pub fn resize_edge(x: f64, y: f64, width: f64, height: f64, edge: f64, axes: ResizeAxes) -> Option<ResizeEdge> {
    if !x.is_finite() || !y.is_finite() || !width.is_finite() || !height.is_finite() || edge <= 0.0 {
        return None;
    }
    let west = x < edge;
    let east = x >= width - edge;
    let north = y < edge;
    let south = y >= height - edge;
    let edge = match (west, east, north, south) {
        (true, _, true, _) => ResizeEdge::NorthWest,
        (_, true, true, _) => ResizeEdge::NorthEast,
        (true, _, _, true) => ResizeEdge::SouthWest,
        (_, true, _, true) => ResizeEdge::SouthEast,
        (true, _, _, _) => ResizeEdge::West,
        (_, true, _, _) => ResizeEdge::East,
        (_, _, true, _) => ResizeEdge::North,
        (_, _, _, true) => ResizeEdge::South,
        _ => return None,
    };
    match (axes.horizontal, axes.vertical, edge) {
        (false, false, _) => None,
        (false, true, ResizeEdge::NorthWest | ResizeEdge::NorthEast) => Some(ResizeEdge::North),
        (false, true, ResizeEdge::SouthWest | ResizeEdge::SouthEast) => Some(ResizeEdge::South),
        (false, true, ResizeEdge::West | ResizeEdge::East) => None,
        (true, false, ResizeEdge::NorthWest | ResizeEdge::SouthWest) => Some(ResizeEdge::West),
        (true, false, ResizeEdge::NorthEast | ResizeEdge::SouthEast) => Some(ResizeEdge::East),
        (true, false, ResizeEdge::North | ResizeEdge::South) => None,
        _ => Some(edge),
    }
}
