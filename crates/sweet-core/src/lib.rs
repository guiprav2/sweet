//! In-process DOM, Tailwind resolver, and desktop painter.
//!
//! JavaScript does not run inside this crate. A host (Node) mutates the DOM
//! and this crate lays it out and paints it. There is no HTML parser, no CSS
//! stylesheet, no sandbox, and no subprocess.

mod color;
mod dom;
mod error;
mod nerd_glyphs;
mod render;
mod tailwind;
mod text;
mod transform;

pub use dom::{Document, NodeId};
pub use error::SweetError;
pub use render::{
    encode_png, physical_size, render, render_argb, render_display_list, render_xrgb, DisplayList,
    Frame, LayoutBox, Picture, PicturePixels, Quad, RenderOptions, TextField,
};
pub use tailwind::CursorStyle;

impl Document {
    pub fn render(&self, options: &RenderOptions) -> Result<Frame, SweetError> {
        render(self, options)
    }

    pub fn render_png(&self, width: u32, height: u32) -> Result<Vec<u8>, SweetError> {
        let frame = self.render(&RenderOptions {
            width: width as f32,
            height: height as f32,
            scale: 1.0,
            hover: None,
            focus: None,
            time: 0.0,
        })?;
        encode_png(&frame)
    }
}
