//! System-wide window show/hide hotkeys (macOS Carbon via `global-hotkey`).
//!
//! Process-wide coordinator (`gpui::Global`): native registration, per-window
//! handles, workspace catalog for Settings, and UI-thread visibility toggles.

use std::collections::HashMap;

use gpui::{AnyWindowHandle, App, AppContext, Global, SharedString, WeakEntity, Window};
use gpui_component::Root;

use super::preferences::{
    self, chord_is_safe_global, desktop_prefs, normalize_chord, schedule_desktop_save, Chord,
    DesktopPrefs, WorkspaceWindowDefinition,
};

/// Which Seance surface a dedicated OS window represents.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WindowScope {
    Main,
    Blank,
    Workspace(String),
}

impl WindowScope {
    pub fn is_blank(&self) -> bool {
        matches!(self, WindowScope::Blank)
    }

    pub fn fixed_workspace(&self) -> Option<&str> {
        match self {
            WindowScope::Workspace(s) => Some(s.as_str()),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WindowTarget {
    Main,
    Workspace { slug: String },
}

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
    MainWindow,
    WorkspaceRow(usize),
    AppShortcut(preferences::AppAction),
}

struct SettingsRecorder {
    entity: WeakEntity<super::settings::SettingsWindow>,
    capture: SettingsCapture,
    /// Preserved for API compat; routing uses the pressed hotkey id map.
    registered_chord: Option<Chord>,
}

pub struct WindowHotkeys {
    main_handle: Option<AnyWindowHandle>,
    workspace_handles: HashMap<String, AnyWindowHandle>,
    catalog: Vec<(String, String)>,
    last_error: Option<String>,
    settings_recorder: Option<SettingsRecorder>,
    #[cfg(target_os = "macos")]
    native: native::NativeHotkeys,
    #[cfg(not(target_os = "macos"))]
    native: native::NativeHotkeys,
}

impl Global for WindowHotkeys {}

impl WindowHotkeys {
    pub fn init(cx: &mut App) {
        if cx.has_global::<Self>() {
            return;
        }
        let mut hotkeys = Self {
            main_handle: None,
            workspace_handles: HashMap::new(),
            catalog: Vec::new(),
            last_error: None,
            settings_recorder: None,
            native: native::NativeHotkeys::new(cx),
        };
        hotkeys.sync_native();
        cx.set_global(hotkeys);
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

    pub fn publish_catalog(cx: &mut App, slugs_labels: impl IntoIterator<Item = (String, String)>) {
        let mut v: Vec<_> = slugs_labels.into_iter().collect();
        v.sort_by(|a, b| a.1.to_lowercase().cmp(&b.1.to_lowercase()));
        Self::global_mut(cx).catalog = v;
    }

    pub fn catalog(cx: &App) -> Vec<(String, String)> {
        Self::global(cx).catalog.clone()
    }

    pub fn register_main(cx: &mut App, handle: AnyWindowHandle) {
        Self::global_mut(cx).main_handle = Some(handle);
    }

    pub fn register_workspace(cx: &mut App, slug: String, handle: AnyWindowHandle) {
        Self::global_mut(cx).workspace_handles.insert(slug, handle);
    }

    pub fn unregister_workspace(cx: &mut App, slug: &str) {
        Self::global_mut(cx).workspace_handles.remove(slug);
    }

    pub fn clear_main(cx: &mut App) {
        Self::global_mut(cx).main_handle = None;
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

    pub fn bind_main(cx: &mut App, chord: Option<Chord>) -> Result<(), WindowHotkeyError> {
        Self::bind_main_inner(cx, chord, true)
    }

    fn bind_main_inner(
        cx: &mut App,
        chord: Option<Chord>,
        persist: bool,
    ) -> Result<(), WindowHotkeyError> {
        let mac = cfg!(target_os = "macos");
        let prefs = desktop_prefs().read().unwrap().clone();
        if let Some(c) = chord.as_ref() {
            let scratch = {
                let mut s = prefs.clone();
                s.main_window_hotkey = None;
                s
            };
            preferences::validate_window_hotkey_candidate(&scratch, c, None, mac)
                .map_err(map_bind_err)?;
            native_collision_against_prefs(&scratch, c)?;
        }
        {
            let g = Self::global_mut(cx);
            g.native
                .transactional_main(&prefs, chord.clone())
                .map_err(|e| {
                    g.last_error = Some(e.message());
                    e
                })?;
        }
        if persist {
            let mut prefs = desktop_prefs().write().unwrap();
            prefs.main_window_hotkey = chord.map(|c| normalize_chord(&c));
            schedule_desktop_save();
        }
        Self::global_mut(cx).last_error = None;
        Ok(())
    }

    pub fn bind_workspace(
        cx: &mut App,
        index: usize,
        chord: Option<Chord>,
    ) -> Result<(), WindowHotkeyError> {
        let mac = cfg!(target_os = "macos");
        let prefs = desktop_prefs().read().unwrap().clone();
        if let Some(c) = chord.as_ref() {
            let scratch = {
                let mut s = prefs.clone();
                if index < s.workspace_windows.len() {
                    s.workspace_windows[index].shortcut = None;
                }
                s
            };
            preferences::validate_window_hotkey_candidate(&scratch, c, Some(index), mac)
                .map_err(map_bind_err)?;
            native_collision_against_prefs(&scratch, c)?;
        }
        {
            let g = Self::global_mut(cx);
            g.native
                .transactional_workspace(index, &prefs, chord.clone())
                .map_err(|e| {
                    g.last_error = Some(e.message());
                    e
                })?;
        }
        let mut prefs = desktop_prefs().write().unwrap();
        if index < prefs.workspace_windows.len() {
            prefs.workspace_windows[index].shortcut = chord.map(|c| normalize_chord(&c));
            schedule_desktop_save();
        }
        Self::global_mut(cx).last_error = None;
        Ok(())
    }

    pub fn add_workspace_definition(_cx: &mut App, workspace: String) -> Result<(), String> {
        let mut prefs = desktop_prefs().write().unwrap();
        if prefs
            .workspace_windows
            .iter()
            .any(|d| d.workspace == workspace)
        {
            return Err("That workspace already has a dedicated window.".into());
        }
        prefs.workspace_windows.push(WorkspaceWindowDefinition {
            workspace,
            shortcut: None,
        });
        schedule_desktop_save();
        Ok(())
    }

    pub fn remove_workspace_definition(cx: &mut App, index: usize) {
        let slug = {
            let prefs = desktop_prefs().read().unwrap();
            if index >= prefs.workspace_windows.len() {
                return;
            }
            prefs.workspace_windows[index].workspace.clone()
        };

        #[cfg(target_os = "macos")]
        {
            let native_err = Self::global_mut(cx).native.unregister_workspace_slug(&slug);
            if let Err(e) = native_err {
                Self::global_mut(cx).last_error = Some(e.message());
                return;
            }
        }

        {
            let mut prefs = desktop_prefs().write().unwrap();
            let Some(pos) = prefs
                .workspace_windows
                .iter()
                .position(|d| d.workspace == slug)
            else {
                return;
            };
            prefs.workspace_windows.remove(pos);
            schedule_desktop_save();
        }

        let handle = Self::global(cx).workspace_handles.get(&slug).copied();
        if let Some(handle) = handle {
            let _ = handle.update(cx, |_, window, _| window.remove_window());
        }
        Self::global_mut(cx).workspace_handles.remove(&slug);
        Self::global_mut(cx).last_error = None;
    }

    pub fn toggle_target(cx: &mut App, target: WindowTarget) {
        match target {
            WindowTarget::Main => {
                let handle = Self::global(cx).main_handle;
                if let Some(handle) = handle {
                    if handle
                        .update(cx, |_, window, _| toggle_visibility(window))
                        .is_ok()
                    {
                        return;
                    }
                    Self::global_mut(cx).main_handle = None;
                }
                open_main_window(cx);
            }
            WindowTarget::Workspace { slug } => {
                let handle = Self::global(cx).workspace_handles.get(&slug).copied();
                if let Some(handle) = handle {
                    if handle
                        .update(cx, |_, window, _| toggle_visibility(window))
                        .is_ok()
                    {
                        return;
                    }
                    Self::global_mut(cx).workspace_handles.remove(&slug);
                }
                open_workspace_window(cx, slug);
            }
        }
    }

    pub fn open_or_toggle_main(cx: &mut App) {
        Self::toggle_target(cx, WindowTarget::Main);
    }

    pub fn open_or_toggle_workspace(cx: &mut App, index: usize) {
        let slug = {
            let prefs = desktop_prefs().read().unwrap();
            prefs
                .workspace_windows
                .get(index)
                .map(|d| d.workspace.clone())
        };
        if let Some(slug) = slug {
            Self::toggle_target(cx, WindowTarget::Workspace { slug });
        }
    }

    /// One GUI in this process owns desktop notifications and the telegram
    /// status bridge — main when it is live, otherwise the lexicographically
    /// first live dedicated workspace window.
    pub fn is_notification_owner(cx: &mut App, own_window: AnyWindowHandle) -> bool {
        fn live(cx: &mut App, handle: AnyWindowHandle) -> bool {
            handle.update(cx, |_, _, _| ()).is_ok()
        }
        let main_handle = Self::global(cx).main_handle;
        if main_handle.is_some_and(|h| live(cx, h)) {
            return main_handle == Some(own_window);
        }
        let workspace_handles: Vec<(String, AnyWindowHandle)> = Self::global(cx)
            .workspace_handles
            .iter()
            .map(|(slug, handle)| (slug.clone(), *handle))
            .collect();
        let mut live_workspaces: Vec<_> = workspace_handles
            .into_iter()
            .filter(|(_, h)| live(cx, *h))
            .collect();
        live_workspaces.sort_by(|a, b| a.0.cmp(&b.0));
        live_workspaces
            .first()
            .is_some_and(|(_, h)| *h == own_window)
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

pub fn open_main_window(cx: &mut App) {
    let bounds = gpui::Bounds::centered(None, gpui::size(gpui::px(1480.), gpui::px(920.)), cx);
    let _ = cx.open_window(
        gpui::WindowOptions {
            window_bounds: Some(gpui::WindowBounds::Windowed(bounds)),
            titlebar: Some(gpui::TitlebarOptions {
                title: Some(SharedString::from("seance")),
                ..Default::default()
            }),
            app_id: Some("seance".into()),
            ..Default::default()
        },
        |window, cx| {
            let view = cx.new(|cx| super::SeanceApp::new(window, cx));
            let handle = window.window_handle();
            WindowHotkeys::register_main(cx, handle);
            window.on_window_should_close(cx, move |_, cx| {
                WindowHotkeys::clear_main(cx);
                true
            });
            cx.new(|cx| Root::new(view, window, cx))
        },
    );
}

pub fn open_workspace_window(cx: &mut App, slug: String) {
    if let Some(handle) = WindowHotkeys::global(cx)
        .workspace_handles
        .get(&slug)
        .copied()
    {
        if handle
            .update(cx, |_, window, _| window.activate_window())
            .is_ok()
        {
            return;
        }
        WindowHotkeys::global_mut(cx)
            .workspace_handles
            .remove(&slug);
    }
    let bounds = gpui::Bounds::centered(None, gpui::size(gpui::px(1280.), gpui::px(800.)), cx);
    let slug_owned = slug.clone();
    let _ = cx.open_window(
        gpui::WindowOptions {
            window_bounds: Some(gpui::WindowBounds::Windowed(bounds)),
            titlebar: Some(gpui::TitlebarOptions {
                title: Some(SharedString::from(format!("seance — {}", slug))),
                ..Default::default()
            }),
            app_id: Some("seance".into()),
            ..Default::default()
        },
        move |window, cx| {
            let view = cx.new(|cx| super::SeanceApp::new_workspace_window(window, cx, &slug_owned));
            let client = view.read(cx).client.clone();
            let handle = window.window_handle();
            WindowHotkeys::register_workspace(cx, slug_owned.clone(), handle);
            window.on_window_should_close(cx, move |_, cx| {
                WindowHotkeys::unregister_workspace(cx, &slug_owned);
                client.disconnect();
                true
            });
            cx.new(|cx| Root::new(view, window, cx))
        },
    );
}

fn toggle_visibility(window: &mut Window) {
    #[cfg(target_os = "macos")]
    {
        match macos_visibility::toggle_action(window) {
            macos_visibility::ToggleAction::Hide => macos_visibility::hide(window),
            macos_visibility::ToggleAction::Show => macos_visibility::show(window),
            macos_visibility::ToggleAction::GpuiFallback => window.activate_window(),
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        window.activate_window();
    }
}

#[cfg(target_os = "macos")]
mod macos_visibility {
    #![allow(unexpected_cfgs)]
    use cocoa::appkit::NSApp;
    use cocoa::base::{id, nil};
    use gpui::Window;
    use objc::{msg_send, sel, sel_impl};
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};

    pub enum ToggleAction {
        Hide,
        Show,
        GpuiFallback,
    }

    fn ns_window(window: &Window) -> Option<id> {
        let raw = HasWindowHandle::window_handle(window).ok()?;
        let RawWindowHandle::AppKit(appkit) = raw.as_raw() else {
            return None;
        };
        #[allow(deprecated)]
        unsafe {
            let view: id = appkit.ns_view.as_ptr() as id;
            let nswindow: id = msg_send![view, window];
            if nswindow == nil {
                return None;
            }
            Some(nswindow)
        }
    }

    pub fn toggle_action(window: &Window) -> ToggleAction {
        let Some(nswindow) = ns_window(window) else {
            return ToggleAction::GpuiFallback;
        };
        #[allow(deprecated)]
        unsafe {
            let app: id = NSApp();
            let app_active: bool = msg_send![app, isActive];
            let visible: bool = msg_send![nswindow, isVisible];
            let mini: bool = msg_send![nswindow, isMiniaturized];
            let key: bool = msg_send![nswindow, isKeyWindow];
            if !visible || mini {
                return ToggleAction::Show;
            }
            if visible && key && app_active {
                return ToggleAction::Hide;
            }
            ToggleAction::Show
        }
    }

    pub fn hide(window: &Window) {
        if let Some(nswindow) = ns_window(window) {
            #[allow(deprecated)]
            unsafe {
                let _: () = msg_send![nswindow, orderOut: nil];
            }
        }
    }

    pub fn show(window: &mut Window) {
        if let Some(nswindow) = ns_window(window) {
            #[allow(deprecated)]
            unsafe {
                let mini: bool = msg_send![nswindow, isMiniaturized];
                if mini {
                    let _: () = msg_send![nswindow, deminiaturize: nil];
                }
                let _: () = msg_send![nswindow, makeKeyAndOrderFront: nil];
            }
            window.activate_window();
        }
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

        pub fn unregister_workspace_slug(&mut self, slug: &str) -> Result<(), WindowHotkeyError> {
            if self.manager.is_none() {
                return Ok(());
            }
            let ids: Vec<u32> = self
                .id_to_target
                .iter()
                .filter_map(|(id, t)| match t {
                    WindowTarget::Workspace { slug: s } if s == slug => Some(*id),
                    _ => None,
                })
                .collect();
            for id in ids {
                self.unregister_id(id)?;
            }
            Ok(())
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

        fn want_from_prefs(prefs: &DesktopPrefs) -> Vec<(Chord, WindowTarget)> {
            let mut want = Vec::new();
            if let Some(c) = prefs.main_window_hotkey.clone() {
                want.push((c, WindowTarget::Main));
            }
            for def in &prefs.workspace_windows {
                if let Some(c) = def.shortcut.clone() {
                    want.push((
                        c,
                        WindowTarget::Workspace {
                            slug: def.workspace.clone(),
                        },
                    ));
                }
            }
            want
        }

        pub fn apply_prefs(&mut self, prefs: &DesktopPrefs) -> Result<(), WindowHotkeyError> {
            let want = Self::want_from_prefs(prefs);
            if self.manager.is_none() {
                return if want.is_empty() {
                    Ok(())
                } else {
                    Err(self.disabled_err())
                };
            }
            let mut desired = HashMap::new();
            for (chord, target) in want {
                let id = chord_to_hotkey(&chord)?.id();
                if desired
                    .insert(id, (normalize_chord(&chord), target))
                    .is_some()
                {
                    return Err(WindowHotkeyError::ConflictNative);
                }
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

        pub fn transactional_main(
            &mut self,
            prefs: &DesktopPrefs,
            new: Option<Chord>,
        ) -> Result<(), WindowHotkeyError> {
            let mut candidate = prefs.clone();
            candidate.main_window_hotkey = new;
            self.apply_prefs(&candidate)
        }

        pub fn transactional_workspace(
            &mut self,
            index: usize,
            prefs: &DesktopPrefs,
            new: Option<Chord>,
        ) -> Result<(), WindowHotkeyError> {
            let mut candidate = prefs.clone();
            let def = candidate
                .workspace_windows
                .get_mut(index)
                .ok_or_else(|| WindowHotkeyError::RegisterFailed("workspace row missing".into()))?;
            def.shortcut = new;
            self.apply_prefs(&candidate)
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
            assert!(native.transactional_main(&old, Some(key("b"))).is_err());
            assert_eq!(state.borrow().keys, HashSet::from([id(&key("a"))]));
            assert_eq!(
                native.target_for_id(id(&key("a"))),
                Some(WindowTarget::Main)
            );
        }

        #[test]
        fn rebind_rolls_back_new_key_if_old_unregister_fails() {
            let (mut native, state) = setup();
            let old = prefs(key("a"));
            native.apply_prefs(&old).unwrap();
            state.borrow_mut().fail_unregister.insert(id(&key("a")));
            assert!(native.transactional_main(&old, Some(key("b"))).is_err());
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
            let error = native.transactional_main(&old, Some(key("b"))).unwrap_err();
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

        #[test]
        fn removing_workspace_keeps_other_slug_and_unchanged_is_noop() {
            let (mut native, state) = setup();
            let mut initial = DesktopPrefs::default();
            initial.workspace_windows = vec![
                WorkspaceWindowDefinition {
                    workspace: "alpha".into(),
                    shortcut: Some(key("a")),
                },
                WorkspaceWindowDefinition {
                    workspace: "beta".into(),
                    shortcut: Some(key("b")),
                },
            ];
            native.apply_prefs(&initial).unwrap();
            state.borrow_mut().calls.clear();
            native.apply_prefs(&initial).unwrap();
            assert!(state.borrow().calls.is_empty());
            initial.workspace_windows.remove(0);
            native.apply_prefs(&initial).unwrap();
            assert_eq!(
                native.target_for_id(id(&key("b"))),
                Some(WindowTarget::Workspace {
                    slug: "beta".into()
                })
            );
            assert_eq!(state.borrow().keys, HashSet::from([id(&key("b"))]));
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
        pub fn unregister_workspace_slug(&mut self, _slug: &str) -> Result<(), WindowHotkeyError> {
            Ok(())
        }
        pub fn apply_prefs(&mut self, _prefs: &DesktopPrefs) -> Result<(), WindowHotkeyError> {
            Ok(())
        }
        pub fn transactional_main(
            &mut self,
            _prefs: &DesktopPrefs,
            _new: Option<Chord>,
        ) -> Result<(), WindowHotkeyError> {
            Err(WindowHotkeyError::RegisterFailed(
                "Global window hotkeys are only supported on macOS.".into(),
            ))
        }
        pub fn transactional_workspace(
            &mut self,
            _index: usize,
            _prefs: &DesktopPrefs,
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
) -> Result<(), WindowHotkeyError> {
    let id = native_chord_id(candidate);
    if id.is_none() {
        return Ok(());
    }
    let id = id.unwrap();
    if let Some(c) = prefs.main_window_hotkey.as_ref() {
        if native_chord_id(c) == Some(id) {
            return Err(WindowHotkeyError::ConflictWindow("main window".into()));
        }
    }
    for def in &prefs.workspace_windows {
        if let Some(c) = def.shortcut.as_ref() {
            if native_chord_id(c) == Some(id) {
                return Err(WindowHotkeyError::ConflictWindow(def.workspace.clone()));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use preferences::make_chord;

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
    fn window_scope_fixed_slug() {
        let s = WindowScope::Workspace("nuance".into());
        assert_eq!(s.fixed_workspace(), Some("nuance"));
        assert!(WindowScope::Blank.is_blank());
    }
}
