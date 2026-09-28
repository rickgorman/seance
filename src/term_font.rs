//! Shared terminal font — match ghostty on this machine.
//!
//! Ghostty config: `font-family = monospace` which resolves (fc-match) to
//! **JetBrainsMono Nerd Font**, size 9, ligatures off. Seance prefers the same
//! family when installed so Claude's block-drawing logo aligns the same way.
//! The primary family is chosen from the host font inventory at startup (see
//! [`init`]); `FontFallbacks` only cover missing glyphs in that face, not a
//! missing primary family (GPUI falls back to `.ZedMono`/etc. instead).

use std::collections::HashSet;
use std::sync::OnceLock;

use gpui::{font, Font, FontFeatures, SharedString};

/// Preferred family when installed (ghostty fc-match on the reference host).
pub const FONT_FAMILY: &str = "JetBrainsMono Nerd Font";

/// Ghostty uses 9; we keep a slightly larger default for multi-pane grids.
/// Still the same face — only the size differs for readability.
pub const FONT_SIZE: f32 = 12.0;

/// Tight line height so half-blocks stack cleanly (ghostty-ish).
pub const LINE_HEIGHT_FACTOR: f32 = 1.2;

const FONT_CANDIDATES: &[&str] = &[
    "JetBrainsMono Nerd Font",
    "JetBrains Mono",
    "Menlo",
    "Liberation Mono",
    "DejaVu Sans Mono",
    "Noto Sans Mono",
];

static RESOLVED_FAMILY: OnceLock<String> = OnceLock::new();

/// Pick and cache the terminal primary family from `installed` (from
/// `TextSystem::all_font_names`). Call once from [`crate::theme::init`] before
/// any windows open.
pub fn init(installed: &HashSet<String>) {
    let _ = RESOLVED_FAMILY.set(select_installed_term_family(installed));
}

/// First installed candidate in priority order, else the preferred name.
pub(crate) fn select_installed_term_family(installed: &HashSet<String>) -> String {
    FONT_CANDIDATES
        .iter()
        .find(|c| installed.contains(**c))
        .map(|c| c.to_string())
        .unwrap_or_else(|| FONT_FAMILY.to_string())
}

fn resolved_family() -> &'static str {
    RESOLVED_FAMILY
        .get()
        .map(|s| s.as_str())
        .unwrap_or(FONT_FAMILY)
}

fn term_font_for_family(family: &str) -> Font {
    Font {
        family: SharedString::from(family),
        features: FontFeatures(std::sync::Arc::new(vec![
            ("calt".into(), 0),
            ("liga".into(), 0),
            ("clig".into(), 0),
            ("dlig".into(), 0),
            ("hlig".into(), 0),
        ])),
        weight: Default::default(),
        style: Default::default(),
        fallbacks: Some(gpui::FontFallbacks::from_fonts(vec![
            "Menlo".into(),
            "JetBrains Mono".into(),
            "DejaVu Sans Mono".into(),
            "monospace".into(),
        ])),
    }
}

/// Terminal font with ligatures disabled (matches ghostty's -liga/-calt).
pub fn term_font() -> Font {
    term_font_for_family(resolved_family())
}

pub fn term_font_bold() -> Font {
    term_font().bold()
}

/// Convenience when only family is needed (legacy call sites).
#[allow(dead_code)]
pub fn term_font_plain() -> Font {
    font(resolved_family())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stock_macos_inventory() -> HashSet<String> {
        [
            "Menlo",
            "Helvetica",
            "Helvetica Neue",
            "SF Pro Text",
            ".AppleSystemUIFont",
        ]
        .into_iter()
        .map(str::to_string)
        .collect()
    }

    #[test]
    fn stock_mac_without_nerd_font_picks_menlo() {
        let family = select_installed_term_family(&stock_macos_inventory());
        assert_eq!(family, "Menlo");
    }

    #[test]
    fn prefers_nerd_font_when_installed() {
        let mut installed = stock_macos_inventory();
        installed.insert("JetBrainsMono Nerd Font".into());
        assert_eq!(
            select_installed_term_family(&installed),
            "JetBrainsMono Nerd Font"
        );
    }

    #[test]
    fn jetbrains_mono_without_nerd_font() {
        let mut installed = stock_macos_inventory();
        installed.insert("JetBrains Mono".into());
        assert_eq!(select_installed_term_family(&installed), "JetBrains Mono");
    }

    #[test]
    fn linux_fallback_order() {
        let installed: HashSet<String> = ["Liberation Mono", "DejaVu Sans Mono", "Noto Sans Mono"]
            .into_iter()
            .map(str::to_string)
            .collect();
        assert_eq!(select_installed_term_family(&installed), "Liberation Mono");
    }

    #[test]
    fn term_font_disables_ligatures_for_chosen_family() {
        let font = term_font_for_family("Menlo");
        assert_eq!(font.family.as_ref(), "Menlo");
        let feats = font.features.0.as_ref();
        assert_eq!(
            feats.iter().find(|(k, _)| k == "liga").map(|(_, v)| *v),
            Some(0)
        );
        assert_eq!(
            feats.iter().find(|(k, _)| k == "calt").map(|(_, v)| *v),
            Some(0)
        );
    }
}
