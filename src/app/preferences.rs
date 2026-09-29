//! Device-local desktop presentation prefs (`~/.config/seance/desktop.json`).
//!
//! Font family/size and app-global keyboard chords live here — not on the daemon.
//! See `CLAUDE.md` for the thin-client exception.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock, RwLock};

use gpui::Keystroke;
use serde::{Deserialize, Serialize};

use super::colors::{self, ColorScheme};

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

/// Legacy (0.26.3) OS window dedicated to a single workspace slug. Read once
/// to seed [`DesktopPrefs::seances`]; never registered or edited after that.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceWindowDefinition {
    pub workspace: String,
    #[serde(default)]
    pub shortcut: Option<Chord>,
}

/// A named Seance: its own OS window and hotkey, showing a user-chosen set of
/// circles out of the shared catalog. `id` is the identity — hotkey
/// registrations, live windows and Settings rows all key on it — and never
/// changes; `name` is only a label. A circle belongs to at most one Seance;
/// circles in none of them are the main window's.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SeanceDef {
    pub id: String,
    #[serde(default)]
    pub name: String,
    /// Circle slugs (slugs never change on rename, so membership doesn't either).
    #[serde(default)]
    pub members: Vec<String>,
    #[serde(default)]
    pub shortcut: Option<Chord>,
    /// Float over the current Space and hand focus back on hide (macOS).
    #[serde(default)]
    pub overlay: bool,
}

/// A window that can carry a global show/hide hotkey — the single lane every
/// conflict check, registration and Settings capture goes through.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum WindowSlot {
    Main,
    Seance(String),
}

/// Which circles a window shows, derived from prefs alone (never from what
/// the connection happens to be subscribed to).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Projection {
    All,
    Only(BTreeSet<String>),
    Except(BTreeSet<String>),
}

impl Projection {
    pub fn admits(&self, workspace: &str) -> bool {
        match self {
            Projection::All => true,
            Projection::Only(set) => set.contains(workspace),
            Projection::Except(set) => !set.contains(workspace),
        }
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
    /// System-wide show/hide for the primary Seance window (macOS).
    #[serde(default)]
    pub main_window_hotkey: Option<Chord>,
    /// Float the main window over the current Space (macOS).
    #[serde(default)]
    pub main_window_overlay: bool,
    /// Legacy single-circle windows, kept verbatim so a rollback still finds
    /// them. Inactive once `seances` exists.
    #[serde(default)]
    pub workspace_windows: Vec<WorkspaceWindowDefinition>,
    /// Named Seances. `None` = not yet migrated from `workspace_windows`;
    /// `Some` — even empty — is authoritative, so deleting every Seance
    /// doesn't bring the legacy windows back.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seances: Option<Vec<SeanceDef>>,
    /// Terminal ANSI / default colors for native panes (device-local).
    #[serde(default)]
    pub terminal_color_scheme: ColorScheme,
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
            main_window_hotkey: None,
            main_window_overlay: false,
            workspace_windows: Vec::new(),
            seances: None,
            terminal_color_scheme: ColorScheme::default(),
        }
    }
}

impl DesktopPrefs {
    pub fn seance_defs(&self) -> &[SeanceDef] {
        self.seances.as_deref().unwrap_or(&[])
    }

    pub fn seance(&self, id: &str) -> Option<&SeanceDef> {
        self.seance_defs().iter().find(|d| d.id == id)
    }

    fn seance_mut(&mut self, id: &str) -> Option<&mut SeanceDef> {
        self.seances.as_mut()?.iter_mut().find(|d| d.id == id)
    }

    /// The Seance a circle belongs to, if any (None = main window).
    pub fn owner_of(&self, workspace: &str) -> Option<&SeanceDef> {
        self.seance_defs()
            .iter()
            .find(|d| d.members.iter().any(|m| m == workspace))
    }

