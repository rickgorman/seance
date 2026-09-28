//! Device-local desktop presentation prefs (`~/.config/seance/desktop.json`).
//!
//! Font family/size and app-global keyboard chords live here — not on the daemon.
//! See `CLAUDE.md` for the thin-client exception.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock, RwLock};

use gpui::Keystroke;
use serde::{Deserialize, Serialize};

pub const FONT_SIZE_DEFAULT: f32 = 12.0;
pub const FONT_SIZE_MIN: f32 = 8.0;
pub const FONT_SIZE_MAX: f32 = 32.0;

/// Stable id for each remappable global action.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AppAction {
    NewSession,
    KillPaneOrWorkspace,
    ToggleNotes,
    PinWorkspace,
    Popout,
    SelectTopWorkspace,
    ToggleOverview,
    NavigatePaneUp,
    NavigatePaneDown,
    NavigatePaneLeft,
    NavigatePaneRight,
    CycleWorkspacePrev,
    CycleWorkspaceNext,
    CyclePanePrev,
    CyclePaneNext,
    PalettePrompts,
    PaletteJump,
    ZoomPane,
    RenameWorkspace,
    ShowLastFailed,
    RailWorkspace1,
    RailWorkspace2,
    RailWorkspace3,
    RailWorkspace4,
    RailWorkspace5,
    RailWorkspace6,
    RailWorkspace7,
    RailWorkspace8,
    RailWorkspace9,
    OpenSettings,
    TermZoomIn,
    TermZoomOut,
    TermZoomReset,
}

impl AppAction {
    pub fn all_global() -> &'static [AppAction] {
        use AppAction::*;
        &[
            NewSession,
            KillPaneOrWorkspace,
            ToggleNotes,
            PinWorkspace,
            Popout,
            SelectTopWorkspace,
            ToggleOverview,
            NavigatePaneUp,
            NavigatePaneDown,
            NavigatePaneLeft,
            NavigatePaneRight,
            CycleWorkspacePrev,
            CycleWorkspaceNext,
            CyclePanePrev,
            CyclePaneNext,
            PalettePrompts,
            PaletteJump,
            ZoomPane,
            RenameWorkspace,
            ShowLastFailed,
            RailWorkspace1,
            RailWorkspace2,
            RailWorkspace3,
            RailWorkspace4,
            RailWorkspace5,
            RailWorkspace6,
            RailWorkspace7,
            RailWorkspace8,
            RailWorkspace9,
            OpenSettings,
            TermZoomIn,
            TermZoomOut,
            TermZoomReset,
        ]
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::NewSession => "New shell pane (summon)",
            Self::KillPaneOrWorkspace => "Kill pane / banish circle",
            Self::ToggleNotes => "Flip pane to notes",
            Self::PinWorkspace => "Pin / unpin circle",
            Self::Popout => "Pop pane out",
            Self::SelectTopWorkspace => "Jump to top of rail",
            Self::ToggleOverview => "Live overview map",
            Self::NavigatePaneUp => "Move focus up (spatial)",
            Self::NavigatePaneDown => "Move focus down (spatial)",
            Self::NavigatePaneLeft => "Move focus left (spatial)",
            Self::NavigatePaneRight => "Move focus right (spatial)",
            Self::CycleWorkspacePrev => "Previous circle (sidebar order)",
            Self::CycleWorkspaceNext => "Next circle (sidebar order)",
            Self::CyclePanePrev => "Previous pane in circle",
            Self::CyclePaneNext => "Next pane in circle",
            Self::PalettePrompts => "Prompt palette",
            Self::PaletteJump => "Jump palette",
            Self::ZoomPane => "Focus-zoom active pane",
            Self::RenameWorkspace => "Rename selected circle",
            Self::ShowLastFailed => "Jump to last failed command",
            Self::RailWorkspace1 => "Select rail row 1",
            Self::RailWorkspace2 => "Select rail row 2",
            Self::RailWorkspace3 => "Select rail row 3",
            Self::RailWorkspace4 => "Select rail row 4",
            Self::RailWorkspace5 => "Select rail row 5",
            Self::RailWorkspace6 => "Select rail row 6",
            Self::RailWorkspace7 => "Select rail row 7",
            Self::RailWorkspace8 => "Select rail row 8",
            Self::RailWorkspace9 => "Select rail row 9",
            Self::OpenSettings => "Open settings",
            Self::TermZoomIn => "Terminal text larger",
            Self::TermZoomOut => "Terminal text smaller",
            Self::TermZoomReset => "Reset terminal text size",
        }
    }

    fn rail_index(self) -> Option<usize> {
        match self {
            Self::RailWorkspace1 => Some(0),
            Self::RailWorkspace2 => Some(1),
            Self::RailWorkspace3 => Some(2),
            Self::RailWorkspace4 => Some(3),
            Self::RailWorkspace5 => Some(4),
            Self::RailWorkspace6 => Some(5),
            Self::RailWorkspace7 => Some(6),
            Self::RailWorkspace8 => Some(7),
            Self::RailWorkspace9 => Some(8),
            _ => None,
        }
    }
}

