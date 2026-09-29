//! System-wide window show/hide hotkeys (macOS Carbon via `global-hotkey`).
//!
//! Process-wide coordinator (`gpui::Global`): native registration, the live
//! main + named-Seance windows, the circle catalog for Settings, and
//! UI-thread visibility toggles. Every target is a [`WindowSlot`] — an
//! immutable Seance id, never a row index or a label.

use std::collections::HashMap;

use gpui::{AnyWindowHandle, App, AppContext, Global, SharedString, WeakEntity, Window};
use gpui_component::Root;

use super::preferences::{
    self, chord_is_safe_global, desktop_prefs, normalize_chord, schedule_desktop_save, Chord,
    DesktopPrefs, Projection, WindowSlot,
};
use super::window_overlay;
use super::SeanceApp;

/// Which Seance surface an OS window represents.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WindowScope {
    Main,
    Blank,
    Seance(String),
}

impl WindowScope {
    pub fn is_blank(&self) -> bool {
        matches!(self, WindowScope::Blank)
    }

    pub fn seance_id(&self) -> Option<&str> {
        match self {
            WindowScope::Seance(id) => Some(id.as_str()),
            _ => None,
        }
    }

    pub fn slot(&self) -> Option<WindowSlot> {
        match self {
            WindowScope::Main => Some(WindowSlot::Main),
            WindowScope::Seance(id) => Some(WindowSlot::Seance(id.clone())),
            WindowScope::Blank => None,
        }
    }

    /// Blank windows keep their old behavior (whatever they subscribe to).
    pub fn projection(&self, prefs: &DesktopPrefs) -> Projection {
        match self {
            WindowScope::Blank => Projection::All,
            WindowScope::Main => preferences::projection_for(prefs, None),
            WindowScope::Seance(id) => preferences::projection_for(prefs, Some(id)),
        }
    }
}

/// Hotkey dispatch target — the same identity as a hotkey slot.
pub type WindowTarget = WindowSlot;

#[derive(Clone, Debug)]
pub enum WindowHotkeyError {
    UnsupportedChord,
    ConflictApp(preferences::AppAction),
    ConflictWindow(String),
    ConflictNative,
    RegisterFailed(String),
}

impl WindowHotkeyError {
    pub fn message(&self) -> String {
        match self {
            Self::UnsupportedChord => "That key shape is not supported for a global hotkey.".into(),
            Self::ConflictApp(a) => format!("Already used by “{}”.", a.label()),
            Self::ConflictWindow(label) => {
                format!("Already used by window hotkey for “{}”.", label)
            }
            Self::ConflictNative => {
                "Conflicts with another global hotkey at the OS level (same physical key).".into()
            }
            Self::RegisterFailed(e) => format!("Could not register hotkey: {e}"),
        }
    }
}

/// Active shortcut capture in Settings — global presses route here instead of toggling.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SettingsCapture {
    Window(WindowSlot),
    AppShortcut(preferences::AppAction),
}

struct SettingsRecorder {
    entity: WeakEntity<super::settings::SettingsWindow>,
    capture: SettingsCapture,
    /// Preserved for API compat; routing uses the pressed hotkey id map.
    registered_chord: Option<Chord>,
}

/// One live main/Seance OS window.
struct LiveWindow {
    handle: AnyWindowHandle,
    app: WeakEntity<SeanceApp>,
    /// Level + collection behavior the window had before Overlay touched it,
    /// restored verbatim when Overlay goes off.
    native_normal: Option<window_overlay::NormalBehavior>,
    /// App that was frontmost when the overlay was shown (pid, not a
    /// pointer). Cleared on hide, close, removal and app deactivation.
    prior_app: Option<i32>,
}

impl LiveWindow {
    fn new(handle: AnyWindowHandle, app: WeakEntity<SeanceApp>) -> Self {
        Self {
            handle,
            app,
            native_normal: None,
            prior_app: None,
        }
    }
}

pub struct WindowHotkeys {
    main: Option<LiveWindow>,
    seances: HashMap<String, LiveWindow>,
    catalog: Vec<(String, String)>,
    last_error: Option<String>,
    settings_recorder: Option<SettingsRecorder>,
    /// Settings window + its native behavior before it was lifted to the
    /// overlay layer (reset whenever a new Settings window appears).
    settings_layer: Option<(AnyWindowHandle, Option<window_overlay::NormalBehavior>)>,
    native: native::NativeHotkeys,
}

impl Global for WindowHotkeys {}

impl WindowHotkeys {
    pub fn init(cx: &mut App) {
        if cx.has_global::<Self>() {
            return;
        }
        let mut hotkeys = Self {
            main: None,
            seances: HashMap::new(),
            catalog: Vec::new(),
            last_error: None,
            settings_recorder: None,
            settings_layer: None,
            native: native::NativeHotkeys::new(cx),
        };
        hotkeys.sync_native();
        cx.set_global(hotkeys);
        window_overlay::install_resign_observer(cx);
    }

    pub fn global(cx: &App) -> &Self {
        cx.global::<Self>()
    }

    pub fn global_mut(cx: &mut App) -> &mut Self {
        cx.global_mut::<Self>()
    }

    pub fn platform_supported() -> bool {
        cfg!(target_os = "macos") && !Self::global_disabled_message().is_some()
    }

    /// Overlay needs only AppKit, not the hotkey manager.
    pub fn overlay_supported() -> bool {
        cfg!(target_os = "macos")
    }

    fn global_disabled_message() -> Option<String> {
        #[cfg(target_os = "macos")]
        {
            return native::startup_error().map(str::to_string);
        }
        #[cfg(not(target_os = "macos"))]
        {
            None
        }
    }

    pub fn last_error(cx: &App) -> Option<String> {
        Self::global(cx)
            .last_error
            .clone()
            .or_else(|| Self::global_disabled_message())
    }

    pub fn set_settings_recorder(
        cx: &mut App,
        entity: WeakEntity<super::settings::SettingsWindow>,
        capture: Option<SettingsCapture>,
        registered_chord: Option<Chord>,
    ) {
        let g = Self::global_mut(cx);
        g.settings_recorder = capture.map(|c| SettingsRecorder {
            entity,
            capture: c,
            registered_chord,
        });
    }

    /// Full daemon catalog `(slug, label)` — published before any projection.
    pub fn publish_catalog(cx: &mut App, slugs_labels: impl IntoIterator<Item = (String, String)>) {
        let mut v: Vec<_> = slugs_labels.into_iter().collect();
        v.sort_by(|a, b| a.1.to_lowercase().cmp(&b.1.to_lowercase()));
        Self::global_mut(cx).catalog = v;
    }