    pub fn overlay_for(&self, slot: &WindowSlot) -> bool {
        match slot {
            WindowSlot::Main => self.main_window_overlay,
            WindowSlot::Seance(id) => self.seance(id).is_some_and(|d| d.overlay),
        }
    }

    /// Human label for a slot (conflict messages, window titles).
    pub fn slot_label(&self, slot: &WindowSlot) -> String {
        match slot {
            WindowSlot::Main => "main window".into(),
            WindowSlot::Seance(id) => self
                .seance(id)
                .map(|d| d.name.clone())
                .unwrap_or_else(|| id.clone()),
        }
    }
}

/// Every bound window hotkey, main first. Legacy `workspace_windows` are not
/// here: after migration they are inert.
pub fn window_hotkey_slots(prefs: &DesktopPrefs) -> Vec<(WindowSlot, Chord)> {
    let mut out = Vec::new();
    if let Some(c) = prefs.main_window_hotkey.clone() {
        out.push((WindowSlot::Main, c));
    }
    for def in prefs.seance_defs() {
        if let Some(c) = def.shortcut.clone() {
            out.push((WindowSlot::Seance(def.id.clone()), c));
        }
    }
    out
}

/// Main shows every circle no Seance claims; a Seance shows exactly its
/// members (a missing id shows nothing — never a guess).
pub fn projection_for(prefs: &DesktopPrefs, seance: Option<&str>) -> Projection {
    match seance {
        None => {
            let claimed: BTreeSet<String> = prefs
                .seance_defs()
                .iter()
                .flat_map(|d| d.members.iter().cloned())
                .collect();
            if claimed.is_empty() {
                Projection::All
            } else {
                Projection::Except(claimed)
            }
        }
        Some(id) => Projection::Only(
            prefs
                .seance(id)
                .map(|d| d.members.iter().cloned().collect())
                .unwrap_or_default(),
        ),
    }
}

/// Fresh immutable id: time-based, bumped past any id already in use.
fn fresh_seance_id(prefs: &DesktopPrefs, now: u64) -> String {
    let mut n = now;
    loop {
        let id = format!("s{n:x}");
        if prefs.seance(&id).is_none() {
            return id;
        }
        n += 1;
    }
}

fn clean_seance_name(
    prefs: &DesktopPrefs,
    name: &str,
    skip: Option<&str>,
) -> Result<String, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("Give the Seance a name.".into());
    }
    let taken = prefs
        .seance_defs()
        .iter()
        .filter(|d| Some(d.id.as_str()) != skip)
        .any(|d| d.name.eq_ignore_ascii_case(name));
    if taken {
        return Err(format!("A Seance named “{name}” already exists."));
    }
    Ok(name.to_string())
}

/// Create an empty named Seance; returns its id.
pub fn create_seance_in(prefs: &mut DesktopPrefs, name: &str, now: u64) -> Result<String, String> {
    let name = clean_seance_name(prefs, name, None)?;
    let id = fresh_seance_id(prefs, now);
    prefs.seances.get_or_insert_with(Vec::new).push(SeanceDef {
        id: id.clone(),
        name,
        members: Vec::new(),
        shortcut: None,
        overlay: false,
    });
    Ok(id)
}

pub fn rename_seance_in(prefs: &mut DesktopPrefs, id: &str, name: &str) -> Result<(), String> {
    let name = clean_seance_name(prefs, name, Some(id))?;
    let def = prefs
        .seance_mut(id)
        .ok_or_else(|| "That Seance no longer exists.".to_string())?;
    def.name = name;
    Ok(())
}

/// Drop a Seance. Its circles simply become unassigned (main window's).
pub fn remove_seance_in(prefs: &mut DesktopPrefs, id: &str) -> Option<SeanceDef> {
    let list = prefs.seances.as_mut()?;
    let pos = list.iter().position(|d| d.id == id)?;
    Some(list.remove(pos))
}

