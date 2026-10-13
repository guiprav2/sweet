use std::collections::HashMap;
use std::sync::LazyLock;

include!("nerd_glyph_map.rs");

static LOOKUP: LazyLock<HashMap<&'static str, char>> =
    LazyLock::new(|| GLYPHS.iter().copied().collect());

pub fn glyph(class: &str) -> Option<char> {
    LOOKUP.get(class).copied()
}
