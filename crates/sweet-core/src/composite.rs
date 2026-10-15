//! Display items for the GPU compositor.
//!
//! Servo keeps painting and compositing apart: a layer is rasterized when its
//! pixels change, and later frames only update the transform the GPU applies.
//! Sweet follows that split. `render` still returns a correct CPU picture for
//! tests and PNG export. Native windows ask for [`CompositeFrame`], skip the
//! CPU blit of animated layers, and hand those pixmaps to wgpu.

use crate::dom::NodeId;

/// One textured quad. Coordinates are CSS pixels in the same space as layout.
#[derive(Clone, Debug)]
pub struct CompositeLayer {
    pub id: NodeId,
    /// Premultiplied RGBA8, row-major, origin at the top left.
    pub rgba: Vec<u8>,
    pub width: u32,
    pub height: u32,
    /// Content change. The presenter uploads only when this differs.
    pub stamp: u64,
    /// Layer-local point that maps to `corners[0]` before the transform.
    pub min_x: f32,
    pub min_y: f32,
    pub logical_w: f32,
    pub logical_h: f32,
    /// Screen-space corners of the layer bounds: top-left, top-right,
    /// bottom-right, bottom-left. Perspective is already applied.
    pub corners: [(f32, f32); 4],
    /// Axis-aligned clip in CSS pixels: x, y, w, h. Zero size means no clip.
    pub clip: [f32; 4],
    /// Rounded clip: x, y, w, h, then four corner radii. Zeros when square.
    pub round: [f32; 8],
    /// Extra opacity already separated from the baked pixels.
    pub opacity: f32,
    /// Painter's order. Larger values are closer to the viewer.
    pub order: u32,
    /// Static content painted after this layer. Empty when nothing covers it.
    pub overlay: Vec<u8>,
    pub overlay_w: u32,
    pub overlay_h: u32,
    /// Physical origin of `overlay` inside the static frame.
    pub overlay_x: i32,
    pub overlay_y: i32,
}

/// Static picture plus the layers the GPU should move on top of it.
#[derive(Clone, Debug)]
pub struct CompositeFrame {
    pub width: u32,
    pub height: u32,
    /// Premultiplied `0xAARRGGBB` pixels of everything that is not a GPU layer.
    pub pixels: Vec<u32>,
    pub layers: Vec<CompositeLayer>,
}