    pub fn catalog(cx: &App) -> Vec<(String, String)> {
        Self::global(cx).catalog.clone()
    }

    fn live_mut(&mut self, slot: &WindowSlot) -> Option<&mut LiveWindow> {
        match slot {
            WindowSlot::Main => self.main.as_mut(),
            WindowSlot::Seance(id) => self.seances.get_mut(id),
        }
    }

    fn live_handle(cx: &App, slot: &WindowSlot) -> Option<AnyWindowHandle> {
        let g = Self::global(cx);
        match slot {
            WindowSlot::Main => g.main.as_ref().map(|w| w.handle),
            WindowSlot::Seance(id) => g.seances.get(id).map(|w| w.handle),
        }
    }

    /// A main/Seance window came up (called from `SeanceApp::new_inner`).
    pub fn register_window(
        cx: &mut App,
        slot: WindowSlot,
        handle: AnyWindowHandle,
        app: WeakEntity<SeanceApp>,
    ) {
        let live = LiveWindow::new(handle, app);
        match slot.clone() {
            WindowSlot::Main => Self::global_mut(cx).main = Some(live),
            WindowSlot::Seance(id) => {
                Self::global_mut(cx).seances.insert(id, live);
            }
        }
        // The window isn't on screen yet inside its own constructor.
        cx.defer(move |cx| Self::apply_overlay(cx, &slot));
    }

    /// The window is going away (WM close, kick, Seance removal). Only drops
    /// the registry entry if it still points at this handle.
    pub fn unregister_window(cx: &mut App, slot: &WindowSlot, handle: AnyWindowHandle) {
        let g = Self::global_mut(cx);
        match slot {
            WindowSlot::Main => {
                if g.main.as_ref().is_some_and(|w| w.handle == handle) {
                    g.main = None;
                }
            }
            WindowSlot::Seance(id) => {
                if g.seances.get(id).is_some_and(|w| w.handle == handle) {
                    g.seances.remove(id);
                }
            }
        }
    }

    /// Reconcile native registrations after desktop prefs change (not on window open).
    #[allow(dead_code)]
    pub fn sync_from_prefs(cx: &mut App) {
        Self::global_mut(cx).sync_native();
    }

    fn sync_native(&mut self) {
        if let Some(msg) = Self::global_disabled_message() {
            self.last_error = Some(msg);
            return;
        }
        let prefs = desktop_prefs().read().unwrap().clone();
        if let Err(e) = self.native.apply_prefs(&prefs) {
            self.last_error = Some(e.message());
        } else {
            self.last_error = None;
        }
    }

    /// Bind or clear (`None`) the show/hide hotkey of one window slot. Native
    /// registration is proven before prefs change.
    pub fn bind_slot(
        cx: &mut App,
        slot: &WindowSlot,
        chord: Option<Chord>,
    ) -> Result<(), WindowHotkeyError> {
        let mac = cfg!(target_os = "macos");
        let prefs = desktop_prefs().read().unwrap().clone();
        if let WindowSlot::Seance(id) = slot {
            if prefs.seance(id).is_none() {
                return Err(WindowHotkeyError::RegisterFailed(
                    "That Seance no longer exists.".into(),
                ));
            }
        }
        if let Some(c) = chord.as_ref() {
            preferences::validate_window_hotkey_candidate(&prefs, c, Some(slot), mac)
                .map_err(map_bind_err)?;
            native_collision_against_prefs(&prefs, c, Some(slot))?;
        }
        {
            let g = Self::global_mut(cx);
            g.native
                .transactional(&prefs, slot, chord.clone())
                .map_err(|e| {
                    g.last_error = Some(e.message());
                    e
                })?;
        }
        {
            let mut prefs = desktop_prefs().write().unwrap();
            *prefs = with_slot_chord(&prefs, slot, chord);
            schedule_desktop_save();
        }
        Self::global_mut(cx).last_error = None;
        Ok(())
    }

    pub fn create_seance(cx: &mut App, name: &str) -> Result<String, String> {
        let now = super::util::now_ms();
        let id = preferences::edit_seances(
            |p| preferences::create_seance_in(p, name, now),
            |r| r.is_ok(),
        )?;
        Self::global_mut(cx).last_error = None;
        Ok(id)
    }

    pub fn rename_seance(cx: &mut App, id: &str, name: &str) -> Result<(), String> {
        preferences::edit_seances(
            |p| preferences::rename_seance_in(p, id, name),
            |r| r.is_ok(),
        )?;
        let title = window_title(&WindowSlot::Seance(id.to_string()));
        if let Some(handle) = Self::live_handle(cx, &WindowSlot::Seance(id.to_string())) {
            let _ = handle.update(cx, |_, window, _| window.set_window_title(&title));
        }
        // The sidebar heading reads the name at render time; projection is
        // unchanged, so nothing else would repaint it.
        if let Some(app) = Self::global(cx).seances.get(id).map(|w| w.app.clone()) {
            let _ = app.update(cx, |_, cx| cx.notify());
        }
        Ok(())
    }

    /// Remove a named Seance: release its hotkey, close its window (GUI
    /// only — every session keeps running) and hand its circles back to the
    /// main window.
    pub fn remove_seance(cx: &mut App, id: &str) -> Result<(), WindowHotkeyError> {
        let prefs = desktop_prefs().read().unwrap().clone();
        let mut candidate = prefs.clone();
        if preferences::remove_seance_in(&mut candidate, id).is_none() {
            return Ok(());
        }
        if prefs.seance(id).is_some_and(|d| d.shortcut.is_some()) {
            let g = Self::global_mut(cx);
            g.native.apply_prefs(&candidate).map_err(|e| {
                g.last_error = Some(e.message());
                e
            })?;
        }
        preferences::edit_seances(|p| preferences::remove_seance_in(p, id), Option::is_some);
        let live = Self::global_mut(cx).seances.remove(id);
        if let Some(live) = live {
            if let Some(app) = live.app.upgrade() {
                app.read(cx).client.disconnect();
            }
            let handle = live.handle;
            cx.defer(move |cx| {
                let _ = handle.update(cx, |_, window, _| window.remove_window());
            });
        }
        Self::global_mut(cx).last_error = None;
        Self::scopes_changed(cx);
        cx.defer(|cx| Self::sync_settings_layer(cx, true));
        Ok(())
    }