/// Serialized chord (modifiers + logical key name).
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Chord {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    #[serde(default)]
    pub meta: bool,
    pub key: String,
}

impl Chord {
    pub fn display(&self, mac: bool) -> String {
        let mut parts = Vec::new();
        if self.ctrl {
            parts.push("ctrl");
        }
        if self.alt {
            parts.push("alt");
        }
        if self.shift {
            parts.push("shift");
        }
        if self.meta {
            parts.push(if mac { "cmd" } else { "meta" });
        }
        parts.push(self.key.as_str());
        parts.join("+")
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DesktopPrefs {
    #[serde(default = "default_font_family")]
    pub font_family: String,
    #[serde(default = "default_font_size")]
    pub font_size: f32,
    /// Per-action override chords. Absent → built-in defaults.
    #[serde(default)]
    pub shortcuts: HashMap<String, Vec<Chord>>,
}

fn default_font_family() -> String {
    crate::term_font::FONT_FAMILY.to_string()
}

fn default_font_size() -> f32 {
    FONT_SIZE_DEFAULT
}

impl Default for DesktopPrefs {
    fn default() -> Self {
        Self {
            font_family: default_font_family(),
            font_size: FONT_SIZE_DEFAULT,
            shortcuts: HashMap::new(),
        }
    }
}

pub fn config_path() -> PathBuf {
    if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
        if !xdg.is_empty() {
            return PathBuf::from(xdg).join("seance/desktop.json");
        }
    }
    PathBuf::from(shellexpand::tilde("~/.config/seance/desktop.json").as_ref())
}

pub fn clamp_font_size(px: f32) -> f32 {
    px.clamp(FONT_SIZE_MIN, FONT_SIZE_MAX)
}

/// Normalize GPUI / persisted key names to one canonical form.
pub fn normalize_key(key: &str) -> String {
    match key {
        " " => "space".to_string(),
        "equals" | "equal" => "=".to_string(),
        "minus" | "hyphen" => "-".to_string(),
        "comma" => ",".to_string(),
        "plus" => "+".to_string(),
        "pageup" | "page_up" | "prior" => "pageup".to_string(),
        "pagedown" | "page_down" | "next" => "pagedown".to_string(),
        "arrowup" => "up".to_string(),
        "arrowdown" => "down".to_string(),
        "arrowleft" => "left".to_string(),
        "arrowright" => "right".to_string(),
        other => other.to_string(),
    }
}

pub fn normalize_chord(chord: &Chord) -> Chord {
    Chord {
        ctrl: chord.ctrl,
        alt: chord.alt,
        shift: chord.shift,
        meta: chord.meta,
        key: normalize_key(&chord.key),
    }
}

fn make_chord(ctrl: bool, meta: bool, shift: bool, key: &str) -> Chord {
    Chord {
        ctrl,
        alt: false,
        shift,
        meta,
        key: normalize_key(key),
    }
}

/// Ctrl+shift (Linux) and cmd+shift (macOS) aliases for app chords.
fn shift_chords_for(key: &str, mac: bool) -> Vec<Chord> {
    let mut out = vec![make_chord(true, false, true, key)];
    if mac {
        out.push(make_chord(false, true, true, key));
    }
    out
}

fn zoom_mod_chords_for(shift: bool, key: &str, mac: bool) -> Vec<Chord> {
    let mut out = vec![make_chord(true, false, shift, key)];
    if mac {
        out.push(make_chord(false, true, shift, key));
    }
    out
}

fn cycle_workspace_chords_for(up: bool, mac: bool) -> Vec<Chord> {
    let page_keys: &[&str] = if up {
        &["pageup", "page_up", "prior"]
    } else {
        &["pagedown", "page_down", "next"]
    };
    let mut out: Vec<Chord> = page_keys
        .iter()
        .map(|k| make_chord(true, false, false, k))
        .collect();
    if mac {
        let arrow = if up { "up" } else { "down" };
        out.push(make_chord(false, true, false, arrow));
        out.push(make_chord(
            false,
            true,
            false,
            if up { "arrowup" } else { "arrowdown" },
        ));
        for key in page_keys {
            out.push(make_chord(false, true, false, key));
        }
    }
    out
}

fn cycle_pane_chords_for(up: bool, mac: bool) -> Vec<Chord> {
    let page_keys: &[&str] = if up {
        &["pageup", "page_up", "prior"]
    } else {
        &["pagedown", "page_down", "next"]
    };
    let mut out: Vec<Chord> = page_keys
        .iter()
        .map(|k| make_chord(true, false, true, k))
        .collect();
    if mac {
        let arrow = if up { "up" } else { "down" };
        out.push(make_chord(false, true, true, arrow));
        out.push(make_chord(
            false,
            true,
            true,
            if up { "arrowup" } else { "arrowdown" },
        ));
        for key in page_keys {
            out.push(make_chord(false, true, true, key));
        }
    }
    out
}

fn spatial_vertical_chords(key: &str) -> Vec<Chord> {
    vec![make_chord(true, false, true, key)]
}

fn dedup_chords(chords: impl IntoIterator<Item = Chord>) -> Vec<Chord> {
    let mut out = Vec::new();
    for chord in chords {
        let chord = normalize_chord(&chord);
        if !out.contains(&chord) {
            out.push(chord);
        }
    }
    out
}

/// Built-in default aliases for an action (replaced entirely on rebind).
pub fn default_chords(action: AppAction) -> Vec<Chord> {
    default_chords_for_platform(action, cfg!(target_os = "macos"))
}

fn default_chords_for_platform(action: AppAction, mac: bool) -> Vec<Chord> {
    let chords = match action {
        AppAction::NewSession => shift_chords_for("n", mac),
        AppAction::KillPaneOrWorkspace => shift_chords_for("w", mac),
        AppAction::ToggleNotes => shift_chords_for("s", mac),
        AppAction::PinWorkspace => shift_chords_for("p", mac),
        AppAction::Popout => shift_chords_for("o", mac),
        AppAction::SelectTopWorkspace => shift_chords_for("home", mac),
        AppAction::ToggleOverview => {
            let mut c = shift_chords_for(" ", mac);
            c.extend(shift_chords_for("space", mac));
            c
        }
        AppAction::NavigatePaneUp => spatial_vertical_chords("up"),
        AppAction::NavigatePaneDown => spatial_vertical_chords("down"),
        AppAction::NavigatePaneLeft => shift_chords_for("left", mac),
        AppAction::NavigatePaneRight => shift_chords_for("right", mac),
        AppAction::CycleWorkspacePrev => cycle_workspace_chords_for(true, mac),
        AppAction::CycleWorkspaceNext => cycle_workspace_chords_for(false, mac),
        AppAction::CyclePanePrev => cycle_pane_chords_for(true, mac),
        AppAction::CyclePaneNext => cycle_pane_chords_for(false, mac),
        AppAction::PalettePrompts => shift_chords_for("k", mac),
        AppAction::PaletteJump => shift_chords_for("j", mac),
        AppAction::ZoomPane => {
            let mut c = shift_chords_for("z", mac);
            c.extend(shift_chords_for("m", mac));
            c
        }
        AppAction::RenameWorkspace => shift_chords_for("r", mac),
        AppAction::ShowLastFailed => shift_chords_for("f", mac),
        AppAction::RailWorkspace1 => {
            let mut c = shift_chords_for("1", mac);
            c.extend(shift_chords_for("!", mac));
            c
        }
        AppAction::RailWorkspace2 => {
            let mut c = shift_chords_for("2", mac);
            c.extend(shift_chords_for("@", mac));
            c
        }
        AppAction::RailWorkspace3 => {
            let mut c = shift_chords_for("3", mac);
            c.extend(shift_chords_for("#", mac));
            c
        }
        AppAction::RailWorkspace4 => {
            let mut c = shift_chords_for("4", mac);
            c.extend(shift_chords_for("$", mac));
            c
        }
        AppAction::RailWorkspace5 => {
            let mut c = shift_chords_for("5", mac);
            c.extend(shift_chords_for("%", mac));
            c
        }
        AppAction::RailWorkspace6 => {
            let mut c = shift_chords_for("6", mac);
            c.extend(shift_chords_for("^", mac));
            c
        }
        AppAction::RailWorkspace7 => {
            let mut c = shift_chords_for("7", mac);
            c.extend(shift_chords_for("&", mac));
            c
        }
        AppAction::RailWorkspace8 => {
            let mut c = shift_chords_for("8", mac);
            c.extend(shift_chords_for("*", mac));
            c
        }
        AppAction::RailWorkspace9 => {
            let mut c = shift_chords_for("9", mac);
            c.extend(shift_chords_for("(", mac));
            c
        }
        AppAction::OpenSettings => zoom_mod_chords_for(false, ",", mac),
        AppAction::TermZoomIn => {
            let mut c = zoom_mod_chords_for(false, "=", mac);
            c.extend(zoom_mod_chords_for(false, "+", mac));
            c.extend(zoom_mod_chords_for(true, "=", mac));
            c.extend(zoom_mod_chords_for(true, "+", mac));
            c
        }
        AppAction::TermZoomOut => zoom_mod_chords_for(false, "-", mac),
        AppAction::TermZoomReset => zoom_mod_chords_for(false, "0", mac),
    };
    dedup_chords(chords)
}

pub fn action_key(action: AppAction) -> String {
    serde_json::to_value(action)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_else(|| format!("{action:?}"))
}

pub fn chords_for_action(prefs: &DesktopPrefs, action: AppAction) -> Vec<Chord> {
    if action == AppAction::OpenSettings {
        return default_chords(action);
    }
    let key = action_key(action);
    let chords = prefs
        .shortcuts
        .get(&key)
        .filter(|v| !v.is_empty())
        .cloned()
        .unwrap_or_else(|| default_chords(action));
    dedup_chords(chords)
}

pub fn keystroke_to_chord(ks: &Keystroke) -> Chord {
    normalize_chord(&Chord {
        ctrl: ks.modifiers.control,
        alt: ks.modifiers.alt,
        shift: ks.modifiers.shift,
        meta: ks.modifiers.platform,
        key: ks.key.to_string(),
    })
}

pub fn chord_matches_keystroke(chord: &Chord, ks: &Keystroke) -> bool {
    keystroke_to_chord(ks) == normalize_chord(chord)
}

fn is_function_key(key: &str) -> bool {
    let key = normalize_key(key);
    let rest = key.strip_prefix('f').or_else(|| key.strip_prefix('F'));
    rest.and_then(|n| n.parse::<u8>().ok())
        .is_some_and(|n| (1..=35).contains(&n))
}

fn is_known_global_key(key: &str) -> bool {
    let key = normalize_key(key);
    if key.len() == 1 {
        let c = key.chars().next().unwrap();
        return c.is_ascii_graphic();
    }
    matches!(
        key.as_str(),
        "space"
            | "enter"
            | "return"
            | "tab"
            | "backspace"
            | "delete"
            | "up"
            | "down"
            | "left"
            | "right"
            | "home"
            | "end"
            | "pageup"
            | "pagedown"
            | "insert"
            | "back"
            | "forward"
            | "escape"
    ) || is_function_key(&key)
}

/// Chords that must never be remapped to another action.
pub fn is_protected_chord(chord: &Chord, mac: bool) -> bool {
    let c = normalize_chord(chord);
    for dc in default_chords(AppAction::OpenSettings) {
        if normalize_chord(&dc) == c {
            return true;
        }
    }
    let quit = normalize_chord(&Chord {
        ctrl: !mac,
        alt: false,
        shift: false,
        meta: mac,
        key: "q".to_string(),
    });
    if c == quit {
        return true;
    }
    let clipboard: Vec<Chord> = if mac {
        vec![
            make_chord(false, true, false, "c"),
            make_chord(false, true, false, "v"),
            make_chord(true, false, true, "c"),
            make_chord(true, false, true, "v"),
        ]
    } else {
        vec![
            make_chord(true, false, true, "c"),
            make_chord(true, false, true, "v"),
        ]
    };
    clipboard.iter().any(|p| normalize_chord(p) == c)
}

/// Bare printable keys must not become global shortcuts (PTY typing).
pub fn chord_is_safe_global(chord: &Chord) -> bool {
    let c = normalize_chord(chord);
    if c.key.is_empty() {
        return false;
    }
    if !is_known_global_key(&c.key)
        || matches!(
            c.key.as_str(),
            "control" | "ctrl" | "alt" | "shift" | "meta" | "platform" | "function"
        )
        || c.key == "escape"
    {
        return false;
    }
    let has_mod = c.ctrl || c.alt || c.meta;
    if !has_mod && !c.shift {
        return is_function_key(&c.key);
    }
    if !has_mod && c.shift {
        return is_function_key(&c.key);
    }
    if has_mod {
        return true;
    }
    false
}

pub fn find_action_for_keystroke_in_prefs(
    prefs: &DesktopPrefs,
    ks: &Keystroke,
) -> Option<AppAction> {
    for action in AppAction::all_global() {
        for chord in chords_for_action(prefs, *action) {
            if chord_matches_keystroke(&chord, ks) {
                return Some(*action);
            }
        }
    }
    None
}

pub fn find_action_for_keystroke(ks: &Keystroke, _mac: bool) -> Option<AppAction> {
    if !chord_is_safe_global(&keystroke_to_chord(ks)) {
        return None;
    }
    let prefs = desktop_prefs().read().ok()?;
    find_action_for_keystroke_in_prefs(&prefs, ks)
}

#[derive(Debug, PartialEq, Eq)]
pub enum BindError {
    Protected,
    UnsafeBareKey,
    Conflict(AppAction),
}

fn validate_effective_shortcuts(
    shortcuts: &HashMap<String, Vec<Chord>>,
    mac: bool,
) -> Result<HashMap<String, Vec<Chord>>, BindError> {
    let mut normalized = HashMap::new();
    for action in AppAction::all_global() {
        if *action == AppAction::OpenSettings {
            continue;
        }
        let key = action_key(*action);
        let Some(chords) = shortcuts.get(&key) else {
            continue;
        };
        let chords = dedup_chords(chords.clone());
        for chord in &chords {
            if is_protected_chord(chord, mac) {
                return Err(BindError::Protected);
            }
            if !chord_is_safe_global(chord) {
                return Err(BindError::UnsafeBareKey);
            }
        }
        if !chords.is_empty() {
            normalized.insert(key, chords);
        }
    }

    let mut effective = DesktopPrefs::default();
    effective.shortcuts = normalized.clone();
    let mut seen = Vec::new();
    for action in AppAction::all_global() {
        for chord in chords_for_action(&effective, *action) {
            let chord = normalize_chord(&chord);
            if let Some((other, _)) = seen.iter().find(|(_, used)| *used == chord) {
                if *other != *action {
                    return Err(BindError::Conflict(*other));
                }
            }
            seen.push((*action, chord));
        }
    }
    Ok(normalized)
}

pub fn validate_bind(
    prefs: &DesktopPrefs,
    action: AppAction,
    chords: Vec<Chord>,
    mac: bool,
) -> Result<Vec<Chord>, BindError> {
    let normalized = dedup_chords(chords.into_iter().map(|c| normalize_chord(&c)));
    if action == AppAction::OpenSettings {
        return Err(BindError::Protected);
    }
    let mut candidate = prefs.shortcuts.clone();
    if normalized.is_empty() {
        candidate.remove(&action_key(action));
    } else {
        candidate.insert(action_key(action), normalized.clone());
    }
    validate_effective_shortcuts(&candidate, mac)?;
    Ok(normalized)
}

pub fn try_bind(action: AppAction, chords: Vec<Chord>, mac: bool) -> Result<(), BindError> {
    let prefs = desktop_prefs().read().unwrap().clone();
    let normalized = validate_bind(&prefs, action, chords, mac)?;
    let mut prefs = desktop_prefs().write().unwrap();
    if normalized.is_empty() {
        prefs.shortcuts.remove(&action_key(action));
    } else {
        prefs.shortcuts.insert(action_key(action), normalized);
    }
    schedule_save();
    Ok(())
}

pub fn reset_action_in_prefs(
    prefs: &mut DesktopPrefs,
    action: AppAction,
    mac: bool,
) -> Result<(), BindError> {
    if action == AppAction::OpenSettings {
        return Ok(());
    }
    let defaults = default_chords(action);
    for other in AppAction::all_global() {
        if *other != action
            && chords_for_action(prefs, *other)
                .iter()
                .any(|chord| defaults.contains(&normalize_chord(chord)))
        {
            return Err(BindError::Conflict(*other));
        }
    }
    let mut candidate = prefs.shortcuts.clone();
    candidate.remove(&action_key(action));
    prefs.shortcuts = validate_effective_shortcuts(&candidate, mac)?;
    Ok(())
}

pub fn reset_action(action: AppAction) -> Result<(), BindError> {
    let mut prefs = desktop_prefs().write().unwrap();
    let result = reset_action_in_prefs(&mut prefs, action, cfg!(target_os = "macos"));
    if result.is_ok() {
        schedule_save();
    }
    result
}

pub fn reset_all_shortcuts() {
    let mut prefs = desktop_prefs().write().unwrap();
    prefs.shortcuts.clear();
    schedule_save();
}

pub fn parse_prefs_json(
    bytes: &str,
    installed: &HashSet<String>,
    text_system: Option<&gpui::TextSystem>,
) -> DesktopPrefs {
    let raw: DesktopPrefs = serde_json::from_str(bytes).unwrap_or_default();
    sanitize_prefs(raw, installed, text_system)
}

fn sanitize_shortcuts(
    shortcuts: HashMap<String, Vec<Chord>>,
    mac: bool,
) -> HashMap<String, Vec<Chord>> {
    match validate_effective_shortcuts(&shortcuts, mac) {
        Ok(shortcuts) => shortcuts,
        Err(error) => {
            eprintln!("ignoring invalid desktop shortcut overrides: {error:?}");
            HashMap::new()
        }
    }
}

fn sanitize_prefs(
    mut prefs: DesktopPrefs,
    installed: &HashSet<String>,
    text_system: Option<&gpui::TextSystem>,
) -> DesktopPrefs {
    prefs.font_size = clamp_font_size(prefs.font_size);
    let family_ok = installed.contains(&prefs.font_family)
        && text_system.is_none_or(|ts| {
            crate::term_font::probe_monospace(ts, &prefs.font_family, prefs.font_size)
        });
    if !family_ok {
        prefs.font_family = crate::term_font::select_installed_term_family(installed);
    }
    let mac = cfg!(target_os = "macos");
    prefs.shortcuts = sanitize_shortcuts(prefs.shortcuts, mac);
    prefs
}

pub fn load_from_disk(
    installed: &HashSet<String>,
    text_system: Option<&gpui::TextSystem>,
) -> DesktopPrefs {
    match std::fs::read_to_string(config_path()) {
        Ok(bytes) => parse_prefs_json(&bytes, installed, text_system),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            sanitize_prefs(DesktopPrefs::default(), installed, text_system)
        }
        Err(_) => sanitize_prefs(DesktopPrefs::default(), installed, text_system),
    }
}

