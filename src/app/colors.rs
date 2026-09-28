//! Terminal color schemes, iTerm2 import, and process-local applied palette.
//!
//! Parsing and [`ColorScheme`] live here; GPUI paint reads [`paint_palette_snapshot`]
//! (updated from desktop prefs via [`apply_palette`]).

use std::collections::HashSet;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock, RwLock};

use gpui::Hsla;
use plist::Value as PlistValue;
use seance_core::terminal_color::{DEFAULT_ANSI16, DEFAULT_BG, DEFAULT_FG};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value as JsonValue;

const MAX_IMPORT_BYTES: usize = 16 * 1024 * 1024;

/// Opaque validated 24-bit RGB (no alpha).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rgb24 {
    r: u8,
    g: u8,
    b: u8,
}

impl Rgb24 {
    pub fn from_rgb(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }

    pub fn from_u32(packed: u32) -> Result<Self, String> {
        if packed > 0xFF_FF_FF {
            return Err(format!("RGB value out of range: {packed:#010x}"));
        }
        let r = ((packed >> 16) & 0xff) as u8;
        let g = ((packed >> 8) & 0xff) as u8;
        let b = (packed & 0xff) as u8;
        Ok(Self { r, g, b })
    }

    pub fn from_hex(hex: &str) -> Result<Self, String> {
        let s = hex.trim();
        let s = s.strip_prefix('#').unwrap_or(s);
        if s.len() != 6 || !s.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(format!("invalid #RRGGBB hex: {hex}"));
        }
        let n = u32::from_str_radix(s, 16).map_err(|_| format!("invalid #RRGGBB hex: {hex}"))?;
        Self::from_u32(n)
    }

    pub fn as_u32(self) -> u32 {
        ((self.r as u32) << 16) | ((self.g as u32) << 8) | (self.b as u32)
    }

    pub fn to_hex(self) -> String {
        format!("#{:02X}{:02X}{:02X}", self.r, self.g, self.b)
    }

    pub fn r(self) -> u8 {
        self.r
    }

    pub fn g(self) -> u8 {
        self.g
    }

    pub fn b(self) -> u8 {
        self.b
    }
}

impl Serialize for Rgb24 {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_hex())
    }
}

impl<'de> Deserialize<'de> for Rgb24 {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        Self::from_hex(&s).map_err(serde::de::Error::custom)
    }
}

/// Full terminal palette for rendering and import inheritance.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ColorScheme {
    pub name: String,
    pub foreground: Rgb24,
    pub background: Rgb24,
    pub ansi: [Rgb24; 16],
    pub cursor: Rgb24,
    pub cursor_text: Rgb24,
    pub selection_background: Rgb24,
    pub selection_foreground: Rgb24,
    pub bold: Option<Rgb24>,
}

impl Default for ColorScheme {
    fn default() -> Self {
        let ansi = std::array::from_fn(|i| Rgb24::from_u32(DEFAULT_ANSI16[i]).expect("baseline"));
        Self {
            name: String::new(),
            foreground: Rgb24::from_u32(DEFAULT_FG).expect("baseline"),
            background: Rgb24::from_u32(DEFAULT_BG).expect("baseline"),
            ansi,
            cursor: Rgb24::from_hex("#E9A03A").expect("baseline"),
            cursor_text: Rgb24::from_hex("#181818").expect("baseline"),
            selection_background: Rgb24::from_hex("#5D4A7D").expect("baseline"),
            selection_foreground: Rgb24::from_hex("#D8D8D8").expect("baseline"),
            bold: None,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ImportResult {
    pub schemes: Vec<ColorScheme>,
    pub warnings: Vec<String>,
}

/// Parse an iTerm2 export: `.itermcolors` (XML/binary plist), JSON profile(s), or full prefs fragment.
pub fn parse_iterm(
    bytes: &[u8],
    source_name: &str,
    base: &ColorScheme,
) -> Result<ImportResult, String> {
    if bytes.len() > MAX_IMPORT_BYTES {
        return Err(format!(
            "input too large ({} bytes; max {MAX_IMPORT_BYTES})",
            bytes.len()
        ));
    }
    let trimmed = trim_utf8_bom(bytes);
    if looks_like_json(trimmed) {
        return parse_iterm_json(trimmed, source_name, base);
    }
    match plist::from_bytes::<PlistValue>(trimmed) {
        Ok(root) => parse_iterm_plist_root(&root, source_name, base),
        Err(e) => Err(format!("{source_name}: not JSON or plist: {e}")),
    }
}

#[cfg(target_os = "macos")]
pub fn load_installed_iterm(base: &ColorScheme) -> Result<ImportResult, String> {
    let mut out = ImportResult::default();
    let home = home_dir()?;

    if let Some(load) = read_main_iterm_prefs(&home)? {
        out.warnings.extend(load.warnings);
        match parse_iterm(&load.bytes, &load.label, base) {
            Ok(part) => merge_import(&mut out, part),
            Err(e) => out.warnings.push(e),
        }
    }

    let dynamic_dir = home.join("Library/Application Support/iTerm2/DynamicProfiles");
    if dynamic_dir.is_dir() {
        let mut paths: Vec<PathBuf> = std::fs::read_dir(&dynamic_dir)
            .map_err(|e| format!("DynamicProfiles: {e}"))?
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|x| x == "json"))
            .collect();
        paths.sort();
        for path in paths {
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| "DynamicProfiles".into());
            match read_import_file_capped(&path) {
                Ok(bytes) => match parse_iterm(&bytes, &name, base) {
                    Ok(part) => merge_import(&mut out, part),
                    Err(e) => out.warnings.push(format!("{name}: {e}")),
                },
                Err(e) => out.warnings.push(format!("{name}: read failed: {e}")),
            }
        }
    }