    /// Move a circle into a Seance (`None` = back to the main window) and
    /// reproject every live window.
    pub fn assign_circle(cx: &mut App, workspace: &str, target: Option<&str>) {
        let changed = preferences::edit_seances(
            |p| preferences::assign_circle_in(p, workspace, target),
            |c| *c,
        );
        if changed {
            Self::scopes_changed(cx);
        }
    }

    /// A circle is gone for good (banished): drop it from any Seance so a
    /// future circle reusing the slug starts unassigned.
    pub fn forget_circle(cx: &mut App, workspace: &str) {
        Self::assign_circle(cx, workspace, None);
    }

    pub fn set_overlay(cx: &mut App, slot: &WindowSlot, on: bool) {
        let changed =
            preferences::edit_seances(|p| preferences::set_overlay_in(p, slot, on), |c| *c);
        if changed {
            Self::apply_overlay(cx, slot);
            // Toggled from Settings: keep Settings usable above the overlay.
            cx.defer(|cx| Self::sync_settings_layer(cx, true));
        }
    }

    /// Whether any window has Overlay on — Settings then joins their layer.
    pub fn any_overlay() -> bool {
        let prefs = desktop_prefs().read().unwrap();
        prefs.main_window_overlay || prefs.seance_defs().iter().any(|d| d.overlay)
    }

    /// Lift Settings to the overlay layer while any Overlay is enabled (and
    /// restore it when none is). `raise` orders it above the overlays —
    /// only for user-initiated Settings actions.
    pub fn sync_settings_layer(cx: &mut App, raise: bool) {
        let Some(handle) = super::settings::settings_window_handle() else {
            Self::global_mut(cx).settings_layer = None;
            return;
        };
        let on = Self::any_overlay();
        let _ = handle.update(cx, |_, window, cx| {
            let g = Self::global_mut(cx);
            if g.settings_layer.as_ref().map(|(h, _)| *h) != Some(handle) {
                g.settings_layer = Some((handle, None));
            }
            if let Some((_, normal)) = g.settings_layer.as_mut() {
                window_overlay::configure_companion(window, on, normal);
            }
            if raise && on {
                window_overlay::raise_companion(window, cx);
            }
        });
    }

    /// Push the slot's Overlay pref onto its live native window.
    fn apply_overlay(cx: &mut App, slot: &WindowSlot) {
        let on = desktop_prefs().read().unwrap().overlay_for(slot);
        let Some(handle) = Self::live_handle(cx, slot) else {
            return;
        };
        let slot = slot.clone();
        let _ = handle.update(cx, move |_, window, cx| {
            let Some(live) = Self::global_mut(cx).live_mut(&slot) else {
                return;
            };
            window_overlay::configure(window, on, &mut live.native_normal);
            if !on {
                live.prior_app = None;
            }
        });
    }

    /// Membership/rename/delete changed: every live window re-derives its
    /// projection. Deferred so callers inside a window's own update (sidebar
    /// menu, new circle) don't re-enter it.
    pub fn scopes_changed(cx: &mut App) {
        cx.defer(|cx| {
            let g = Self::global(cx);
            let targets: Vec<(AnyWindowHandle, WeakEntity<SeanceApp>)> = g
                .main
                .iter()
                .chain(g.seances.values())
                .map(|w| (w.handle, w.app.clone()))
                .collect();
            for (handle, app) in targets {
                let _ = handle.update(cx, |_, window, cx| {
                    let _ = app.update(cx, |app, cx| app.apply_scope_change(window, cx));
                });
            }
        });
    }

    pub fn toggle_target(cx: &mut App, target: WindowTarget) {
        if let WindowSlot::Seance(id) = &target {
            if desktop_prefs().read().unwrap().seance(id).is_none() {
                return;
            }
        }
        if let Some(handle) = Self::live_handle(cx, &target) {
            let slot = target.clone();
            if handle
                .update(cx, move |_, window, cx| {
                    toggle_visibility(window, &slot, cx)
                })
                .is_ok()
            {
                return;
            }
            Self::unregister_window(cx, &target, handle);
        }
        match target {
            WindowSlot::Main => open_main_window(cx),
            WindowSlot::Seance(id) => open_seance_window(cx, id),
        }
    }

    pub fn open_or_toggle_main(cx: &mut App) {
        Self::toggle_target(cx, WindowSlot::Main);
    }

    /// Settings "Open": show the window through the same path as the
    /// hotkey (never hides it).
    pub fn open_seance(cx: &mut App, id: &str) {
        let slot = WindowSlot::Seance(id.to_string());
        if let Some(handle) = Self::live_handle(cx, &slot) {
            let prior = window_overlay::frontmost_other_app();
            let s = slot.clone();
            if handle
                .update(cx, move |_, window, cx| show_slot(window, &s, prior, cx))
                .is_ok()
            {
                return;
            }
            Self::unregister_window(cx, &slot, handle);
        }
        open_seance_window(cx, id.to_string());
    }

    /// App lost activation: tuck away every visible overlay window without
    /// touching whichever app the user just went to.
    /// Stale notifications (a show since, or active again) are dropped by
    /// the observer before they get here.
    pub fn on_app_resigned(cx: &mut App) {
        let prefs = desktop_prefs().read().unwrap().clone();
        let g = Self::global_mut(cx);
        let mut targets = Vec::new();
        if let Some(main) = g.main.as_mut() {
            if prefs.main_window_overlay {
                main.prior_app = None;
                targets.push(main.handle);
            }
        }
        for (id, live) in g.seances.iter_mut() {
            if prefs.seance(id).is_some_and(|d| d.overlay) {
                live.prior_app = None;
                targets.push(live.handle);
            }
        }
        for handle in targets {
            let _ = handle.update(cx, |_, window, _| window_overlay::hide_quietly(window));
        }
    }

    /// One GUI in this process owns desktop notifications and the telegram
    /// status bridge — main when it is live, otherwise the live Seance window
    /// with the lowest id.
    pub fn is_notification_owner(cx: &mut App, own_window: AnyWindowHandle) -> bool {
        fn live(cx: &mut App, handle: AnyWindowHandle) -> bool {
            handle.update(cx, |_, _, _| ()).is_ok()
        }
        let main_handle = Self::global(cx).main.as_ref().map(|w| w.handle);
        if main_handle.is_some_and(|h| live(cx, h)) {
            return main_handle == Some(own_window);
        }
        let mut seances: Vec<(String, AnyWindowHandle)> = Self::global(cx)
            .seances
            .iter()
            .map(|(id, w)| (id.clone(), w.handle))
            .collect();
        seances.sort_by(|a, b| a.0.cmp(&b.0));
        seances
            .into_iter()
            .find(|(_, h)| live(cx, *h))
            .is_some_and(|(_, h)| h == own_window)
    }

