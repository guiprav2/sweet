//! Tailwind default palette. Hex values only — there is no CSS color parser.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rgba {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl Rgba {
    pub const TRANSPARENT: Self = Self {
        r: 0,
        g: 0,
        b: 0,
        a: 0,
    };

    pub const fn rgb(hex: u32) -> Self {
        Self {
            r: ((hex >> 16) & 0xff) as u8,
            g: ((hex >> 8) & 0xff) as u8,
            b: (hex & 0xff) as u8,
            a: 255,
        }
    }

    pub fn with_alpha(self, alpha: f32) -> Self {
        let alpha = alpha.clamp(0.0, 1.0);
        Self {
            a: (self.a as f32 * alpha).round() as u8,
            ..self
        }
    }
}

const SHADES: [u16; 11] = [50, 100, 200, 300, 400, 500, 600, 700, 800, 900, 950];

const NAMES: &[&str] = &[
    "slate", "gray", "zinc", "neutral", "stone", "red", "orange", "amber", "yellow", "lime",
    "green", "emerald", "teal", "cyan", "sky", "blue", "indigo", "violet", "purple", "fuchsia",
    "pink", "rose",
];

#[rustfmt::skip]
const COLORS: [[u32; 11]; 22] = [
    [0xf8fafc, 0xf1f5f9, 0xe2e8f0, 0xcbd5e1, 0x94a3b8, 0x64748b, 0x475569, 0x334155, 0x1e293b, 0x0f172a, 0x020617],
    [0xf9fafb, 0xf3f4f6, 0xe5e7eb, 0xd1d5db, 0x9ca3af, 0x6b7280, 0x4b5563, 0x374151, 0x1f2937, 0x111827, 0x030712],
    [0xfafafa, 0xf4f4f5, 0xe4e4e7, 0xd4d4d8, 0xa1a1aa, 0x71717a, 0x52525b, 0x3f3f46, 0x27272a, 0x18181b, 0x09090b],
    [0xfafafa, 0xf5f5f5, 0xe5e5e5, 0xd4d4d4, 0xa3a3a3, 0x737373, 0x525252, 0x404040, 0x262626, 0x171717, 0x0a0a0a],
    [0xfafaf9, 0xf5f5f4, 0xe7e5e4, 0xd6d3d1, 0xa8a29e, 0x78716c, 0x57534e, 0x44403c, 0x292524, 0x1c1917, 0x0c0a09],
    [0xfef2f2, 0xfee2e2, 0xfecaca, 0xfca5a5, 0xf87171, 0xef4444, 0xdc2626, 0xb91c1c, 0x991b1b, 0x7f1d1d, 0x450a0a],
    [0xfff7ed, 0xffedd5, 0xfed7aa, 0xfdba74, 0xfb923c, 0xf97316, 0xea580c, 0xc2410c, 0x9a3412, 0x7c2d12, 0x431407],
    [0xfffbeb, 0xfef3c7, 0xfde68a, 0xfcd34d, 0xfbbf24, 0xf59e0b, 0xd97706, 0xb45309, 0x92400e, 0x78350f, 0x451a03],
    [0xfefce8, 0xfef9c3, 0xfef08a, 0xfde047, 0xfacc15, 0xeab308, 0xca8a04, 0xa16207, 0x854d0e, 0x713f12, 0x422006],
    [0xf7fee7, 0xecfccb, 0xd9f99d, 0xbef264, 0xa3e635, 0x84cc16, 0x65a30d, 0x4d7c0f, 0x3f6212, 0x365314, 0x1a2e05],
    [0xf0fdf4, 0xdcfce7, 0xbbf7d0, 0x86efac, 0x4ade80, 0x22c55e, 0x16a34a, 0x15803d, 0x166534, 0x14532d, 0x052e16],
    [0xecfdf5, 0xd1fae5, 0xa7f3d0, 0x6ee7b7, 0x34d399, 0x10b981, 0x059669, 0x047857, 0x065f46, 0x064e3b, 0x022c22],
    [0xf0fdfa, 0xccfbf1, 0x99f6e4, 0x5eead4, 0x2dd4bf, 0x14b8a6, 0x0d9488, 0x0f766e, 0x115e59, 0x134e4a, 0x042f2e],
    [0xecfeff, 0xcffafe, 0xa5f3fc, 0x67e8f9, 0x22d3ee, 0x06b6d4, 0x0891b2, 0x0e7490, 0x155e75, 0x164e63, 0x083344],
    [0xf0f9ff, 0xe0f2fe, 0xbae6fd, 0x7dd3fc, 0x38bdf8, 0x0ea5e9, 0x0284c7, 0x0369a1, 0x075985, 0x0c4a6e, 0x082f49],
    [0xeff6ff, 0xdbeafe, 0xbfdbfe, 0x93c5fd, 0x60a5fa, 0x3b82f6, 0x2563eb, 0x1d4ed8, 0x1e40af, 0x1e3a8a, 0x172554],
    [0xeef2ff, 0xe0e7ff, 0xc7d2fe, 0xa5b4fc, 0x818cf8, 0x6366f1, 0x4f46e5, 0x4338ca, 0x3730a3, 0x312e81, 0x1e1b4b],
    [0xf5f3ff, 0xede9fe, 0xddd6fe, 0xc4b5fd, 0xa78bfa, 0x8b5cf6, 0x7c3aed, 0x6d28d9, 0x5b21b6, 0x4c1d95, 0x2e1065],
    [0xfaf5ff, 0xf3e8ff, 0xe9d5ff, 0xd8b4fe, 0xc084fc, 0xa855f7, 0x9333ea, 0x7e22ce, 0x6b21a8, 0x581c87, 0x3b0764],
    [0xfdf4ff, 0xfae8ff, 0xf5d0fe, 0xf0abfc, 0xe879f9, 0xd946ef, 0xc026d3, 0xa21caf, 0x86198f, 0x701a75, 0x4a044e],
    [0xfdf2f8, 0xfce7f3, 0xfbcfe8, 0xf9a8d4, 0xf472b6, 0xec4899, 0xdb2777, 0xbe185d, 0x9d174d, 0x831843, 0x500724],
    [0xfff1f2, 0xffe4e6, 0xfecdd3, 0xfda4af, 0xfb7185, 0xf43f5e, 0xe11d48, 0xbe123c, 0x9f1239, 0x881337, 0x4c0519],
];