    dedup_schemes(&mut out.schemes);
    Ok(out)
}

#[cfg(not(target_os = "macos"))]
pub fn load_installed_iterm(_base: &ColorScheme) -> Result<ImportResult, String> {
    Err("installed iTerm2 profile import is only supported on macOS".into())
}

/// Read at most [`MAX_IMPORT_BYTES`] from any reader (one-byte reads are fine).
pub fn read_import_stream_capped(read: impl Read) -> Result<Vec<u8>, String> {
    let mut limited = read.take((MAX_IMPORT_BYTES + 1) as u64);
    let mut buf = Vec::new();
    limited
        .read_to_end(&mut buf)
        .map_err(|e| format!("read failed: {e}"))?;
    if buf.len() > MAX_IMPORT_BYTES {
        return Err(format!("input too large (>{MAX_IMPORT_BYTES} bytes)"));
    }
    Ok(buf)
}

/// Read at most [`MAX_IMPORT_BYTES`] from a file (reject before buffering more).
pub fn read_import_file_capped(path: &Path) -> Result<Vec<u8>, String> {
    let file = std::fs::File::open(path).map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            format!("{}: not found", path.display())
        } else {
            format!("{}: {e}", path.display())
        }
    })?;
    read_import_stream_capped(file).map_err(|e| format!("{}: {e}", path.display()))
}

pub fn scheme_ansi16_u32(scheme: &ColorScheme) -> [u32; 16] {
    std::array::from_fn(|i| scheme.ansi[i].as_u32())
}

pub fn rgb24_to_hsla(c: Rgb24) -> Hsla {
    gpui::Rgba {
        r: c.r() as f32 / 255.,
        g: c.g() as f32 / 255.,
        b: c.b() as f32 / 255.,
        a: 1.,
    }
    .into()
}

/// Build a scheme from validated `#RRGGBB` fields (`bold` blank → `None`).
pub fn color_scheme_from_fields(
    name: &str,
    foreground: &str,
    background: &str,
    ansi: &[String; 16],
    cursor: &str,
    cursor_text: &str,
    selection_background: &str,
    selection_foreground: &str,
    bold: &str,
) -> Result<ColorScheme, String> {
    let bold = if bold.trim().is_empty() {
        None
    } else {
        Some(Rgb24::from_hex(bold)?)
    };
    let mut parsed_ansi = [Rgb24::from_rgb(0, 0, 0); 16];
    for (i, h) in ansi.iter().enumerate() {
        parsed_ansi[i] = Rgb24::from_hex(h)?;
    }
    Ok(ColorScheme {
        name: name.to_string(),
        foreground: Rgb24::from_hex(foreground)?,
        background: Rgb24::from_hex(background)?,
        ansi: parsed_ansi,
        cursor: Rgb24::from_hex(cursor)?,
        cursor_text: Rgb24::from_hex(cursor_text)?,
        selection_background: Rgb24::from_hex(selection_background)?,
        selection_foreground: Rgb24::from_hex(selection_foreground)?,
        bold,
    })
}

struct AppliedPalette {
    scheme: Arc<ColorScheme>,
    generation: u64,
}

static APPLIED: OnceLock<RwLock<AppliedPalette>> = OnceLock::new();

fn applied_palette_lock() -> &'static RwLock<AppliedPalette> {
    APPLIED.get_or_init(|| {
        RwLock::new(AppliedPalette {
            scheme: Arc::new(ColorScheme::default()),
            generation: 0,
        })
    })
}

pub fn init_applied_palette(scheme: &ColorScheme) {
    if APPLIED
        .set(RwLock::new(AppliedPalette {
            scheme: Arc::new(scheme.clone()),
            generation: 1,
        }))
        .is_err()
    {
        return;
    }
}

/// Coherent palette snapshot for one paint pass (`Arc` + generation).
pub fn paint_palette_snapshot() -> (Arc<ColorScheme>, u64) {
    let guard = applied_palette_lock().read().expect("applied palette");
    (guard.scheme.clone(), guard.generation)
}