    pub fn on_global_hotkey(cx: &mut App, id: u32) {
        let recorder = Self::global(cx).settings_recorder.as_ref().map(|r| {
            (
                r.entity.clone(),
                r.capture.clone(),
                r.registered_chord.clone(),
            )
        });
        if let Some((entity, capture, _registered_chord)) = recorder {
            if let Some(entity) = entity.upgrade() {
                if let Some(chord) = Self::global(cx).native.chord_for_id(id) {
                    entity.update(cx, |settings, cx| {
                        settings.complete_hotkey_capture(capture, Some(chord), cx);
                    });
                }
                return;
            }
            Self::global_mut(cx).settings_recorder = None;
        }
        if let Some(target) = Self::global(cx).native.target_for_id(id) {
            Self::toggle_target(cx, target);
        }
    }
}

/// `prefs` with one slot's chord replaced (normalized).
fn with_slot_chord(prefs: &DesktopPrefs, slot: &WindowSlot, chord: Option<Chord>) -> DesktopPrefs {
    let mut out = prefs.clone();
    let chord = chord.map(|c| normalize_chord(&c));
    match slot {
        WindowSlot::Main => out.main_window_hotkey = chord,
        WindowSlot::Seance(id) => {
            if let Some(def) = out
                .seances
                .as_mut()
                .and_then(|l| l.iter_mut().find(|d| &d.id == id))
            {
                def.shortcut = chord;
            }
        }
    }
    out
}

fn map_bind_err(e: preferences::BindError) -> WindowHotkeyError {
    match e {
        preferences::BindError::Conflict(a) => WindowHotkeyError::ConflictApp(a),
        preferences::BindError::Protected | preferences::BindError::UnsafeBareKey => {
            WindowHotkeyError::UnsupportedChord
        }
        preferences::BindError::ConflictWindowHotkey => {
            WindowHotkeyError::ConflictWindow("another window hotkey".into())
        }
    }
}

fn window_title(slot: &WindowSlot) -> String {
    match slot {
        WindowSlot::Main => "seance".into(),
        WindowSlot::Seance(_) => format!(
            "seance — {}",
            desktop_prefs().read().unwrap().slot_label(slot)
        ),
    }
}

/// Whether `slot` opens through the hidden-then-overlay-show path. macOS
/// only: elsewhere a saved Overlay has no effect, so windows open normally.
fn opens_as_overlay(slot: &WindowSlot) -> bool {
    overlay_supported(desktop_prefs().read().unwrap().overlay_for(slot))
}

fn overlay_supported(pref: bool) -> bool {
    cfg!(target_os = "macos") && pref
}

/// Settings opens hidden and is shown by the companion raise (above the
/// overlays, on the current Space) whenever an Overlay is live here.
pub fn settings_opens_as_companion() -> bool {
    overlay_supported(WindowHotkeys::any_overlay())
}

/// Boot must not activate Seance ahead of an Overlay Main's own show (which
/// orders the window onto the current Space first, then activates).
pub fn main_boot_activates() -> bool {
    !opens_as_overlay(&WindowSlot::Main)
}

fn open_scoped_window(
    cx: &mut App,
    slot: WindowSlot,
    size: gpui::Size<gpui::Pixels>,
    build: fn(&mut Window, &mut gpui::Context<SeanceApp>, &WindowSlot) -> SeanceApp,
) {
    // Overlay: remember the app in front BEFORE a window of ours exists,
    // create it hidden and unfocused, then show it through the same native
    // path as the hotkey once Overlay flags are on (pointer screen, current
    // Space, key before activation).
    let overlay = opens_as_overlay(&slot);
    let prior = if overlay {
        window_overlay::frontmost_other_app()
    } else {
        None
    };
    let display_id = if overlay {
        window_overlay::pointer_display_id(cx)
    } else {
        None
    };
    let bounds = gpui::Bounds::centered(display_id, size, cx);
    let shown = slot.clone();
    let opened = cx.open_window(
        gpui::WindowOptions {
            window_bounds: Some(gpui::WindowBounds::Windowed(bounds)),
            titlebar: Some(gpui::TitlebarOptions {
                title: Some(SharedString::from(window_title(&slot))),
                ..Default::default()
            }),
            app_id: Some("seance".into()),
            focus: !overlay,
            show: !overlay,
            display_id,
            ..Default::default()
        },
        move |window, cx| {
            let view = cx.new(|cx| build(window, cx, &slot));
            let client = view.read(cx).client.clone();
            let handle = window.window_handle();
            let closing = slot.clone();
            window.on_window_should_close(cx, move |_, cx| {
                WindowHotkeys::unregister_window(cx, &closing, handle);
                client.disconnect();
                true
            });
            cx.new(|cx| Root::new(view, window, cx))
        },
    );
    if let (true, Ok(handle)) = (overlay, opened) {
        let handle: AnyWindowHandle = handle.into();
        // Queued after `register_window`'s own deferred configure.
        cx.defer(move |cx| {
            WindowHotkeys::apply_overlay(cx, &shown);
            let _ = handle.update(cx, |_, window, cx| show_slot(window, &shown, prior, cx));
        });
    }
}

pub fn open_main_window(cx: &mut App) {
    open_scoped_window(
        cx,
        WindowSlot::Main,
        gpui::size(gpui::px(1480.), gpui::px(920.)),
        |window, cx, _| SeanceApp::new(window, cx),
    );
}

pub fn open_seance_window(cx: &mut App, id: String) {
    if desktop_prefs().read().unwrap().seance(&id).is_none() {
        return;
    }
    open_scoped_window(
        cx,
        WindowSlot::Seance(id),
        gpui::size(gpui::px(1280.), gpui::px(800.)),
        |window, cx, slot| match slot {
            WindowSlot::Seance(id) => SeanceApp::new_seance_window(window, cx, id),
            WindowSlot::Main => SeanceApp::new(window, cx),
        },
    );
}