static DESKTOP: OnceLock<RwLock<DesktopPrefs>> = OnceLock::new();
static SAVE_ERROR: Mutex<Option<String>> = Mutex::new(None);
static SAVE_TX: OnceLock<std::sync::mpsc::Sender<u64>> = OnceLock::new();

pub fn desktop_prefs() -> &'static RwLock<DesktopPrefs> {
    DESKTOP.get_or_init(|| RwLock::new(DesktopPrefs::default()))
}

pub fn init(installed: &HashSet<String>, text_system: &gpui::TextSystem) {
    let prefs = load_from_disk(installed, Some(text_system));
    let _ = DESKTOP.set(RwLock::new(prefs.clone()));
    crate::term_font::apply_appearance(&prefs.font_family, prefs.font_size);
    spawn_save_thread();
}

pub fn save_error() -> Option<String> {
    SAVE_ERROR.lock().ok().and_then(|g| g.clone())
}

pub fn adjust_font_size(delta: i32) {
    let mut prefs = desktop_prefs().write().unwrap();
    let next = clamp_font_size(prefs.font_size + delta as f32);
    if next == prefs.font_size {
        return;
    }
    prefs.font_size = next;
    crate::term_font::apply_appearance(&prefs.font_family, prefs.font_size);
    schedule_save();
}