/// Update process-local applied palette. Returns `true` when colors changed.
pub fn apply_palette(scheme: ColorScheme) -> bool {
    let mut guard = applied_palette_lock().write().expect("applied palette");
    if guard.scheme.as_ref() == &scheme {
        return false;
    }
    guard.generation += 1;
    guard.scheme = Arc::new(scheme);
    crate::remote_term_view::invalidate_shaped_paint_caches();
    true
}

fn merge_import(out: &mut ImportResult, part: ImportResult) {
    out.warnings.extend(part.warnings);
    out.schemes.extend(part.schemes);
}

fn dedup_schemes(schemes: &mut Vec<ColorScheme>) {
    let mut seen = HashSet::new();
    schemes.retain(|s| seen.insert(scheme_fingerprint(s)));
}

fn scheme_fingerprint(s: &ColorScheme) -> String {
    let mut fp = format!("name={}\0", s.name);
    fp.push_str(&palette_fingerprint(s));
    fp
}

fn palette_fingerprint(s: &ColorScheme) -> String {
    let mut parts = vec![
        s.foreground.to_hex(),
        s.background.to_hex(),
        s.cursor.to_hex(),
        s.cursor_text.to_hex(),
        s.selection_background.to_hex(),
        s.selection_foreground.to_hex(),
    ];
    for a in &s.ansi {
        parts.push(a.to_hex());
    }
    parts.push(match s.bold {
        Some(b) => b.to_hex(),
        None => String::new(),
    });
    parts.join("|")
}

fn trim_utf8_bom(bytes: &[u8]) -> &[u8] {
    bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(bytes)
}

fn looks_like_json(bytes: &[u8]) -> bool {
    let s = bytes
        .iter()
        .copied()
        .skip_while(|b| b.is_ascii_whitespace())
        .take(1)
        .collect::<Vec<_>>();
    matches!(s.first(), Some(b'{' | b'['))
}

fn parse_iterm_json(
    bytes: &[u8],
    source_name: &str,
    base: &ColorScheme,
) -> Result<ImportResult, String> {
    let root: JsonValue =
        serde_json::from_slice(bytes).map_err(|e| format!("{source_name}: invalid JSON: {e}"))?;
    let plist_root = json_to_plist(&root);
    parse_iterm_plist_root(&plist_root, source_name, base)
}

fn parse_iterm_plist_root(
    root: &PlistValue,
    source_name: &str,
    base: &ColorScheme,
) -> Result<ImportResult, String> {
    let mut out = ImportResult::default();
    let profiles = collect_profile_dicts(root);
    if profiles.is_empty() {
        return Err(format!("{source_name}: no profiles found"));
    }
    let fallback_name = default_profile_name(source_name);
    let profile_count = profiles.len();
    for (idx, dict) in profiles {
        let name_hint = profile_display_name(dict).unwrap_or_else(|| {
            if profile_count == 1 {
                fallback_name.clone()
            } else {
                format!("{fallback_name} ({idx})")
            }
        });
        match profile_from_dict(dict, base, &name_hint) {
            Ok(scheme) => out.schemes.push(scheme),
            Err(msg) => out
                .warnings
                .push(format!("{source_name}: profile {name_hint}: {msg}")),
        }
    }
    dedup_schemes(&mut out.schemes);
    if out.schemes.is_empty() && out.warnings.is_empty() {
        return Err(format!("{source_name}: no valid color profiles"));
    }
    Ok(out)
}

fn default_profile_name(source_name: &str) -> String {
    Path::new(source_name)
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| source_name.to_string())
}

fn collect_profile_dicts(root: &PlistValue) -> Vec<(usize, &plist::Dictionary)> {
    let mut out = Vec::new();
    match root {
        PlistValue::Dictionary(d) => {
            if let Some(PlistValue::Array(bookmarks)) = d.get("New Bookmarks") {
                for (i, item) in bookmarks.iter().enumerate() {
                    if let PlistValue::Dictionary(pd) = item {
                        out.push((i, pd));
                    }
                }
                if !out.is_empty() {
                    return out;
                }
            }
            if let Some(PlistValue::Array(profiles)) = d.get("Profiles") {
                for (i, item) in profiles.iter().enumerate() {
                    if let PlistValue::Dictionary(pd) = item {
                        out.push((i, pd));
                    }
                }
                if !out.is_empty() {
                    return out;
                }
            }
            if dict_has_recognized_color(d) {
                out.push((0, d));
            }
        }
        PlistValue::Array(items) => {
            for (i, item) in items.iter().enumerate() {
                match item {
                    PlistValue::Dictionary(pd) => out.push((i, pd)),
                    _ => {}
                }
            }
        }
        _ => {}
    }
    out
}