fn toggle_visibility(window: &mut Window, slot: &WindowSlot, cx: &mut App) {
    let overlay = desktop_prefs().read().unwrap().overlay_for(slot);
    match window_overlay::toggle_action(window, overlay) {
        window_overlay::ToggleAction::Hide => {
            let prior = WindowHotkeys::global_mut(cx)
                .live_mut(slot)
                .and_then(|w| w.prior_app.take());
            window_overlay::hide(window, if overlay { prior } else { None });
        }
        window_overlay::ToggleAction::Show => {
            // Before any native call that could activate us.
            let prior = window_overlay::frontmost_other_app();
            show_slot(window, slot, prior, cx);
        }
        window_overlay::ToggleAction::GpuiFallback => window.activate_window(),
    }
}

/// Every show — hotkey, Settings Open, a freshly created window — lands
/// here. `prior` must have been captured before anything touched focus.
fn show_slot(window: &mut Window, slot: &WindowSlot, prior: Option<i32>, cx: &mut App) {
    let overlay = desktop_prefs().read().unwrap().overlay_for(slot);
    window_overlay::show(window, overlay, cx);
    if let Some(live) = WindowHotkeys::global_mut(cx).live_mut(slot) {
        live.prior_app = if overlay { prior } else { None };
    }
}

#[cfg(target_os = "macos")]
mod native {
    use super::*;
    use global_hotkey::hotkey::{Code, HotKey, Modifiers};
    use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};
    use gpui::App;
    use std::collections::{HashMap, HashSet};
    use std::sync::OnceLock;

    static STARTUP_ERROR: OnceLock<Option<String>> = OnceLock::new();

    pub fn startup_error() -> Option<&'static str> {
        STARTUP_ERROR.get().and_then(|o| o.as_deref())
    }

    trait Registrar {
        fn register(&self, hotkey: HotKey) -> Result<(), String>;
        fn unregister(&self, hotkey: HotKey) -> Result<(), String>;
    }

    impl Registrar for GlobalHotKeyManager {
        fn register(&self, hotkey: HotKey) -> Result<(), String> {
            GlobalHotKeyManager::register(self, hotkey).map_err(|error| error.to_string())
        }

        fn unregister(&self, hotkey: HotKey) -> Result<(), String> {
            GlobalHotKeyManager::unregister(self, hotkey).map_err(|error| error.to_string())
        }
    }

    pub struct NativeHotkeys {
        manager: Option<Box<dyn Registrar>>,
        id_to_target: HashMap<u32, WindowTarget>,
        id_to_chord: HashMap<u32, Chord>,
        registered: HashMap<u32, HotKey>,
        held_ids: HashSet<u32>,
        needs_reconcile: bool,
    }

    impl NativeHotkeys {
        pub fn new(cx: &mut App) -> Self {
            let startup = GlobalHotKeyManager::new().map_err(|e| e.to_string());
            STARTUP_ERROR.get_or_init(|| startup.as_ref().err().cloned());
            let manager: Option<Box<dyn Registrar>> = match startup {
                Ok(m) => Some(Box::new(m)),
                Err(e) => {
                    eprintln!("[seance] global hotkeys disabled: {e}");
                    None
                }
            };
            if manager.is_some() {
                let (tx, mut rx) = futures::channel::mpsc::unbounded();
                GlobalHotKeyEvent::set_event_handler(Some(move |ev| {
                    let _ = tx.unbounded_send(ev);
                }));
                cx.spawn(async move |cx| {
                    use futures::StreamExt;
                    while let Some(ev) = rx.next().await {
                        let released = ev.state == HotKeyState::Released;
                        let id = ev.id;
                        cx.update(|app| {
                            let g = WindowHotkeys::global_mut(app);
                            if g.native.manager.is_none() {
                                return;
                            }
                            if released {
                                g.native.held_ids.remove(&id);
                                return;
                            }
                            if g.native.held_ids.contains(&id) {
                                return;
                            }
                            g.native.held_ids.insert(id);
                            WindowHotkeys::on_global_hotkey(app, id);
                        });
                    }
                })
                .detach();
            }
            Self {
                manager,
                id_to_target: HashMap::new(),
                id_to_chord: HashMap::new(),
                registered: HashMap::new(),
                held_ids: HashSet::new(),
                needs_reconcile: false,
            }
        }

        fn disabled_err(&self) -> WindowHotkeyError {
            WindowHotkeyError::RegisterFailed(
                STARTUP_ERROR
                    .get()
                    .and_then(|o| o.clone())
                    .unwrap_or_else(|| "Global hotkeys unavailable.".into()),
            )
        }

        pub fn chord_for_id(&self, id: u32) -> Option<Chord> {
            self.id_to_chord.get(&id).cloned()
        }

        pub fn target_for_id(&self, id: u32) -> Option<WindowTarget> {
            self.id_to_target.get(&id).cloned()
        }

        fn register_chord(
            &mut self,
            chord: &Chord,
            target: WindowTarget,
        ) -> Result<u32, WindowHotkeyError> {
            let Some(manager) = self.manager.as_ref() else {
                return Err(self.disabled_err());
            };
            let hk = chord_to_hotkey(chord)?;
            let id = hk.id();
            if self.registered.contains_key(&id) {
                return Err(WindowHotkeyError::ConflictNative);
            }
            manager
                .register(hk)
                .map_err(|e| WindowHotkeyError::RegisterFailed(e.to_string()))?;
            self.registered.insert(id, hk);
            self.id_to_target.insert(id, target);
            self.id_to_chord.insert(id, normalize_chord(chord));
            Ok(id)
        }

        fn unregister_id(&mut self, id: u32) -> Result<(), WindowHotkeyError> {
            let Some(hk) = self.registered.remove(&id) else {
                self.id_to_target.remove(&id);
                self.id_to_chord.remove(&id);
                self.held_ids.remove(&id);
                return Ok(());
            };
            let Some(manager) = self.manager.as_ref() else {
                self.registered.insert(id, hk);
                return Err(self.disabled_err());
            };
            match manager.unregister(hk) {
                Ok(()) => {
                    self.id_to_target.remove(&id);
                    self.id_to_chord.remove(&id);
                    self.held_ids.remove(&id);
                    Ok(())
                }
                Err(e) => {
                    self.registered.insert(id, hk);
                    Err(WindowHotkeyError::RegisterFailed(e.to_string()))
                }
            }
        }

        pub fn apply_prefs(&mut self, prefs: &DesktopPrefs) -> Result<(), WindowHotkeyError> {
            let want = preferences::window_hotkey_slots(prefs);
            if self.manager.is_none() {
                return if want.is_empty() {
                    Ok(())
                } else {
                    Err(self.disabled_err())
                };
            }
            let mut desired = HashMap::new();
            for (target, chord) in want {
                let id = chord_to_hotkey(&chord)?.id();
                // Binding refuses physical-key twins up front; a hand-edited
                // file that still has one keeps the first slot working
                // instead of losing every window hotkey.
                if desired.contains_key(&id) {
                    eprintln!(
                        "[seance] window hotkey {} shares a physical key with another; skipped",
                        chord.display(true)
                    );
                    continue;
                }
                desired.insert(id, (normalize_chord(&chord), target));
            }
            let before = (
                self.registered.clone(),
                self.id_to_target.clone(),
                self.id_to_chord.clone(),
                self.needs_reconcile,
            );
            let change = (|| {
                // Prove new registrations before releasing any working key.
                for (id, (chord, target)) in &desired {
                    if !self.registered.contains_key(id) {
                        self.register_chord(chord, target.clone())?;
                    }
                }
                for id in before.0.keys().copied() {
                    if !desired.contains_key(&id) {
                        self.unregister_id(id)?;
                    }
                }
                Ok::<(), WindowHotkeyError>(())
            })();
            if let Err(error) = change {
                let mut failures = Vec::new();
                let additions: Vec<_> = self
                    .registered
                    .keys()
                    .copied()
                    .filter(|id| !before.0.contains_key(id))
                    .collect();
                for id in additions {
                    if let Err(rollback) = self.unregister_id(id) {
                        failures.push(rollback.message());
                        // Track reserved keys for cleanup, but don't dispatch
                        // window actions for an uncommitted registration.
                        self.id_to_target.remove(&id);
                        self.id_to_chord.remove(&id);
                    }
                }
                for (id, hotkey) in &before.0 {
                    if !self.registered.contains_key(id) {
                        match self.manager.as_ref().unwrap().register(*hotkey) {
                            Ok(()) => {
                                self.registered.insert(*id, *hotkey);
                            }
                            Err(restore) => failures.push(restore),
                        }
                    }
                    if self.registered.contains_key(id) {
                        if let Some(target) = before.1.get(id) {
                            self.id_to_target.insert(*id, target.clone());
                        }
                        if let Some(chord) = before.2.get(id) {
                            self.id_to_chord.insert(*id, chord.clone());
                        }
                    }
                }
                self.needs_reconcile = before.3 || !failures.is_empty();
                if failures.is_empty() {
                    return Err(error);
                }
                return Err(WindowHotkeyError::RegisterFailed(format!(
                    "{} Rollback also failed: {}. Preferences were not changed; some keys may remain reserved. Retry or restart Seance to reconcile them.",
                    error.message(), failures.join("; ")
                )));
            }
            // Reassigning an already registered physical key needs no OS call.
            self.id_to_target = desired
                .iter()
                .map(|(id, (_, target))| (*id, target.clone()))
                .collect();
            self.id_to_chord = desired
                .into_iter()
                .map(|(id, (chord, _))| (id, chord))
                .collect();
            self.needs_reconcile = false;
            Ok(())
        }

        /// Rebind one slot, keeping every other registration; on failure the
        /// previous registrations stay (see `apply_prefs`).
        pub fn transactional(
            &mut self,
            prefs: &DesktopPrefs,
            slot: &WindowSlot,
            new: Option<Chord>,
        ) -> Result<(), WindowHotkeyError> {
            self.apply_prefs(&with_slot_chord(prefs, slot, new))
        }
    }

    pub fn chord_to_hotkey(chord: &Chord) -> Result<HotKey, WindowHotkeyError> {
        let c = normalize_chord(chord);
        if !chord_is_safe_global(&c) {
            return Err(WindowHotkeyError::UnsupportedChord);
        }
        let code = chord_to_code(&c)?;
        let mut mods = Modifiers::empty();
        if c.ctrl {
            mods |= Modifiers::CONTROL;
        }
        if c.alt {
            mods |= Modifiers::ALT;
        }
        if c.shift {
            mods |= Modifiers::SHIFT;
        }
        if c.meta {
            mods |= Modifiers::SUPER;
        }
        Ok(HotKey::new(Some(mods), code))
    }

    fn chord_to_code(chord: &Chord) -> Result<Code, WindowHotkeyError> {
        let key = chord.key.as_str();
        if key.len() == 1 {
            let ch = key.chars().next().unwrap();
            if ch.is_ascii_alphabetic() {
                let upper = ch.to_ascii_uppercase();
                return parse_code(&format!("Key{upper}"));
            }
            if ch.is_ascii_digit() {
                return parse_code(&format!("Digit{ch}"));
            }
            return punctuation_code(ch, chord.shift);
        }
        parse_code(&match key {
            "space" => "Space",
            "enter" | "return" => "Enter",
            "tab" => "Tab",
            "backspace" => "Backspace",
            "delete" => "Delete",
            "up" => "ArrowUp",
            "down" => "ArrowDown",
            "left" => "ArrowLeft",
            "right" => "ArrowRight",
            "home" => "Home",
            "end" => "End",
            "pageup" => "PageUp",
            "pagedown" => "PageDown",
            "escape" => "Escape",
            other if other.starts_with('f') => {
                let n = other[1..].parse::<u8>().unwrap_or(0);
                return parse_code(&format!("F{n}"));
            }
            other => other,
        })
    }

    fn punctuation_code(ch: char, shift: bool) -> Result<Code, WindowHotkeyError> {
        let (code, needs_shift) = match ch {
            '=' | '+' => (Code::Equal, ch == '+'),
            '-' | '_' => (Code::Minus, ch == '_'),
            ',' | '<' => (Code::Comma, ch == '<'),
            '.' | '>' => (Code::Period, ch == '>'),
            '/' | '?' => (Code::Slash, ch == '?'),
            ';' | ':' => (Code::Semicolon, ch == ':'),
            '\'' | '"' => (Code::Quote, ch == '"'),
            '[' | '{' => (Code::BracketLeft, ch == '{'),
            ']' | '}' => (Code::BracketRight, ch == '}'),
            '\\' | '|' => (Code::Backslash, ch == '|'),
            '`' | '~' => (Code::Backquote, ch == '~'),
            '!' => (Code::Digit1, true),
            '@' => (Code::Digit2, true),
            '#' => (Code::Digit3, true),
            '$' => (Code::Digit4, true),
            '%' => (Code::Digit5, true),
            '^' => (Code::Digit6, true),
            '&' => (Code::Digit7, true),
            '*' => (Code::Digit8, true),
            '(' => (Code::Digit9, true),
            ')' => (Code::Digit0, true),
            _ => return Err(WindowHotkeyError::UnsupportedChord),
        };
        let _ = (needs_shift, shift);
        Ok(code)
    }

    fn parse_code(name: &str) -> Result<Code, WindowHotkeyError> {
        name.parse::<Code>()
            .map_err(|_| WindowHotkeyError::UnsupportedChord)
    }

    #[cfg(test)]
    mod registration_tests {
        use super::*;
        use std::cell::RefCell;
        use std::rc::Rc;

        #[derive(Default)]
        struct State {
            keys: HashSet<u32>,
            fail_register: HashSet<u32>,
            fail_unregister: HashSet<u32>,
            calls: Vec<(bool, u32)>,
        }
        struct FakeRegistrar(Rc<RefCell<State>>);
        impl Registrar for FakeRegistrar {
            fn register(&self, key: HotKey) -> Result<(), String> {
                let mut state = self.0.borrow_mut();
                state.calls.push((true, key.id()));
                if state.fail_register.contains(&key.id()) {
                    return Err("register failed".into());
                }
                state.keys.insert(key.id());
                Ok(())
            }
            fn unregister(&self, key: HotKey) -> Result<(), String> {
                let mut state = self.0.borrow_mut();
                state.calls.push((false, key.id()));
                if state.fail_unregister.contains(&key.id()) {
                    return Err("unregister failed".into());
                }
                state.keys.remove(&key.id());
                Ok(())
            }
        }
        fn setup() -> (NativeHotkeys, Rc<RefCell<State>>) {
            let state = Rc::new(RefCell::new(State::default()));
            let native = NativeHotkeys {
                manager: Some(Box::new(FakeRegistrar(state.clone()))),
                id_to_target: HashMap::new(),
                id_to_chord: HashMap::new(),
                registered: HashMap::new(),
                held_ids: HashSet::new(),
                needs_reconcile: false,
            };
            (native, state)
        }
        fn key(letter: &str) -> Chord {
            Chord {
                ctrl: true,
                alt: true,
                shift: false,
                meta: true,
                key: letter.into(),
            }
        }
        fn id(chord: &Chord) -> u32 {
            chord_to_hotkey(chord).unwrap().id()
        }
        fn prefs(chord: Chord) -> DesktopPrefs {
            DesktopPrefs {
                main_window_hotkey: Some(chord),
                ..DesktopPrefs::default()
            }
        }

        #[test]
        fn rebind_preserves_old_key_when_new_registration_fails() {
            let (mut native, state) = setup();
            let old = prefs(key("a"));
            native.apply_prefs(&old).unwrap();
            state.borrow_mut().fail_register.insert(id(&key("b")));
            assert!(native
                .transactional(&old, &WindowSlot::Main, Some(key("b")))
                .is_err());
            assert_eq!(state.borrow().keys, HashSet::from([id(&key("a"))]));
            assert_eq!(native.target_for_id(id(&key("a"))), Some(WindowSlot::Main));
        }

        #[test]
        fn rebind_rolls_back_new_key_if_old_unregister_fails() {
            let (mut native, state) = setup();
            let old = prefs(key("a"));
            native.apply_prefs(&old).unwrap();
            state.borrow_mut().fail_unregister.insert(id(&key("a")));
            assert!(native
                .transactional(&old, &WindowSlot::Main, Some(key("b")))
                .is_err());
            assert_eq!(state.borrow().keys, HashSet::from([id(&key("a"))]));
            assert!(!native.needs_reconcile);
        }

        #[test]
        fn rollback_failure_is_reported_and_uncommitted_key_cannot_toggle() {
            let (mut native, state) = setup();
            let old = prefs(key("a"));
            native.apply_prefs(&old).unwrap();
            state
                .borrow_mut()
                .fail_unregister
                .extend([id(&key("a")), id(&key("b"))]);
            let error = native
                .transactional(&old, &WindowSlot::Main, Some(key("b")))
                .unwrap_err();
            assert!(error.message().contains("Rollback also failed"));
            assert!(native.needs_reconcile);
            assert_eq!(native.target_for_id(id(&key("b"))), None);
            state.borrow_mut().fail_unregister.clear();
            native.apply_prefs(&old).unwrap();
            assert!(!native.needs_reconcile);
            assert_eq!(state.borrow().keys, HashSet::from([id(&key("a"))]));
        }

        #[test]
        fn reconciliation_proves_additions_before_releasing_existing_keys() {
            let (mut native, state) = setup();
            native.apply_prefs(&prefs(key("a"))).unwrap();
            state.borrow_mut().fail_register.insert(id(&key("b")));
            assert!(native.apply_prefs(&prefs(key("b"))).is_err());
            assert!(state.borrow().keys.contains(&id(&key("a"))));
        }

        fn two_seances() -> (DesktopPrefs, String, String) {
            let mut prefs = DesktopPrefs::default();
            let a = preferences::create_seance_in(&mut prefs, "Alpha", 10).unwrap();
            let b = preferences::create_seance_in(&mut prefs, "Beta", 10).unwrap();
            let prefs = with_slot_chord(&prefs, &WindowSlot::Seance(a.clone()), Some(key("a")));
            let prefs = with_slot_chord(&prefs, &WindowSlot::Seance(b.clone()), Some(key("b")));
            (prefs, a, b)
        }

        #[test]
        fn removing_seance_keeps_other_target_and_unchanged_is_noop() {
            let (mut prefs, a, b) = two_seances();
            let (mut native, state) = setup();
            native.apply_prefs(&prefs).unwrap();
            state.borrow_mut().calls.clear();
            native.apply_prefs(&prefs).unwrap();
            assert!(state.borrow().calls.is_empty());
            preferences::remove_seance_in(&mut prefs, &a).unwrap();
            native.apply_prefs(&prefs).unwrap();
            assert_eq!(native.target_for_id(id(&key("a"))), None);
            assert_eq!(
                native.target_for_id(id(&key("b"))),
                Some(WindowSlot::Seance(b))
            );
            assert_eq!(state.borrow().keys, HashSet::from([id(&key("b"))]));
        }

        #[test]
        fn renaming_seance_keeps_registration_target_and_skips_os_calls() {
            let (mut prefs, a, _) = two_seances();
            let (mut native, state) = setup();
            native.apply_prefs(&prefs).unwrap();
            state.borrow_mut().calls.clear();
            preferences::rename_seance_in(&mut prefs, &a, "Client work").unwrap();
            native.apply_prefs(&prefs).unwrap();
            assert!(state.borrow().calls.is_empty());
            assert_eq!(
                native.target_for_id(id(&key("a"))),
                Some(WindowSlot::Seance(a))
            );
        }

        #[test]
        fn readded_seance_with_same_name_gets_a_fresh_target() {
            let (mut prefs, a, _) = two_seances();
            let (mut native, _state) = setup();
            native.apply_prefs(&prefs).unwrap();
            preferences::remove_seance_in(&mut prefs, &a).unwrap();
            native.apply_prefs(&prefs).unwrap();
            let again = preferences::create_seance_in(&mut prefs, "Alpha", 11).unwrap();
            assert_ne!(again, a);
            native
                .transactional(&prefs, &WindowSlot::Seance(again.clone()), Some(key("a")))
                .unwrap();
            assert_eq!(
                native.target_for_id(id(&key("a"))),
                Some(WindowSlot::Seance(again))
            );
        }

        #[test]
        fn failed_seance_rebind_keeps_every_existing_key() {
            let (prefs, a, b) = two_seances();
            let (mut native, state) = setup();
            native.apply_prefs(&prefs).unwrap();
            state.borrow_mut().fail_register.insert(id(&key("c")));
            assert!(native
                .transactional(&prefs, &WindowSlot::Seance(a.clone()), Some(key("c")))
                .is_err());
            assert_eq!(
                state.borrow().keys,
                HashSet::from([id(&key("a")), id(&key("b"))])
            );
            assert_eq!(
                native.target_for_id(id(&key("a"))),
                Some(WindowSlot::Seance(a))
            );
            assert_eq!(
                native.target_for_id(id(&key("b"))),
                Some(WindowSlot::Seance(b))
            );
        }

        #[test]
        fn physical_key_twin_keeps_first_slot_instead_of_failing_all() {
            let (mut prefs, a, b) = two_seances();
            prefs.main_window_hotkey = Some(key("m"));
            prefs = with_slot_chord(&prefs, &WindowSlot::Seance(b), Some(key("a")));
            let (mut native, state) = setup();
            native.apply_prefs(&prefs).unwrap();
            assert_eq!(
                native.target_for_id(id(&key("a"))),
                Some(WindowSlot::Seance(a))
            );
            assert_eq!(native.target_for_id(id(&key("m"))), Some(WindowSlot::Main));
            assert_eq!(state.borrow().keys.len(), 2);
        }
    }
}