pub fn palette(name: &str, shade: u16) -> Option<Rgba> {
    let row = NAMES.iter().position(|n| *n == name)?;
    let col = SHADES.iter().position(|s| *s == shade)?;
    Some(Rgba::rgb(COLORS[row][col]))
}

/// `red-500`, `white`, `transparent`, `[#rgb]`, `[#rrggbb]`, `[#rrggbbaa]`.
pub fn parse_color(value: &str) -> Option<Rgba> {
    let (value, alpha) = split_alpha(value);
    let color = parse_color_body(value)?;
    Some(match alpha {
        Some(alpha) => color.with_alpha(alpha),
        None => color,
    })
}

/// `white/55` and `rose-300/70` are Tailwind opacity modifiers, 0–100.
fn split_alpha(value: &str) -> (&str, Option<f32>) {
    let Some((head, tail)) = value.rsplit_once('/') else {
        return (value, None);
    };
    let Ok(step) = tail.parse::<f32>() else {
        return (value, None);
    };
    (head, Some((step / 100.0).clamp(0.0, 1.0)))
}

fn parse_color_body(value: &str) -> Option<Rgba> {
    match value {
        "white" => return Some(Rgba::rgb(0xffffff)),
        "black" => return Some(Rgba::rgb(0x000000)),
        "transparent" => return Some(Rgba::TRANSPARENT),
        "inherit" | "current" => return None,
        _ => {}
    }
    if let Some(inner) = arbitrary(value) {
        return parse_hex(inner);
    }
    if let Some((name, shade)) = value.rsplit_once('-') {
        if let Ok(shade) = shade.parse::<u16>() {
            return palette(name, shade);
        }
    }
    None
}

pub fn arbitrary(value: &str) -> Option<&str> {
    value.strip_prefix('[').and_then(|v| v.strip_suffix(']'))
}

fn parse_hex(value: &str) -> Option<Rgba> {
    let hex = value.strip_prefix('#')?;
    match hex.len() {
        3 | 4 => {
            let mut nibbles = [0u8; 4];
            let chars: Vec<u8> = hex.bytes().collect();
            for (i, byte) in chars.iter().enumerate() {
                nibbles[i] = hex_nibble(*byte)?;
            }
            let expand = |n: u8| (n << 4) | n;
            let a = if chars.len() == 4 {
                expand(nibbles[3])
            } else {
                255
            };
            Some(Rgba {
                r: expand(nibbles[0]),
                g: expand(nibbles[1]),
                b: expand(nibbles[2]),
                a,
            })
        }
        6 | 8 => {
            let raw = u32::from_str_radix(hex, 16).ok()?;
            if hex.len() == 6 {
                Some(Rgba::rgb(raw))
            } else {
                Some(Rgba {
                    r: ((raw >> 24) & 0xff) as u8,
                    g: ((raw >> 16) & 0xff) as u8,
                    b: ((raw >> 8) & 0xff) as u8,
                    a: (raw & 0xff) as u8,
                })
            }
        }
        _ => None,
    }
}

fn hex_nibble(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slash_opacity_scales_the_alpha() {
        let color = parse_color("white/55").unwrap();
        assert_eq!(color.r, 255);
        assert_eq!(color.a, (255.0_f32 * 0.55).round() as u8);
        assert_eq!(
            parse_color("rose-300/70").unwrap().a,
            (255.0_f32 * 0.70).round() as u8
        );
    }

    #[test]
    fn red_500_matches_the_tailwind_palette() {
        let color = parse_color("red-500").unwrap();
        assert_eq!(color, Rgba::rgb(0xef4444));
    }

    #[test]
    fn arbitrary_hex() {
        assert_eq!(parse_color("[#abc]").unwrap(), Rgba::rgb(0xaabbcc));
        assert_eq!(parse_color("[#11223344]").unwrap().a, 0x44);
    }
}