pub fn reset_font_defaults(installed: &HashSet<String>) {
    let family = crate::term_font::select_installed_term_family(installed);
    let mut prefs = desktop_prefs().write().unwrap();
    prefs.font_family = family;
    prefs.font_size = FONT_SIZE_DEFAULT;
    crate::term_font::apply_appearance(&prefs.font_family, prefs.font_size);
    schedule_save();
}

pub fn reset_terminal_font_size() {
    let mut prefs = desktop_prefs().write().unwrap();
    prefs.font_size = FONT_SIZE_DEFAULT;
    crate::term_font::apply_appearance(&prefs.font_family, prefs.font_size);
    schedule_save();
}

pub fn apply_font_from_settings(family: String, size: f32, installed: &HashSet<String>) {
    let family = if installed.contains(&family) {
        family
    } else {
        crate::term_font::select_installed_term_family(installed)
    };
    let mut prefs = desktop_prefs().write().unwrap();
    prefs.font_family = family;
    prefs.font_size = clamp_font_size(size);
    crate::term_font::apply_appearance(&prefs.font_family, prefs.font_size);
    schedule_save();
}

fn spawn_save_thread() {
    let _ = SAVE_TX.get_or_init(|| {
        let (tx, rx) = std::sync::mpsc::channel::<u64>();
        std::thread::Builder::new()
            .name("seance-desktop-prefs-save".into())
            .spawn(move || {
                let mut last_gen = 0u64;
                while let Ok(gen) = rx.recv() {
                    let mut newest = gen;
                    while let Ok(g) = rx.try_recv() {
                        newest = g;
                    }
                    if newest < last_gen {
                        continue;
                    }
                    last_gen = newest;
                    let prefs = desktop_prefs().read().unwrap().clone();
                    if let Err(e) = write_prefs_atomic(&prefs) {
                        if let Ok(mut err) = SAVE_ERROR.lock() {
                            *err = Some(e.to_string());
                        }
                    } else if let Ok(mut err) = SAVE_ERROR.lock() {
                        *err = None;
                    }
                }
            })
            .ok();
        tx
    });
}