#[cfg(not(target_os = "macos"))]
mod native {
    use super::*;
    use gpui::App;

    pub struct NativeHotkeys;

    impl NativeHotkeys {
        pub fn new(_cx: &mut App) -> Self {
            Self
        }
        pub fn chord_for_id(&self, _id: u32) -> Option<Chord> {
            None
        }
        pub fn target_for_id(&self, _id: u32) -> Option<WindowTarget> {
            None
        }
        pub fn apply_prefs(&mut self, _prefs: &DesktopPrefs) -> Result<(), WindowHotkeyError> {
            Ok(())
        }
        pub fn transactional(
            &mut self,
            _prefs: &DesktopPrefs,
            _slot: &WindowSlot,
            _new: Option<Chord>,
        ) -> Result<(), WindowHotkeyError> {
            Err(WindowHotkeyError::RegisterFailed(
                "Global window hotkeys are only supported on macOS.".into(),
            ))
        }
    }

    pub fn startup_error() -> Option<&'static str> {
        None
    }

    pub fn chord_to_hotkey(_chord: &Chord) -> Result<(), WindowHotkeyError> {
        Err(WindowHotkeyError::UnsupportedChord)
    }
}

/// Native hotkey identity for conflict detection (macOS); logical fallback elsewhere.
pub fn native_chord_id(chord: &Chord) -> Option<u32> {
    #[cfg(target_os = "macos")]
    {
        return native::chord_to_hotkey(chord).ok().map(|hk| hk.id());
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = chord;
        None
    }
}

