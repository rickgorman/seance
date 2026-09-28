//! Tagged terminal colors on the SCG3 wire.
//!
//! Explicit RGB stays in the low 24 bits (`0x00RRGGBB`). Semantic palette
//! references use a non-zero high byte so clients can repaint with a local
//! scheme without confusing “true red” with “ANSI red”.

pub const DEFAULT_COLOR: u32 = 0xFFFF_FFFF;

/// Ghostty baseline palette (must match daemon `color_for_index` 0..=15).
pub const DEFAULT_ANSI16: [u32; 16] = [
    0x00_18_18_18,
    0x00_ab_46_42,
    0x00_a1_b5_6c,
    0x00_f7_ca_88,
    0x00_7c_af_c2,
    0x00_ba_8b_af,
    0x00_86_c1_b9,
    0x00_d8_d8_d8,
    0x00_58_58_58,
    0x00_ab_46_42,
    0x00_a1_b5_6c,
    0x00_f7_ca_88,
    0x00_7c_af_c2,
    0x00_ba_8b_af,
    0x00_86_c1_b9,
    0x00_f8_f8_f8,
];

pub const DEFAULT_FG: u32 = 0x00_d8_d8_d8;
pub const DEFAULT_BG: u32 = 0x00_18_18_18;
pub const DEFAULT_CURSOR: u32 = 0x00_e5_c0_7b;

const TAG_INDEX: u32 = 0x01_00_00_00;
const TAG_INDEX_PREDIM: u32 = 0x02_00_00_00;
/// Semantic cursor (palette slot resolved client-side).
pub const CURSOR_COLOR: u32 = 0x03_00_00_00;

/// Pack an indexed color reference for transport (indices 0..=255).
pub fn encode_index(index: u8, pre_dim: bool) -> u32 {
    let tag = if pre_dim { TAG_INDEX_PREDIM } else { TAG_INDEX };
    tag | (index as u32)
}

pub fn is_default_color(packed: u32) -> bool {
    packed == DEFAULT_COLOR
}

pub fn is_cursor_tag(packed: u32) -> bool {
    packed == CURSOR_COLOR
}

/// Indexed tag that already carries the daemon-side dim transform.
pub fn is_predim_tag(packed: u32) -> bool {
    (packed & 0xFF_00_00_00) == TAG_INDEX_PREDIM
}

pub fn tag_index(packed: u32) -> Option<(u8, bool)> {
    let hi = packed & 0xFF_00_00_00;
    match hi {
        TAG_INDEX => Some(((packed & 0xFF) as u8, false)),
        TAG_INDEX_PREDIM => Some(((packed & 0xFF) as u8, true)),
        _ => None,
    }
}

pub fn dim_u32(c: u32) -> u32 {
    let r = ((c >> 16) & 0xff) * 65 / 100;
    let g = ((c >> 8) & 0xff) * 65 / 100;
    let b = (c & 0xff) * 65 / 100;
    (r << 16) | (g << 8) | b
}

fn indexed_rgb(index: u8, ansi16: &[u32; 16]) -> u32 {
    let idx = index as usize;
    match idx {
        0..=15 => ansi16[idx],
        16..=231 => {
            let j = idx - 16;
            let steps = [0u32, 95, 135, 175, 215, 255];
            (steps[j / 36] << 16) | (steps[(j / 6) % 6] << 8) | steps[j % 6]
        }
        232..=255 => {
            let v = (8 + (idx - 232) * 10) as u32;
            (v << 16) | (v << 8) | v
        }
        _ => DEFAULT_FG,
    }
}

/// Resolve a packed cell color to 24-bit RGB (`0x00RRGGBB`).
///
/// `default_rgb` is used when `packed == DEFAULT_COLOR`. Pass scheme fg/bg as
/// appropriate. `cursor_rgb` paints [`CURSOR_COLOR`].
pub fn resolve(packed: u32, default_rgb: u32, ansi16: &[u32; 16], cursor_rgb: u32) -> u32 {
    if packed == DEFAULT_COLOR {
        return default_rgb;
    }
    if packed == CURSOR_COLOR {
        return cursor_rgb;
    }
    if let Some((index, pre_dim)) = tag_index(packed) {
        let base = indexed_rgb(index, ansi16);
        return if pre_dim { dim_u32(base) } else { base };
    }
    // Explicit RGB (historical v1 and application truecolor / OSC overrides).
    packed & 0x00_FF_FF_FF
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_rgb_never_collides_with_ansi_tag() {
        let ansi_red = DEFAULT_ANSI16[1];
        let tag_red = encode_index(1, false);
        assert_ne!(ansi_red, tag_red);
        assert_eq!(
            resolve(tag_red, DEFAULT_FG, &DEFAULT_ANSI16, DEFAULT_CURSOR),
            ansi_red
        );
        assert_eq!(
            resolve(ansi_red, DEFAULT_FG, &DEFAULT_ANSI16, DEFAULT_CURSOR),
            ansi_red
        );
    }

    #[test]
    fn alternate_palette_resolution() {
        let alt: [u32; 16] = {
            let mut a = DEFAULT_ANSI16;
            a[1] = 0x00_FF_00_00;
            a
        };
        let tag = encode_index(1, false);
        assert_eq!(
            resolve(tag, DEFAULT_FG, &alt, DEFAULT_CURSOR),
            0x00_FF_00_00
        );
    }

    #[test]
    fn cube_grayscale_and_predim() {
        let i16 = encode_index(16, false);
        let steps = [0u32, 95, 135, 175, 215, 255];
        let expect = (steps[0] << 16) | (steps[0] << 8) | steps[0];
        assert_eq!(
            resolve(i16, DEFAULT_FG, &DEFAULT_ANSI16, DEFAULT_CURSOR),
            expect
        );

        let gray = encode_index(233, true);
        let v = (8 + (233 - 232) * 10) as u32;
        let raw = (v << 16) | (v << 8) | v;
        assert_eq!(
            resolve(gray, DEFAULT_FG, &DEFAULT_ANSI16, DEFAULT_CURSOR),
            dim_u32(raw)
        );
        assert!(is_predim_tag(gray));
    }

    #[test]
    fn default_and_cursor_tags() {
        assert_eq!(
            resolve(DEFAULT_COLOR, DEFAULT_FG, &DEFAULT_ANSI16, DEFAULT_CURSOR),
            DEFAULT_FG
        );
        assert_eq!(
            resolve(CURSOR_COLOR, DEFAULT_FG, &DEFAULT_ANSI16, DEFAULT_CURSOR),
            DEFAULT_CURSOR
        );
    }

    #[test]
    fn predim_tag_distinct_from_normal_index() {
        let normal = encode_index(1, false);
        let predim = encode_index(1, true);
        assert!(is_predim_tag(predim));
        assert!(!is_predim_tag(normal));
    }
}