fn profile_display_name(dict: &plist::Dictionary) -> Option<String> {
    dict.get("Name")
        .or_else(|| dict.get("Profile Name"))
        .and_then(plist_value_as_string)
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

fn profile_from_dict(
    dict: &plist::Dictionary,
    base: &ColorScheme,
    name: &str,
) -> Result<ColorScheme, String> {
    if !dict_has_recognized_color(dict) {
        return Err("no recognized terminal color fields".into());
    }
    let mut scheme = base.clone();
    scheme.name = name.to_string();

    if let Some(v) = dict.get("Foreground Color") {
        scheme.foreground = parse_iterm_color_value(v, "Foreground Color")?;
    }
    if let Some(v) = dict.get("Background Color") {
        scheme.background = parse_iterm_color_value(v, "Background Color")?;
    }
    for i in 0..16 {
        let key = format!("Ansi {i} Color");
        if let Some(v) = dict.get(&key) {
            scheme.ansi[i] = parse_iterm_color_value(v, &key)?;
        }
    }
    if let Some(v) = dict.get("Bold Color") {
        scheme.bold = Some(parse_iterm_color_value(v, "Bold Color")?);
    }
    if let Some(v) = dict.get("Cursor Color") {
        scheme.cursor = parse_iterm_color_value(v, "Cursor Color")?;
    }
    if let Some(v) = dict.get("Cursor Text Color") {
        scheme.cursor_text = parse_iterm_color_value(v, "Cursor Text Color")?;
    }
    if let Some(v) = dict.get("Selection Color") {
        scheme.selection_background = parse_iterm_color_value(v, "Selection Color")?;
    }
    if let Some(v) = dict.get("Selected Text Color") {
        scheme.selection_foreground = parse_iterm_color_value(v, "Selected Text Color")?;
    }
    Ok(scheme)
}

fn dict_has_recognized_color(dict: &plist::Dictionary) -> bool {
    RECOGNIZED_COLOR_KEYS.iter().any(|k| dict.contains_key(*k))
}

const RECOGNIZED_COLOR_KEYS: &[&str] = &[
    "Foreground Color",
    "Background Color",
    "Bold Color",
    "Cursor Color",
    "Cursor Text Color",
    "Selection Color",
    "Selected Text Color",
    "Ansi 0 Color",
    "Ansi 1 Color",
    "Ansi 2 Color",
    "Ansi 3 Color",
    "Ansi 4 Color",
    "Ansi 5 Color",
    "Ansi 6 Color",
    "Ansi 7 Color",
    "Ansi 8 Color",
    "Ansi 9 Color",
    "Ansi 10 Color",
    "Ansi 11 Color",
    "Ansi 12 Color",
    "Ansi 13 Color",
    "Ansi 14 Color",
    "Ansi 15 Color",
];

fn parse_iterm_color_value(v: &PlistValue, field: &str) -> Result<Rgb24, String> {
    let dict = match v {
        PlistValue::Dictionary(d) => d,
        _ => return Err(format!("{field}: expected color dictionary")),
    };
    parse_iterm_color_dict(dict, field)
}

fn parse_iterm_color_dict(dict: &plist::Dictionary, field: &str) -> Result<Rgb24, String> {
    let r = dict
        .get("Red Component")
        .ok_or_else(|| format!("{field}: missing Red Component"))?;
    let g = dict
        .get("Green Component")
        .ok_or_else(|| format!("{field}: missing Green Component"))?;
    let b = dict
        .get("Blue Component")
        .ok_or_else(|| format!("{field}: missing Blue Component"))?;
    let rf = plist_scalar_f64(r).ok_or_else(|| format!("{field}: invalid Red Component"))?;
    let gf = plist_scalar_f64(g).ok_or_else(|| format!("{field}: invalid Green Component"))?;
    let bf = plist_scalar_f64(b).ok_or_else(|| format!("{field}: invalid Blue Component"))?;
    let space = dict
        .get("Color Space")
        .and_then(plist_value_as_string)
        .map(|s| s.trim().to_string());

    components_to_rgb24(rf, gf, bf, space.as_deref(), field)
}

fn plist_scalar_f64(v: &PlistValue) -> Option<f64> {
    match v {
        PlistValue::Real(r) => Some(*r),
        PlistValue::Integer(i) => i
            .as_signed()
            .map(|n| n as f64)
            .or_else(|| i.as_unsigned().map(|n| n as f64)),
        PlistValue::String(value) => value.trim().parse().ok(),
        _ => None,
    }
}

fn plist_value_as_string(v: &PlistValue) -> Option<String> {
    match v {
        PlistValue::String(s) => Some(s.clone()),
        _ => None,
    }
}

fn components_to_rgb24(
    r: f64,
    g: f64,
    b: f64,
    color_space: Option<&str>,
    field: &str,
) -> Result<Rgb24, String> {
    for c in [r, g, b] {
        if !c.is_finite() {
            return Err(format!("{field}: non-finite component"));
        }
        if c < 0.0 || c > 1.0 {
            return Err(format!("{field}: component out of range 0..1"));
        }
    }

    let (sr, sg, sb) = match color_space {
        None | Some("") => (r, g, b),
        Some(s) if s.eq_ignore_ascii_case("srgb") => (r, g, b),
        Some(s) if s.eq_ignore_ascii_case("display p3") => display_p3_to_srgb(r, g, b),
        Some(other) => {
            return Err(format!("{field}: unsupported Color Space '{other}'"));
        }
    };

    Ok(Rgb24::from_rgb(
        float_to_byte(sr, field)?,
        float_to_byte(sg, field)?,
        float_to_byte(sb, field)?,
    ))
}

fn float_to_byte(v: f64, field: &str) -> Result<u8, String> {
    if !v.is_finite() {
        return Err(format!("{field}: non-finite value"));
    }
    let scaled = v * 255.0;
    if scaled < 0.0 || scaled > 255.0 {
        return Err(format!("{field}: component out of range after conversion"));
    }
    Ok(scaled.round() as u8)
}

/// Display P3 (gamma-encoded) → sRGB (gamma-encoded), IEC 61966-2-1 transfer.
fn display_p3_to_srgb(r: f64, g: f64, b: f64) -> (f64, f64, f64) {
    let rl = decode_srgb_transfer(r);
    let gl = decode_srgb_transfer(g);
    let bl = decode_srgb_transfer(b);
    let sr = 1.224_940_176 * rl - 0.224_940_176 * gl;
    let sg = -0.042_056_954 * rl + 1.042_056_954 * gl;
    let sb = -0.019_637_555 * rl - 0.078_636_021 * gl + 1.098_273_547 * bl;
    (
        encode_srgb_transfer(sr.clamp(0.0, 1.0)),
        encode_srgb_transfer(sg.clamp(0.0, 1.0)),
        encode_srgb_transfer(sb.clamp(0.0, 1.0)),
    )
}

fn decode_srgb_transfer(c: f64) -> f64 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

fn encode_srgb_transfer(c: f64) -> f64 {
    if c <= 0.0031308 {
        12.92 * c
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    }
}

fn json_to_plist(v: &JsonValue) -> PlistValue {
    match v {
        JsonValue::Null => PlistValue::String(String::new()),
        JsonValue::Bool(b) => PlistValue::Boolean(*b),
        JsonValue::Number(n) => {
            if let Some(i) = n.as_i64() {
                PlistValue::Integer(plist::Integer::from(i))
            } else if let Some(u) = n.as_u64() {
                PlistValue::Integer(plist::Integer::from(u))
            } else {
                PlistValue::Real(n.as_f64().unwrap_or(0.0))
            }
        }
        JsonValue::String(s) => PlistValue::String(s.clone()),
        JsonValue::Array(a) => PlistValue::Array(a.iter().map(json_to_plist).collect()),
        JsonValue::Object(o) => {
            let mut d = plist::Dictionary::new();
            for (k, val) in o {
                d.insert(k.clone(), json_to_plist(val));
            }
            PlistValue::Dictionary(d)
        }
    }
}

#[cfg(target_os = "macos")]
fn home_dir() -> Result<PathBuf, String> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or_else(|| "HOME not set".to_string())
}