/// Move a circle into `target` (None = back to the main window). Membership
/// is exclusive: the circle leaves whichever Seance held it. Returns whether
/// anything changed; an unknown target changes nothing.
pub fn assign_circle_in(prefs: &mut DesktopPrefs, workspace: &str, target: Option<&str>) -> bool {
    if let Some(id) = target {
        if prefs.seance(id).is_none() {
            return false;
        }
        if prefs
            .seance(id)
            .is_some_and(|d| d.members.iter().any(|m| m == workspace))
        {
            return false;
        }
    }
    let mut changed = false;
    if let Some(list) = prefs.seances.as_mut() {
        for def in list.iter_mut() {
            let before = def.members.len();
            def.members.retain(|m| m != workspace);
            changed |= def.members.len() != before;
        }
    }
    if let Some(def) = target.and_then(|id| prefs.seance_mut(id)) {
        def.members.push(workspace.to_string());
        changed = true;
    }
    changed
}

pub fn set_overlay_in(prefs: &mut DesktopPrefs, slot: &WindowSlot, on: bool) -> bool {
    match slot {
        WindowSlot::Main => {
            let changed = prefs.main_window_overlay != on;
            prefs.main_window_overlay = on;
            changed
        }
        WindowSlot::Seance(id) => match prefs.seance_mut(id) {
            Some(def) if def.overlay != on => {
                def.overlay = on;
                true
            }
            _ => false,
        },
    }
}

/// Every legacy window becomes a single-circle Seance. The id is derived from
/// the slug, so re-parsing an unsaved file gives the same ids.
fn seances_from_legacy(legacy: &[WorkspaceWindowDefinition]) -> Vec<SeanceDef> {
    legacy
        .iter()
        .filter_map(|d| {
            let slug = d.workspace.trim();
            (!slug.is_empty()).then(|| SeanceDef {
                id: format!("ws-{slug}"),
                name: slug.to_string(),
                members: vec![slug.to_string()],
                shortcut: d.shortcut.clone(),
                overlay: false,
            })
        })
        .collect()
}