#[allow(dead_code)]
pub fn chords_native_collision(a: &Chord, b: &Chord) -> bool {
    match (native_chord_id(a), native_chord_id(b)) {
        (Some(x), Some(y)) => x == y,
        _ => false,
    }
}

fn native_collision_against_prefs(
    prefs: &DesktopPrefs,
    candidate: &Chord,
    skip: Option<&WindowSlot>,
) -> Result<(), WindowHotkeyError> {
    let Some(id) = native_chord_id(candidate) else {
        return Ok(());
    };
    for (slot, chord) in preferences::window_hotkey_slots(prefs) {
        if skip != Some(&slot) && native_chord_id(&chord) == Some(id) {
            return Err(WindowHotkeyError::ConflictWindow(prefs.slot_label(&slot)));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use preferences::make_chord;

    #[test]
    fn overlay_opens_hidden_only_where_supported() {
        assert!(!overlay_supported(false));
        assert_eq!(overlay_supported(true), cfg!(target_os = "macos"));
    }

    #[test]
    fn native_id_distinguishes_ctrl_from_cmd_on_macos() {
        if !cfg!(target_os = "macos") {
            return;
        }
        let ctrl = make_chord(true, false, false, "j");
        let cmd = make_chord(false, true, false, "j");
        let a = native_chord_id(&ctrl);
        let b = native_chord_id(&cmd);
        assert!(a.is_some() && b.is_some());
        assert_ne!(a, b);
    }

    #[test]
    fn plus_and_equals_differ_when_shift_differs_on_macos() {
        if !cfg!(target_os = "macos") {
            return;
        }
        let plain = make_chord(false, true, false, "=");
        let shifted = make_chord(false, true, true, "+");
        assert_ne!(native_chord_id(&plain), native_chord_id(&shifted));
    }

    #[test]
    fn window_scope_projects_by_seance_id() {
        let mut prefs = DesktopPrefs::default();
        let id = preferences::create_seance_in(&mut prefs, "Client work", 1).unwrap();
        preferences::assign_circle_in(&mut prefs, "nuance-api", Some(&id));
        let seance = WindowScope::Seance(id.clone());
        assert_eq!(seance.slot(), Some(WindowSlot::Seance(id)));
        assert!(seance.projection(&prefs).admits("nuance-api"));
        assert!(!seance.projection(&prefs).admits("home"));
        assert!(!WindowScope::Main.projection(&prefs).admits("nuance-api"));
        assert!(WindowScope::Main.projection(&prefs).admits("home"));
        assert!(WindowScope::Blank.projection(&prefs).admits("nuance-api"));
        assert_eq!(WindowScope::Blank.slot(), None);
    }
}