#[cfg(target_os = "macos")]
fn expand_tilde(path: &str, home: &Path) -> PathBuf {
    if path == "~" {
        return home.to_path_buf();
    }
    if let Some(rest) = path.strip_prefix("~/") {
        return home.join(rest);
    }
    PathBuf::from(path)
}

#[cfg(target_os = "macos")]
struct MainPrefsLoad {
    bytes: Vec<u8>,
    label: String,
    warnings: Vec<String>,
}

#[cfg(target_os = "macos")]
fn read_main_iterm_prefs(home: &Path) -> Result<Option<MainPrefsLoad>, String> {
    let standard = home.join("Library/Preferences/com.googlecode.iterm2.plist");
    let bytes = match read_import_file_capped(&standard) {
        Ok(b) => b,
        Err(e) if e.ends_with(": not found") => return Ok(None),
        Err(e) => return Err(format!("iTerm2 preferences: {e}")),
    };
    let label = standard.to_string_lossy().into_owned();
    let mut warnings = Vec::new();

    if let Ok(PlistValue::Dictionary(d)) = plist::from_bytes::<PlistValue>(&bytes) {
        if let Some(custom) = iterm_custom_prefs_path(&d, home) {
            match read_import_file_capped(&custom) {
                Ok(custom_bytes) => {
                    return Ok(Some(MainPrefsLoad {
                        bytes: custom_bytes,
                        label: custom.to_string_lossy().into_owned(),
                        warnings,
                    }));
                }
                Err(e) => {
                    warnings.push(format!(
                        "custom iTerm2 prefs at {} unavailable ({e}); using standard preferences",
                        custom.display()
                    ));
                }
            }
        }
    }

    Ok(Some(MainPrefsLoad {
        bytes,
        label,
        warnings,
    }))
}