/// Migrate once, then repair what a hand edit (or an older build) could leave
/// behind: duplicate ids, blank names, a circle claimed twice (first claim
/// wins), and window hotkeys that collide — only the offending row's hotkey
/// is dropped, never the rest.
fn sanitize_seances(prefs: &mut DesktopPrefs, mac: bool) {
    let raw = match prefs.seances.take() {
        Some(list) => list,
        None => seances_from_legacy(&prefs.workspace_windows),
    };
    let mut out: Vec<SeanceDef> = Vec::new();
    let mut claimed = HashSet::new();
    for mut def in raw {
        def.id = def.id.trim().to_string();
        if def.id.is_empty() || out.iter().any(|d| d.id == def.id) {
            continue;
        }
        def.name = def.name.trim().to_string();
        if def.name.is_empty() {
            def.name = def.id.clone();
        }
        def.members = std::mem::take(&mut def.members)
            .into_iter()
            .map(|m| m.trim().to_string())
            .filter(|m| !m.is_empty() && claimed.insert(m.clone()))
            .collect();
        if let Some(chord) = def.shortcut.take() {
            let mut scratch = prefs.clone();
            scratch.seances = Some(out.clone());
            match validate_window_hotkey_candidate(&scratch, &chord, None, mac) {
                Ok(()) => def.shortcut = Some(normalize_chord(&chord)),
                Err(e) => eprintln!("ignoring window hotkey for Seance “{}”: {e:?}", def.name),
            }
        }
        out.push(def);
    }
    prefs.seances = Some(out);
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

pub(crate) fn make_chord(ctrl: bool, meta: bool, shift: bool, key: &str) -> Chord {
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
    ConflictWindowHotkey,
}

pub fn schedule_desktop_save() {
    schedule_save();
}

fn all_window_hotkey_chords(prefs: &DesktopPrefs) -> Vec<Chord> {
    window_hotkey_slots(prefs)
        .into_iter()
        .map(|(_, c)| normalize_chord(&c))
        .collect()
}

/// `skip` is the slot being rebound — its own current chord isn't a conflict.
pub fn validate_window_hotkey_candidate(
    prefs: &DesktopPrefs,
    chord: &Chord,
    skip: Option<&WindowSlot>,
    mac: bool,
) -> Result<(), BindError> {
    let c = normalize_chord(chord);
    if !chord_is_safe_global(&c) {
        return Err(BindError::UnsafeBareKey);
    }
    if is_protected_chord(&c, mac) {
        return Err(BindError::Protected);
    }
    for action in AppAction::all_global() {
        for ac in chords_for_action(prefs, *action) {
            if normalize_chord(&ac) == c {
                return Err(BindError::Conflict(*action));
            }
        }
    }
    for (slot, used) in window_hotkey_slots(prefs) {
        if skip != Some(&slot) && normalize_chord(&used) == c {
            return Err(BindError::ConflictWindowHotkey);
        }
    }
    Ok(())
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
    for wh in all_window_hotkey_chords(&effective) {
        for action in AppAction::all_global() {
            for chord in chords_for_action(&effective, *action) {
                if normalize_chord(&chord) == wh {
                    return Err(BindError::Conflict(*action));
                }
            }
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
    for chord in &normalized {
        validate_window_hotkey_candidate(prefs, chord, None, mac)?;
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
    for wh in all_window_hotkey_chords(prefs) {
        if defaults.iter().any(|d| normalize_chord(d) == wh) {
            return Err(BindError::ConflictWindowHotkey);
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

pub fn reset_all_shortcuts_in_prefs(
    prefs: &DesktopPrefs,
    mac: bool,
) -> Result<HashMap<String, Vec<Chord>>, BindError> {
    let validated = validate_effective_shortcuts(&HashMap::new(), mac)?;
    let mut trial = prefs.clone();
    trial.shortcuts = validated.clone();
    for wh in all_window_hotkey_chords(&trial) {
        for action in AppAction::all_global() {
            for chord in chords_for_action(&trial, *action) {
                if normalize_chord(&chord) == wh {
                    return Err(BindError::ConflictWindowHotkey);
                }
            }
        }
    }
    Ok(validated)
}

pub fn reset_all_shortcuts() -> Result<(), BindError> {
    let mac = cfg!(target_os = "macos");
    let mut prefs = desktop_prefs().write().unwrap();
    let validated = reset_all_shortcuts_in_prefs(&prefs, mac)?;
    prefs.shortcuts = validated;
    schedule_save();
    Ok(())
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
    sanitize_seances(&mut prefs, mac);
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
    colors::init_applied_palette(&prefs.terminal_color_scheme);
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

pub fn apply_color_scheme_from_settings(scheme: ColorScheme) -> Result<(), String> {
    let mut prefs = desktop_prefs().write().unwrap();
    prefs.terminal_color_scheme = scheme.clone();
    colors::apply_palette(scheme);
    schedule_save();
    Ok(())
}

pub fn reset_color_scheme_defaults() {
    let scheme = ColorScheme::default();
    let mut prefs = desktop_prefs().write().unwrap();
    prefs.terminal_color_scheme = scheme.clone();
    colors::apply_palette(scheme);
    schedule_save();
}

/// Apply a Seance edit to the live prefs, saving only when it changed
/// something. Window side effects (retitle, reproject, hotkeys) belong to
/// `WindowHotkeys`, which calls these.
pub fn edit_seances<T>(f: impl FnOnce(&mut DesktopPrefs) -> T, changed: impl Fn(&T) -> bool) -> T {
    let mut prefs = desktop_prefs().write().unwrap();
    let out = f(&mut prefs);
    if changed(&out) {
        schedule_save();
    }
    out
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
        assert!(back.main_window_hotkey.is_none());
        assert!(back.workspace_windows.is_empty());
    }

    #[test]
    fn window_hotkey_fields_roundtrip_in_json() {
        let mut prefs = DesktopPrefs::default();
        prefs.main_window_hotkey = Some(make_chord(false, true, false, "m"));
        prefs.workspace_windows.push(WorkspaceWindowDefinition {
            workspace: "nuance".into(),
            shortcut: Some(make_chord(false, true, true, "n")),
        });
        let json = serde_json::to_string(&prefs).unwrap();
        let back: DesktopPrefs = serde_json::from_str(&json).unwrap();
        assert_eq!(back.main_window_hotkey, prefs.main_window_hotkey);
        assert_eq!(back.workspace_windows, prefs.workspace_windows);
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
    fn reset_all_refuses_when_window_hotkey_uses_freed_default() {
        let mac = cfg!(target_os = "macos");
        let mut prefs = DesktopPrefs::default();
        let default_new = default_chords(AppAction::NewSession)[0].clone();
        prefs
            .shortcuts
            .insert(action_key(AppAction::NewSession), vec![shift_chord("x")]);
        prefs.main_window_hotkey = Some(default_new);
        assert!(matches!(
            reset_all_shortcuts_in_prefs(&prefs, mac),
            Err(BindError::ConflictWindowHotkey)
        ));
    }

    #[test]
    fn window_hotkey_conflicts_with_app_shortcut() {
        let mac = cfg!(target_os = "macos");
        let mut prefs = DesktopPrefs::default();
        let chord = default_chords(AppAction::NewSession)[0].clone();
        prefs.main_window_hotkey = Some(chord.clone());
        assert!(matches!(
            validate_window_hotkey_candidate(&prefs, &chord, None, mac),
            Err(BindError::Conflict(AppAction::NewSession))
        ));
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

    #[test]
    fn terminal_color_scheme_roundtrips_in_desktop_json() {
        let mut prefs = DesktopPrefs::default();
        prefs.terminal_color_scheme.background = colors::Rgb24::from_hex("#222222").unwrap();
        let json = serde_json::to_string(&prefs).unwrap();
        let installed: HashSet<String> = HashSet::new();
        let back = parse_prefs_json(&json, &installed, None);
        assert_eq!(back.terminal_color_scheme.background.to_hex(), "#222222");
    }

    /// ctrl+alt+cmd+key: no app shortcut uses it, so window-hotkey tests
    /// exercise only window-vs-window rules.
    fn win_chord(key: &str) -> Chord {
        Chord {
            ctrl: true,
            alt: true,
            shift: false,
            meta: true,
            key: normalize_key(key),
        }
    }

    fn legacy_json() -> String {
        let mut prefs = DesktopPrefs::default();
        prefs.font_size = 17.0;
        prefs.main_window_hotkey = Some(make_chord(false, true, true, "m"));
        prefs.workspace_windows = vec![
            WorkspaceWindowDefinition {
                workspace: "nuance".into(),
                shortcut: Some(win_chord("1")),
            },
            WorkspaceWindowDefinition {
                workspace: "home".into(),
                shortcut: None,
            },
        ];
        let json = serde_json::to_string(&prefs).unwrap();
        assert!(!json.contains("\"seances\""));
        json
    }

    #[test]
    fn legacy_workspace_windows_migrate_to_single_circle_seances() {
        let installed: HashSet<String> = HashSet::new();
        let p = parse_prefs_json(&legacy_json(), &installed, None);
        let defs = p.seance_defs();
        assert_eq!(defs.len(), 2);
        assert_eq!(defs[0].id, "ws-nuance");
        assert_eq!(defs[0].name, "nuance");
        assert_eq!(defs[0].members, vec!["nuance".to_string()]);
        assert_eq!(defs[0].shortcut, Some(win_chord("1")));
        assert!(!defs[0].overlay);
        assert_eq!(defs[1].shortcut, None);
        // Everything else rides along untouched, legacy list included.
        assert_eq!(p.font_size, 17.0);
        assert_eq!(
            p.main_window_hotkey,
            Some(make_chord(false, true, true, "m"))
        );
        assert_eq!(p.workspace_windows.len(), 2);
    }

    #[test]
    fn migration_is_idempotent_and_survives_save_reload() {
        let installed: HashSet<String> = HashSet::new();
        let once = parse_prefs_json(&legacy_json(), &installed, None);
        let twice = parse_prefs_json(&legacy_json(), &installed, None);
        assert_eq!(once.seances, twice.seances);
        let saved = serde_json::to_string(&once).unwrap();
        let reloaded = parse_prefs_json(&saved, &installed, None);
        assert_eq!(reloaded.seances, once.seances);
        assert_eq!(reloaded.workspace_windows, once.workspace_windows);
    }

    #[test]
    fn empty_seance_list_is_authoritative_and_never_remigrates() {
        let installed: HashSet<String> = HashSet::new();
        let mut p = parse_prefs_json(&legacy_json(), &installed, None);
        for id in ["ws-nuance", "ws-home"] {
            remove_seance_in(&mut p, id).unwrap();
        }
        let saved = serde_json::to_string(&p).unwrap();
        let reloaded = parse_prefs_json(&saved, &installed, None);
        assert_eq!(reloaded.seances, Some(Vec::new()));
        assert!(window_hotkey_slots(&reloaded)
            .iter()
            .all(|(slot, _)| *slot == WindowSlot::Main));
    }

    #[test]
    fn sanitize_keeps_first_claim_and_drops_only_the_colliding_hotkey() {
        let installed: HashSet<String> = HashSet::new();
        let m = make_chord(false, true, true, "m");
        let mut prefs = DesktopPrefs::default();
        prefs.main_window_hotkey = Some(m.clone());
        prefs.seances = Some(vec![
            SeanceDef {
                id: "a".into(),
                name: "  ".into(),
                members: vec!["x".into(), "y".into()],
                shortcut: Some(make_chord(false, true, true, "a")),
                overlay: true,
            },
            SeanceDef {
                id: "b".into(),
                name: "Beta".into(),
                members: vec!["y".into(), "z".into()],
                shortcut: Some(m),
                overlay: false,
            },
            SeanceDef {
                id: "a".into(),
                name: "dup".into(),
                members: vec!["w".into()],
                shortcut: None,
                overlay: false,
            },
        ]);
        let json = serde_json::to_string(&prefs).unwrap();
        let p = parse_prefs_json(&json, &installed, None);
        let defs = p.seance_defs();
        assert_eq!(defs.len(), 2);
        assert_eq!(defs[0].name, "a");
        assert_eq!(defs[0].members, vec!["x".to_string(), "y".to_string()]);
        assert!(defs[0].shortcut.is_some());
        assert!(defs[0].overlay);
        assert_eq!(defs[1].members, vec!["z".to_string()]);
        assert_eq!(defs[1].shortcut, None);
        assert!(p.main_window_hotkey.is_some());
    }

    #[test]
    fn assign_moves_exclusively_and_none_returns_to_main() {
        let mut p = DesktopPrefs::default();
        let a = create_seance_in(&mut p, "Client work", 5).unwrap();
        let b = create_seance_in(&mut p, "JoyRudder", 5).unwrap();
        assert_ne!(a, b);
        assert!(assign_circle_in(&mut p, "nuance", Some(&a)));
        assert!(!assign_circle_in(&mut p, "nuance", Some(&a)));
        assert!(assign_circle_in(&mut p, "nuance", Some(&b)));
        assert!(p.seance(&a).unwrap().members.is_empty());
        assert_eq!(p.owner_of("nuance").map(|d| d.id.clone()), Some(b.clone()));
        assert!(!assign_circle_in(&mut p, "nuance", Some("gone")));
        assert_eq!(p.owner_of("nuance").map(|d| d.id.clone()), Some(b.clone()));
        assert!(assign_circle_in(&mut p, "nuance", None));
        assert!(p.owner_of("nuance").is_none());
        assert!(!assign_circle_in(&mut p, "nuance", None));
    }

    #[test]
    fn removing_a_seance_returns_its_circles_to_main() {
        let mut p = DesktopPrefs::default();
        let a = create_seance_in(&mut p, "Client work", 5).unwrap();
        assign_circle_in(&mut p, "nuance", Some(&a));
        assert!(!projection_for(&p, None).admits("nuance"));
        let removed = remove_seance_in(&mut p, &a).unwrap();
        assert_eq!(removed.members, vec!["nuance".to_string()]);
        assert!(projection_for(&p, None).admits("nuance"));
        assert_eq!(
            projection_for(&p, Some(&a)),
            Projection::Only(BTreeSet::new())
        );
        assert!(remove_seance_in(&mut p, &a).is_none());
    }

    #[test]
    fn projection_main_is_all_without_seances_and_except_claimed_with_them() {
        let mut p = DesktopPrefs::default();
        assert_eq!(projection_for(&p, None), Projection::All);
        let a = create_seance_in(&mut p, "Client work", 5).unwrap();
        // An empty Seance claims nothing: main still shows everything.
        assert_eq!(projection_for(&p, None), Projection::All);
        assign_circle_in(&mut p, "nuance", Some(&a));
        assert!(!projection_for(&p, None).admits("nuance"));
        assert!(projection_for(&p, None).admits("home"));
        assert!(projection_for(&p, Some(&a)).admits("nuance"));
        assert!(!projection_for(&p, Some(&a)).admits("home"));
        assert!(!projection_for(&p, Some("missing")).admits("nuance"));
    }

    #[test]
    fn names_are_required_unique_and_rename_keeps_identity() {
        let mut p = DesktopPrefs::default();
        assert!(create_seance_in(&mut p, "   ", 1).is_err());
        let a = create_seance_in(&mut p, " Personal ", 1).unwrap();
        assert_eq!(p.seance(&a).unwrap().name, "Personal");
        assert!(create_seance_in(&mut p, "personal", 1).is_err());
        let b = create_seance_in(&mut p, "Client", 1).unwrap();
        assert!(rename_seance_in(&mut p, &b, "PERSONAL").is_err());
        rename_seance_in(&mut p, &a, "personal").unwrap();
        rename_seance_in(&mut p, &a, "Home stuff").unwrap();
        assert_eq!(p.seance(&a).unwrap().id, a);
        assert_eq!(p.slot_label(&WindowSlot::Seance(a)), "Home stuff");
    }

    #[test]
    fn window_hotkey_validation_skips_only_the_rebound_slot() {
        let mac = cfg!(target_os = "macos");
        let mut p = DesktopPrefs::default();
        let a = create_seance_in(&mut p, "A", 1).unwrap();
        let chord = win_chord("2");
        p.seances.as_mut().unwrap()[0].shortcut = Some(chord.clone());
        let slot = WindowSlot::Seance(a);
        assert!(validate_window_hotkey_candidate(&p, &chord, Some(&slot), mac).is_ok());
        assert_eq!(
            validate_window_hotkey_candidate(&p, &chord, Some(&WindowSlot::Main), mac),
            Err(BindError::ConflictWindowHotkey)
        );
    }

    #[test]
    fn overlay_defaults_off_and_toggles_per_slot() {
        let mut p = DesktopPrefs::default();
        let a = create_seance_in(&mut p, "A", 1).unwrap();
        let slot = WindowSlot::Seance(a);
        assert!(!p.overlay_for(&WindowSlot::Main));
        assert!(!p.overlay_for(&slot));
        assert!(set_overlay_in(&mut p, &slot, true));
        assert!(!set_overlay_in(&mut p, &slot, true));
        assert!(p.overlay_for(&slot));
        assert!(!p.overlay_for(&WindowSlot::Main));
        assert!(!set_overlay_in(
            &mut p,
            &WindowSlot::Seance("gone".into()),
            true
        ));
    }
}