static SAVE_GEN: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn schedule_save() {
    spawn_save_thread();
    let gen = SAVE_GEN.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
    if let Some(tx) = SAVE_TX.get() {
        let _ = tx.send(gen);
    }
}

fn write_prefs_atomic(prefs: &DesktopPrefs) -> anyhow::Result<()> {
    let path = config_path();
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let json = serde_json::to_string_pretty(prefs)?;
    let tmp = path.with_extension(format!("json.{}.tmp", std::process::id()));
    std::fs::write(&tmp, json)?;
    std::fs::rename(&tmp, &path)?;
    Ok(())
}

pub fn rail_index_for_action(action: AppAction) -> Option<usize> {
    action.rail_index()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shift_chord(key: &str) -> Chord {
        shift_chords_for(key, cfg!(target_os = "macos"))[0].clone()
    }

    fn ks(chord: &Chord) -> Keystroke {
        Keystroke {
            modifiers: gpui::Modifiers {
                control: chord.ctrl,
                alt: chord.alt,
                shift: chord.shift,
                platform: chord.meta,
                function: false,
            },
            key: chord.key.clone().into(),
            key_char: None,
        }
    }

    #[test]
    fn defaults_roundtrip_in_json() {
        let prefs = DesktopPrefs::default();
        let json = serde_json::to_string(&prefs).unwrap();
        let back: DesktopPrefs = serde_json::from_str(&json).unwrap();
        assert_eq!(back.font_size, FONT_SIZE_DEFAULT);
    }

    #[test]
    fn every_default_chord_dispatches_to_its_declared_action() {
        let prefs = DesktopPrefs::default();
        for action in AppAction::all_global() {
            for chord in default_chords(*action) {
                assert_eq!(
                    find_action_for_keystroke_in_prefs(&prefs, &ks(&chord)),
                    Some(*action),
                    "default chord {:?} dispatched incorrectly",
                    chord
                );
            }
        }
    }

    #[test]
    fn mac_cycle_and_spatial_defaults_keep_their_modifier_roles() {
        let spatial_up = default_chords_for_platform(AppAction::NavigatePaneUp, true);
        assert!(spatial_up.contains(&make_chord(true, false, true, "up")));
        assert!(!spatial_up.contains(&make_chord(false, true, true, "up")));

        let spatial_left = default_chords_for_platform(AppAction::NavigatePaneLeft, true);
        assert!(spatial_left.contains(&make_chord(true, false, true, "left")));
        assert!(spatial_left.contains(&make_chord(false, true, true, "left")));

        let workspace_prev = default_chords_for_platform(AppAction::CycleWorkspacePrev, true);
        assert!(workspace_prev.contains(&make_chord(false, true, false, "pageup")));
        assert!(workspace_prev.contains(&make_chord(false, true, false, "up")));

        let pane_prev = default_chords_for_platform(AppAction::CyclePanePrev, true);
        assert!(pane_prev.contains(&make_chord(false, true, true, "pageup")));
        assert!(pane_prev.contains(&make_chord(false, true, true, "up")));
    }

    #[test]
    fn invalid_font_size_clamps() {
        let installed: HashSet<String> = ["Menlo"].into_iter().map(str::to_string).collect();
        let p = parse_prefs_json(
            r#"{"font_size": 99, "font_family": "Menlo"}"#,
            &installed,
            None,
        );
        assert_eq!(p.font_size, FONT_SIZE_MAX);
        let p2 = parse_prefs_json(r#"{"font_size": 1}"#, &installed, None);
        assert_eq!(p2.font_size, FONT_SIZE_MIN);
    }

    #[test]
    fn missing_font_falls_back_to_installed() {
        let installed: HashSet<String> = ["Menlo"].into_iter().map(str::to_string).collect();
        let p = parse_prefs_json(r#"{"font_family": "NoSuchFont"}"#, &installed, None);
        assert_eq!(p.font_family, "Menlo");
    }

    #[test]
    fn rebind_replaces_defaults() {
        let mut prefs = DesktopPrefs::default();
        let custom = vec![shift_chord("x")];
        prefs
            .shortcuts
            .insert(action_key(AppAction::NewSession), custom.clone());
        let chords = chords_for_action(&prefs, AppAction::NewSession);
        assert_eq!(chords, custom);
        assert!(!chords.iter().any(|c| c.key == "n"));
    }

    #[test]
    fn conflict_rejects_second_action() {
        let mac = cfg!(target_os = "macos");
        let prefs = DesktopPrefs::default();
        let taken = default_chords(AppAction::NewSession);
        assert!(matches!(
            validate_bind(&prefs, AppAction::PaletteJump, taken, mac),
            Err(BindError::Conflict(AppAction::NewSession))
        ));
    }

    #[test]
    fn bare_key_rejected() {
        let mac = cfg!(target_os = "macos");
        let bare = Chord {
            ctrl: false,
            alt: false,
            shift: false,
            meta: false,
            key: "k".to_string(),
        };
        assert!(matches!(
            validate_bind(
                &DesktopPrefs::default(),
                AppAction::PaletteJump,
                vec![bare],
                mac
            ),
            Err(BindError::UnsafeBareKey)
        ));
    }

    #[test]
    fn shift_only_printable_rejected() {
        let mac = cfg!(target_os = "macos");
        let bare = Chord {
            ctrl: false,
            alt: false,
            shift: true,
            meta: false,
            key: "K".to_string(),
        };
        assert!(matches!(
            validate_bind(
                &DesktopPrefs::default(),
                AppAction::PaletteJump,
                vec![bare],
                mac
            ),
            Err(BindError::UnsafeBareKey)
        ));
    }

    #[test]
    fn custom_binding_without_shift_dispatches() {
        let mut prefs = DesktopPrefs::default();
        let custom = Chord {
            ctrl: true,
            alt: true,
            shift: false,
            meta: false,
            key: "j".to_string(),
        };
        prefs
            .shortcuts
            .insert(action_key(AppAction::PaletteJump), vec![custom.clone()]);
        let event = ks(&custom);
        assert_eq!(
            find_action_for_keystroke_in_prefs(&prefs, &event),
            Some(AppAction::PaletteJump)
        );
    }

    #[test]
    fn custom_ctrl_and_cmd_bindings_are_distinct() {
        let mut prefs = DesktopPrefs::default();
        let ctrl = make_chord(true, false, false, "j");
        let cmd = make_chord(false, true, false, "j");
        prefs
            .shortcuts
            .insert(action_key(AppAction::PaletteJump), vec![ctrl.clone()]);
        assert_eq!(
            find_action_for_keystroke_in_prefs(&prefs, &ks(&ctrl)),
            Some(AppAction::PaletteJump)
        );
        assert_ne!(
            find_action_for_keystroke_in_prefs(&prefs, &ks(&cmd)),
            Some(AppAction::PaletteJump)
        );
    }

    #[test]
    fn zoom_in_matches_gpui_equals_key() {
        let (chord, event) = if cfg!(target_os = "macos") {
            (
                make_chord(false, true, false, "="),
                Keystroke {
                    modifiers: gpui::Modifiers {
                        control: false,
                        alt: false,
                        shift: false,
                        platform: true,
                        function: false,
                    },
                    key: "=".into(),
                    key_char: None,
                },
            )
        } else {
            (
                make_chord(true, false, false, "="),
                Keystroke {
                    modifiers: gpui::Modifiers {
                        control: true,
                        alt: false,
                        shift: false,
                        platform: false,
                        function: false,
                    },
                    key: "=".into(),
                    key_char: None,
                },
            )
        };
        assert!(chord_matches_keystroke(&chord, &event));
        let prefs = DesktopPrefs::default();
        assert_eq!(
            find_action_for_keystroke_in_prefs(&prefs, &event),
            Some(AppAction::TermZoomIn)
        );
        assert_eq!(
            find_action_for_keystroke_in_prefs(
                &prefs,
                &Keystroke {
                    key: "+".into(),
                    ..event
                }
            ),
            Some(AppAction::TermZoomIn)
        );
    }

    #[test]
    fn mac_clipboard_chords_protected() {
        let mac = true;
        let copy = make_chord(false, true, false, "c");
        assert!(is_protected_chord(&copy, mac));
        assert!(is_protected_chord(&make_chord(true, false, true, "c"), mac));
        assert!(is_protected_chord(&make_chord(true, false, true, "v"), mac));
        let linux_copy = make_chord(true, false, false, "c");
        assert!(!is_protected_chord(&linux_copy, mac));
    }

    #[test]
    fn recovery_settings_override_is_ignored() {
        let mut prefs = DesktopPrefs::default();
        prefs.shortcuts.insert(
            action_key(AppAction::OpenSettings),
            vec![make_chord(true, false, false, "x")],
        );
        assert_eq!(
            chords_for_action(&prefs, AppAction::OpenSettings),
            default_chords(AppAction::OpenSettings)
        );
        assert!(matches!(
            validate_bind(
                &DesktopPrefs::default(),
                AppAction::OpenSettings,
                vec![make_chord(true, false, false, "x")],
                cfg!(target_os = "macos")
            ),
            Err(BindError::Protected)
        ));
    }

    #[test]
    fn valid_two_action_reassignment_survives_json_reload() {
        let first = default_chords(AppAction::NewSession)[0].clone();
        let second = default_chords(AppAction::PaletteJump)[0].clone();
        let mut prefs = DesktopPrefs::default();
        prefs
            .shortcuts
            .insert(action_key(AppAction::NewSession), vec![second.clone()]);
        prefs
            .shortcuts
            .insert(action_key(AppAction::PaletteJump), vec![first.clone()]);

        let json = serde_json::to_string(&prefs).unwrap();
        let installed: HashSet<String> = ["Menlo"].into_iter().map(str::to_string).collect();
        let reloaded = parse_prefs_json(&json, &installed, None);
        assert_eq!(
            reloaded.shortcuts.get(&action_key(AppAction::NewSession)),
            Some(&vec![second])
        );
        assert_eq!(
            reloaded.shortcuts.get(&action_key(AppAction::PaletteJump)),
            Some(&vec![first])
        );
    }

    #[test]
    fn reset_rejects_a_freed_default_used_by_another_action() {
        let action = AppAction::NewSession;
        let other = AppAction::PaletteJump;
        let mut prefs = DesktopPrefs::default();
        prefs
            .shortcuts
            .insert(action_key(action), vec![make_chord(true, false, true, "x")]);
        prefs
            .shortcuts
            .insert(action_key(other), default_chords(action));
        assert_eq!(
            reset_action_in_prefs(&mut prefs, action, cfg!(target_os = "macos")),
            Err(BindError::Conflict(other))
        );
        assert!(prefs.shortcuts.contains_key(&action_key(action)));
    }

    #[test]
    fn unsafe_global_chords_reject_escape_unknown_and_modifier_only_keys() {
        for key in ["escape", "control", "mystery-key"] {
            assert!(!chord_is_safe_global(&make_chord(true, false, false, key)));
        }
    }

    #[test]
    fn cycle_rebind_drops_old_page_alias() {
        let mut prefs = DesktopPrefs::default();
        let custom = vec![make_chord(true, false, false, "tab")];
        prefs
            .shortcuts
            .insert(action_key(AppAction::CycleWorkspaceNext), custom.clone());
        let chords = chords_for_action(&prefs, AppAction::CycleWorkspaceNext);
        assert_eq!(chords, custom);
        assert!(!chords.iter().any(|c| c.key == "pagedown"));
    }

    #[test]
    fn chord_display_uses_cmd_on_mac() {
        let c = Chord {
            ctrl: false,
            meta: true,
            alt: false,
            shift: false,
            key: ",".into(),
        };
        assert!(c.display(true).contains("cmd"));
    }
}