/// When iTerm2 loads prefs from a custom folder, resolve that plist path.
#[cfg(target_os = "macos")]
pub fn iterm_custom_prefs_path(main_prefs: &plist::Dictionary, home: &Path) -> Option<PathBuf> {
    let load_custom = main_prefs
        .get("LoadPrefsFromCustomFolder")
        .and_then(|v| match v {
            PlistValue::Boolean(b) => Some(*b),
            _ => None,
        })
        .unwrap_or(false);
    if !load_custom {
        return None;
    }
    let folder = main_prefs
        .get("PrefsCustomFolder")
        .and_then(plist_value_as_string)
        .or_else(|| {
            main_prefs
                .get("localPrefsCustomFolder")
                .and_then(plist_value_as_string)
        });
    folder.map(|folder| {
        let mut custom = expand_tilde(folder.trim(), home);
        if custom.is_dir() {
            custom.push("com.googlecode.iterm2.plist");
        }
        custom
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_color(r: f64, g: f64, b: f64) -> PlistValue {
        let mut d = plist::Dictionary::new();
        d.insert("Red Component".into(), PlistValue::Real(r));
        d.insert("Green Component".into(), PlistValue::Real(g));
        d.insert("Blue Component".into(), PlistValue::Real(b));
        PlistValue::Dictionary(d)
    }

    fn sample_color_srgb(r: f64, g: f64, b: f64) -> PlistValue {
        let mut d = plist::Dictionary::new();
        d.insert("Red Component".into(), PlistValue::Real(r));
        d.insert("Green Component".into(), PlistValue::Real(g));
        d.insert("Blue Component".into(), PlistValue::Real(b));
        d.insert("Color Space".into(), PlistValue::String("sRGB".into()));
        PlistValue::Dictionary(d)
    }

    fn plist_xml_bytes(v: &PlistValue) -> Vec<u8> {
        let mut buf = Vec::new();
        v.to_writer_xml(&mut buf).unwrap();
        buf
    }

    fn plist_binary_bytes(v: &PlistValue) -> Vec<u8> {
        let mut buf = Vec::new();
        v.to_writer_binary(&mut buf).unwrap();
        buf
    }

    #[test]
    fn rgb24_hex_roundtrip() {
        let c = Rgb24::from_hex("#A1B2C3").unwrap();
        assert_eq!(c.to_hex(), "#A1B2C3");
        assert_eq!(c.as_u32(), 0x00_a1_b2_c3);
        assert!(Rgb24::from_hex("ZZZZZZ").is_err());
        assert!(Rgb24::from_hex("#ABC").is_err());
        assert!(Rgb24::from_u32(0x1_00_00_00).is_err());
    }

    #[test]
    fn default_scheme_matches_baseline() {
        let d = ColorScheme::default();
        assert_eq!(d.foreground, Rgb24::from_u32(DEFAULT_FG).unwrap());
        assert_eq!(d.background, Rgb24::from_u32(DEFAULT_BG).unwrap());
        assert_eq!(d.cursor.to_hex(), "#E9A03A");
        for i in 0..16 {
            assert_eq!(d.ansi[i], Rgb24::from_u32(DEFAULT_ANSI16[i]).unwrap());
        }
    }

    #[test]
    fn read_import_stream_capped_handles_short_reads() {
        struct OneByteAtATime(Vec<u8>);
        impl Read for OneByteAtATime {
            fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
                if self.0.is_empty() {
                    return Ok(0);
                }
                let n = 1.min(buf.len());
                buf[..n].copy_from_slice(&self.0[..n]);
                self.0.drain(..n);
                Ok(n)
            }
        }
        let payload = b"hello capped read";
        let out = read_import_stream_capped(OneByteAtATime(payload.to_vec())).unwrap();
        assert_eq!(out, payload);
    }

    #[test]
    fn read_import_stream_capped_rejects_oversized() {
        let huge = vec![0u8; MAX_IMPORT_BYTES + 1];
        let err = read_import_stream_capped(huge.as_slice()).unwrap_err();
        assert!(err.contains("too large"));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn iterm_custom_prefs_path_respects_load_flag_and_key() {
        let home = std::env::temp_dir().join("seance-iterm-custom-prefs-test");
        let custom_dir = home.join("custom-iterm");
        let _ = std::fs::remove_dir_all(&custom_dir);
        std::fs::create_dir_all(&custom_dir).unwrap();
        let home = home.as_path();
        let mut d = plist::Dictionary::new();
        d.insert(
            "LoadPrefsFromCustomFolder".into(),
            PlistValue::Boolean(false),
        );
        d.insert(
            "PrefsCustomFolder".into(),
            PlistValue::String("~/custom-iterm".into()),
        );
        assert!(iterm_custom_prefs_path(&d, home).is_none());

        d.insert(
            "LoadPrefsFromCustomFolder".into(),
            PlistValue::Boolean(true),
        );
        let path = iterm_custom_prefs_path(&d, home).unwrap();
        assert_eq!(path, home.join("custom-iterm/com.googlecode.iterm2.plist"));

        d.remove("PrefsCustomFolder");
        let legacy_dir = home.join("legacy-iterm");
        std::fs::create_dir_all(&legacy_dir).unwrap();
        d.insert(
            "localPrefsCustomFolder".into(),
            PlistValue::String("~/legacy-iterm".into()),
        );
        let path = iterm_custom_prefs_path(&d, home).unwrap();
        assert_eq!(path, home.join("legacy-iterm/com.googlecode.iterm2.plist"));
        let _ = std::fs::remove_dir_all(home);
    }

    #[test]
    fn legacy_numeric_string_components_are_imported_and_validated() {
        let bytes = br#"{"Name":"Legacy","Foreground Color":{"Red Component":"0.2","Green Component":"0.4","Blue Component":"0.6"}}"#;
        let result = parse_iterm(bytes, "legacy.json", &ColorScheme::default()).unwrap();
        assert_eq!(result.schemes.len(), 1);
        assert_eq!(result.schemes[0].foreground.to_hex(), "#336699");
        for invalid in ["NaN", "2.0", "not a number"] {
            let bytes = format!(
                r#"{{"Foreground Color":{{"Red Component":"{invalid}","Green Component":"0","Blue Component":"0"}}}}"#
            );
            let result =
                parse_iterm(bytes.as_bytes(), "invalid.json", &ColorScheme::default()).unwrap();
            assert!(result.schemes.is_empty());
            assert!(!result.warnings.is_empty());
        }
    }

    #[test]
    fn xml_itermcolors_parses() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>Background Color</key><dict>
<key>Red Component</key><real>0.1</real>
<key>Green Component</key><real>0.2</real>
<key>Blue Component</key><real>0.3</real>
</dict>
</dict></plist>"#;
        let base = ColorScheme::default();
        let out = parse_iterm(xml.as_bytes(), "Test.itermcolors", &base).unwrap();
        assert_eq!(out.schemes.len(), 1);
        assert_eq!(out.schemes[0].name, "Test");
        assert_eq!(out.schemes[0].background.to_hex(), "#1A334D");
        assert_eq!(out.schemes[0].foreground, base.foreground);
    }

    #[test]
    fn missing_fields_inherit_base() {
        let mut dict = plist::Dictionary::new();
        dict.insert("Foreground Color".into(), sample_color(1.0, 1.0, 1.0));
        let base = ColorScheme::default();
        let scheme = profile_from_dict(&dict, &base, "x").unwrap();
        assert_eq!(scheme.foreground.to_hex(), "#FFFFFF");
        assert_eq!(scheme.background, base.background);
    }

    #[test]
    fn no_recognized_colors_rejects() {
        let mut dict = plist::Dictionary::new();
        dict.insert("Name".into(), PlistValue::String("evil".into()));
        dict.insert("Command".into(), PlistValue::String("rm -rf /".into()));
        let base = ColorScheme::default();
        assert!(profile_from_dict(&dict, &base, "evil").is_err());
    }

    #[test]
    fn malformed_component_rejects_profile() {
        let mut inner = plist::Dictionary::new();
        inner.insert("Red Component".into(), PlistValue::Real(f64::NAN));
        inner.insert("Green Component".into(), PlistValue::Real(0.0));
        inner.insert("Blue Component".into(), PlistValue::Real(0.0));
        let mut dict = plist::Dictionary::new();
        dict.insert("Background Color".into(), PlistValue::Dictionary(inner));
        let base = ColorScheme::default();
        assert!(profile_from_dict(&dict, &base, "bad").is_err());
    }

    #[test]
    fn unsupported_color_space_rejects() {
        let mut inner = plist::Dictionary::new();
        inner.insert("Red Component".into(), PlistValue::Real(0.5));
        inner.insert("Green Component".into(), PlistValue::Real(0.5));
        inner.insert("Blue Component".into(), PlistValue::Real(0.5));
        inner.insert(
            "Color Space".into(),
            PlistValue::String("Generic RGB".into()),
        );
        let mut dict = plist::Dictionary::new();
        dict.insert("Background Color".into(), PlistValue::Dictionary(inner));
        let base = ColorScheme::default();
        assert!(profile_from_dict(&dict, &base, "bad").is_err());
    }

    #[test]
    fn json_profiles_array() {
        let json = r#"[
          {"Name":"A","Background Color":{"Red Component":0,"Green Component":0,"Blue Component":0}},
          {"Name":"B","Foreground Color":{"Red Component":1,"Green Component":1,"Blue Component":1}}
        ]"#;
        let base = ColorScheme::default();
        let out = parse_iterm(json.as_bytes(), "profiles.json", &base).unwrap();
        assert_eq!(out.schemes.len(), 2);
        assert_eq!(out.schemes[0].name, "A");
        assert_eq!(out.schemes[0].background.to_hex(), "#000000");
        assert_eq!(out.schemes[1].foreground.to_hex(), "#FFFFFF");
    }

    #[test]
    fn json_profiles_wrapper() {
        let json = r#"{"Profiles":[{"Name":"Dyn","Ansi 0 Color":{"Red Component":0.5,"Green Component":0.5,"Blue Component":0.5}}]}"#;
        let base = ColorScheme::default();
        let out = parse_iterm(json.as_bytes(), "dynamic.json", &base).unwrap();
        assert_eq!(out.schemes.len(), 1);
        assert_eq!(out.schemes[0].ansi[0].to_hex(), "#808080");
    }

    #[test]
    fn new_bookmarks_prefs_fragment() {
        let mut one = plist::Dictionary::new();
        one.insert("Name".into(), PlistValue::String("One".into()));
        one.insert("Background Color".into(), sample_color(0.0, 0.0, 0.0));
        let mut broken = plist::Dictionary::new();
        broken.insert("Name".into(), PlistValue::String("Broken".into()));
        broken.insert(
            "Foreground Color".into(),
            PlistValue::String("not a dict".into()),
        );
        let mut root_d = plist::Dictionary::new();
        root_d.insert(
            "New Bookmarks".into(),
            PlistValue::Array(vec![
                PlistValue::Dictionary(one),
                PlistValue::Dictionary(broken),
            ]),
        );
        let root = PlistValue::Dictionary(root_d);
        let bytes = plist_xml_bytes(&root);
        let base = ColorScheme::default();
        let out = parse_iterm(&bytes, "prefs.plist", &base).unwrap();
        assert_eq!(out.schemes.len(), 1);
        assert_eq!(out.schemes[0].name, "One");
        assert_eq!(out.warnings.len(), 1);
    }

    #[test]
    fn binary_plist_itermcolors() {
        let mut root_d = plist::Dictionary::new();
        root_d.insert(
            "Cursor Color".into(),
            sample_color_srgb(0.9137254901960784, 0.6274509803921569, 0.22745098039215686),
        );
        let root = PlistValue::Dictionary(root_d);
        let bytes = plist_binary_bytes(&root);
        assert!(bytes.starts_with(b"bplist"));
        let base = ColorScheme::default();
        let out = parse_iterm(&bytes, "bin.itermcolors", &base).unwrap();
        assert_eq!(out.schemes[0].cursor.to_hex(), "#E9A03A");
    }

    #[test]
    fn display_p3_conversion_applies_matrix() {
        let mut inner = plist::Dictionary::new();
        inner.insert("Red Component".into(), PlistValue::Real(1.0));
        inner.insert("Green Component".into(), PlistValue::Real(0.5));
        inner.insert("Blue Component".into(), PlistValue::Real(0.0));
        inner.insert(
            "Color Space".into(),
            PlistValue::String("Display P3".into()),
        );
        let mut dict = plist::Dictionary::new();
        dict.insert("Background Color".into(), PlistValue::Dictionary(inner));
        let base = ColorScheme::default();
        let scheme = profile_from_dict(&dict, &base, "p3").unwrap();
        assert_eq!(scheme.background.to_hex(), "#FF7600");
    }

    #[test]
    fn dedup_identical_name_and_palette() {
        let a = ColorScheme {
            name: "Dup".into(),
            foreground: Rgb24::from_hex("#111111").unwrap(),
            ..ColorScheme::default()
        };
        let b = a.clone();
        let mut list = vec![a, b];
        dedup_schemes(&mut list);
        assert_eq!(list.len(), 1);
    }

    #[test]
    fn load_installed_non_macos_errors() {
        if cfg!(target_os = "macos") {
            return;
        }
        let err = load_installed_iterm(&ColorScheme::default()).unwrap_err();
        assert!(err.contains("macOS"));
    }

    #[test]
    fn rejects_oversized_input() {
        let huge = vec![b' '; MAX_IMPORT_BYTES + 1];
        let err = parse_iterm(&huge, "big.json", &ColorScheme::default()).unwrap_err();
        assert!(err.contains("too large"));
    }

    #[test]
    fn color_scheme_serde_hex_roundtrip() {
        let scheme = ColorScheme {
            name: "serde".into(),
            foreground: Rgb24::from_hex("#D8D8D8").unwrap(),
            ..ColorScheme::default()
        };
        let json = serde_json::to_string(&scheme).unwrap();
        assert!(json.contains("\"#D8D8D8\""));
        let back: ColorScheme = serde_json::from_str(&json).unwrap();
        assert_eq!(back, scheme);
    }

    #[test]
    fn minimal_fixture_file() {
        let xml = include_str!("color_fixtures/minimal.itermcolors");
        let base = ColorScheme::default();
        let out = parse_iterm(xml.as_bytes(), "minimal.itermcolors", &base).unwrap();
        assert_eq!(out.schemes.len(), 1);
        assert_eq!(out.schemes[0].foreground.to_hex(), "#D8D8D8");
        assert_eq!(out.schemes[0].background.to_hex(), "#181818");
        assert_eq!(out.schemes[0].cursor, base.cursor);
    }
}
