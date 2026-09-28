//! Desktop settings window — font, colors, shortcuts (device-local prefs).

use std::collections::HashSet;
use std::sync::Mutex;

use crate::theme::SeancePalette;
use gpui::{
    div, prelude::*, px, AnyWindowHandle, App, Context, FocusHandle, Focusable, Render,
    SharedString, Window,
};
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::Root;

use super::colors::{self, ColorScheme, ImportResult};
use super::preferences::{
    self, adjust_font_size, apply_color_scheme_from_settings, apply_font_from_settings,
    chords_for_action, clamp_font_size, desktop_prefs, reset_action, reset_all_shortcuts,
    reset_color_scheme_defaults, reset_font_defaults, reset_terminal_font_size, save_error,
    try_bind, AppAction, BindError, Chord,
};
use super::window_hotkeys::{self, SettingsCapture, WindowHotkeys};
use super::SeanceApp;
use crate::term_font;

static SETTINGS_WINDOW: Mutex<Option<AnyWindowHandle>> = Mutex::new(None);

pub fn register_settings_window(handle: AnyWindowHandle) {
    if let Ok(mut g) = SETTINGS_WINDOW.lock() {
        *g = Some(handle);
    }
}

pub fn clear_settings_window() {
    if let Ok(mut g) = SETTINGS_WINDOW.lock() {
        *g = None;
    }
}

pub fn focus_existing_settings(cx: &mut App) -> bool {
    let handle = SETTINGS_WINDOW.lock().ok().and_then(|g| g.clone());
    if let Some(handle) = handle {
        return cx
            .update_window(handle, |_, window, _| window.activate_window())
            .is_ok();
    }
    false
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SettingsPage {
    Font,
    Colors,
    Keyboard,
    Windows,
}

const COLOR_FIELD_LABELS: [&str; 23] = [
    "Foreground",
    "Background",
    "Cursor",
    "Cursor text",
    "Selection background",
    "Selection foreground",
    "Bold (optional)",
    "ANSI 0",
    "ANSI 1",
    "ANSI 2",
    "ANSI 3",
    "ANSI 4",
    "ANSI 5",
    "ANSI 6",
    "ANSI 7",
    "ANSI 8",
    "ANSI 9",
    "ANSI 10",
    "ANSI 11",
    "ANSI 12",
    "ANSI 13",
    "ANSI 14",
    "ANSI 15",
];

/// Separate window so capture never leaks into live PTYs.
pub struct SettingsWindow {
    page: SettingsPage,
    focus_handle: FocusHandle,
    draft_family: String,
    draft_size: f32,
    monospace_fonts: Vec<String>,
    capture: Option<AppAction>,
    windows_capture: Option<SettingsCapture>,
    bind_error: Option<String>,
    row_error: Option<String>,
    polled_save_err: Option<String>,
    installed: HashSet<String>,
    pick_workspace: String,
    color_fields: Vec<(SharedString, gpui::Entity<InputState>)>,
    color_apply_error: Option<String>,
    color_import_busy: bool,
    color_import_error: Option<String>,
    color_import_warnings: Vec<String>,
    color_import_profiles: Vec<ColorScheme>,
    color_import_idx: usize,
}

impl SettingsWindow {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let installed: HashSet<String> = cx.text_system().all_font_names().into_iter().collect();
        let prefs = desktop_prefs().read().unwrap().clone();
        let color_fields = Self::make_color_field_inputs(window, cx, &prefs.terminal_color_scheme);
        let mut monospace_fonts = installed
            .iter()
            .filter(|name| term_font::probe_monospace(cx.text_system(), name, prefs.font_size))
            .cloned()
            .collect::<Vec<_>>();
        monospace_fonts.sort();
        let polled_save_err = save_error();
        let entity = cx.weak_entity();
        cx.spawn(async move |_, cx| loop {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(500))
                .await;
            let err = save_error();
            if let Some(view) = entity.upgrade() {
                view.update(cx, |this, cx| {
                    if this.polled_save_err != err {
                        this.polled_save_err = err.clone();
                        cx.notify();
                    }
                });
            } else {
                break;
            }
        })
        .detach();
        Self {
            page: SettingsPage::Font,
            focus_handle: cx.focus_handle(),
            draft_family: prefs.font_family,
            draft_size: prefs.font_size,
            monospace_fonts,
            capture: None,
            windows_capture: None,
            bind_error: None,
            row_error: None,
            polled_save_err,
            installed,
            pick_workspace: String::new(),
            color_fields,
            color_apply_error: None,
            color_import_busy: false,
            color_import_error: None,
            color_import_warnings: Vec::new(),
            color_import_profiles: Vec::new(),
            color_import_idx: 0,
        }
    }

    fn make_color_field_inputs(
        window: &mut Window,
        cx: &mut Context<Self>,
        scheme: &ColorScheme,
    ) -> Vec<(SharedString, gpui::Entity<InputState>)> {
        let values = color_scheme_to_field_hex(scheme);
        COLOR_FIELD_LABELS
            .iter()
            .zip(values)
            .map(|(label, value)| {
                let input = cx.new(|cx| InputState::new(window, cx).default_value(value));
                cx.subscribe(&input, |this, _, event, cx| {
                    if matches!(event, InputEvent::Change) {
                        this.color_apply_error = None;
                        cx.notify();
                    }
                })
                .detach();
                (SharedString::from(*label), input)
            })
            .collect()
    }

    fn sync_color_fields_from_scheme(&self, scheme: ColorScheme, cx: &mut Context<Self>) {
        let values = color_scheme_to_field_hex(&scheme);
        let handle = SETTINGS_WINDOW.lock().ok().and_then(|g| g.clone());
        let Some(handle) = handle else {
            return;
        };
        let inputs = self.color_fields.clone();
        cx.defer(move |cx| {
            let _ = cx.update_window(handle, |_, window, cx| {
                for ((_, input), value) in inputs.iter().zip(values) {
                    input.update(cx, |state, cx| {
                        state.set_value(value, window, cx);
                    });
                }
            });
        });
    }

    fn read_color_scheme_from_fields(&self, cx: &App) -> Result<ColorScheme, String> {
        let mut values = Vec::with_capacity(COLOR_FIELD_LABELS.len());
        for (_, input) in &self.color_fields {
            values.push(input.read(cx).value().to_string());
        }
        let ansi: [String; 16] = std::array::from_fn(|i| values[7 + i].clone());
        colors::color_scheme_from_fields(
            "Custom", &values[0], &values[1], &ansi, &values[2], &values[3], &values[4],
            &values[5], &values[6],
        )
    }

    pub fn complete_hotkey_capture(
        &mut self,
        capture: SettingsCapture,
        chord: Option<Chord>,
        cx: &mut Context<Self>,
    ) {
        if self.windows_capture.as_ref() != Some(&capture) {
            return;
        }
        let is_app_capture = matches!(capture, SettingsCapture::AppShortcut(_));
        let result = match capture {
            SettingsCapture::MainWindow => {
                window_hotkeys::WindowHotkeys::bind_main(cx, chord.clone())
            }
            SettingsCapture::WorkspaceRow(i) => {
                window_hotkeys::WindowHotkeys::bind_workspace(cx, i, chord.clone())
            }
            SettingsCapture::AppShortcut(action) => {
                if let Some(c) = chord {
                    try_bind(action, vec![c], cfg!(target_os = "macos")).map_err(|e| match e {
                        BindError::ConflictWindowHotkey => {
                            window_hotkeys::WindowHotkeyError::ConflictWindow(
                                "a window show/hide hotkey".into(),
                            )
                        }
                        BindError::Conflict(a) => window_hotkeys::WindowHotkeyError::ConflictApp(a),
                        BindError::Protected => window_hotkeys::WindowHotkeyError::UnsupportedChord,
                        BindError::UnsafeBareKey => {
                            window_hotkeys::WindowHotkeyError::UnsupportedChord
                        }
                    })
                } else {
                    Err(window_hotkeys::WindowHotkeyError::UnsupportedChord)
                }
            }
        };
        match result {
            Ok(()) => {
                self.capture = None;
                self.windows_capture = None;
                let weak = cx.weak_entity();
                window_hotkeys::WindowHotkeys::set_settings_recorder(cx, weak, None, None);
                self.row_error = None;
                self.bind_error = None;
            }
            Err(e) if is_app_capture => self.bind_error = Some(e.message()),
            Err(e) => self.row_error = Some(e.message()),
        }
        cx.notify();
    }
}

impl Focusable for SettingsWindow {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for SettingsWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mac = cfg!(target_os = "macos");
        let save_err = self.polled_save_err.clone().or_else(save_error);
        let page = self.page;
        let windows_err = if page == SettingsPage::Windows {
            self.row_error
                .clone()
                .or_else(|| WindowHotkeys::last_error(cx))
        } else {
            None
        };
        let keyboard_err = if page == SettingsPage::Keyboard {
            self.bind_error.clone()
        } else {
            None
        };
        let color_err = self
            .color_apply_error
            .clone()
            .or_else(|| self.color_import_error.clone());

        div()
            .id("settings-window")
            .size_full()
            .flex()
            .flex_col()
            .bg(SeancePalette::bg())
            .text_color(SeancePalette::text())
            .track_focus(&self.focus_handle)
            .capture_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, window, cx| {
                if settings_close_keystroke(event) {
                    this.close_settings_window(window, cx);
                    cx.stop_propagation();
                    return;
                }
                if this.capture.is_some() || this.windows_capture.is_some() {
                    if event.keystroke.key == "escape" {
                        this.capture = None;
                        this.windows_capture = None;
                        this.bind_error = None;
                        this.row_error = None;
                        let weak = cx.weak_entity();
                        window_hotkeys::WindowHotkeys::set_settings_recorder(cx, weak, None, None);
                        cx.notify();
                        cx.stop_propagation();
                        return;
                    }
                    let chord = preferences::keystroke_to_chord(&event.keystroke);
                    if let Some(wcap) = this.windows_capture.clone() {
                        this.complete_hotkey_capture(wcap, Some(chord), cx);
                    }
                    cx.notify();
                    cx.stop_propagation();
                }
            }))
            .child(
                div()
                    .flex_none()
                    .px_4()
                    .py_3()
                    .border_b_1()
                    .border_color(SeancePalette::border())
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .text_lg()
                            .text_color(SeancePalette::flame())
                            .child("Settings"),
                    )
                    .child(
                        div()
                            .flex()
                            .gap_2()
                            .child(tab_button(
                                "Font",
                                page == SettingsPage::Font,
                                cx,
                                |this, cx| {
                                    this.page = SettingsPage::Font;
                                    this.clear_capture(cx);
                                    cx.notify();
                                },
                            ))
                            .child(tab_button(
                                "Colors",
                                page == SettingsPage::Colors,
                                cx,
                                |this, cx| {
                                    this.page = SettingsPage::Colors;
                                    this.clear_capture(cx);
                                    cx.notify();
                                },
                            ))
                            .child(tab_button(
                                "Keyboard",
                                page == SettingsPage::Keyboard,
                                cx,
                                |this, cx| {
                                    this.page = SettingsPage::Keyboard;
                                    this.clear_capture(cx);
                                    cx.notify();
                                },
                            ))
                            .child(tab_button(
                                "Windows",
                                page == SettingsPage::Windows,
                                cx,
                                |this, cx| {
                                    this.page = SettingsPage::Windows;
                                    this.clear_capture(cx);
                                    cx.notify();
                                },
                            )),
                    ),
            )
            .children({
                let mut banners = Vec::new();
                if let Some(msg) = save_err {
                    banners.push(error_banner(format!("Could not save desktop.json: {msg}")));
                }
                if let Some(msg) = windows_err {
                    banners.push(error_banner(msg));
                }
                if let Some(msg) = keyboard_err {
                    banners.push(error_banner(msg));
                }
                if let Some(msg) = color_err {
                    banners.push(error_banner(msg));
                }
                banners
            })
            .child(
                div()
                    .id("settings-scroll")
                    .flex_1()
                    .overflow_y_scroll()
                    .p_4()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .children(match page {
                        SettingsPage::Font => self.render_font_page(window, cx),
                        SettingsPage::Colors => self.render_colors_page(window, cx),
                        SettingsPage::Keyboard => self.render_keyboard_page(mac, cx),
                        SettingsPage::Windows => self.render_windows_page(mac, cx),
                    }),
            )
    }
}

fn error_banner(msg: String) -> gpui::AnyElement {
    div()
        .flex_none()
        .px_4()
        .py_2()
        .border_b_1()
        .border_color(SeancePalette::border())
        .text_sm()
        .text_color(SeancePalette::danger())
        .child(msg)
        .into_any_element()
}

/// Platform close-window chord (Cmd+W / Ctrl+W) — must win over hotkey capture and inputs.
fn settings_close_keystroke(event: &gpui::KeyDownEvent) -> bool {
    let key = event.keystroke.key.as_str();
    if key != "w" {
        return false;
    }
    if cfg!(target_os = "macos") {
        event.keystroke.modifiers.platform
    } else {
        event.keystroke.modifiers.control
    }
}

fn color_scheme_to_field_hex(scheme: &ColorScheme) -> Vec<String> {
    let mut out = vec![
        scheme.foreground.to_hex(),
        scheme.background.to_hex(),
        scheme.cursor.to_hex(),
        scheme.cursor_text.to_hex(),
        scheme.selection_background.to_hex(),
        scheme.selection_foreground.to_hex(),
        scheme.bold.map(|b| b.to_hex()).unwrap_or_default(),
    ];
    for a in &scheme.ansi {
        out.push(a.to_hex());
    }
    out
}

fn tab_button(
    label: &'static str,
    active: bool,
    cx: &mut Context<SettingsWindow>,
    on_click: impl Fn(&mut SettingsWindow, &mut Context<SettingsWindow>) + 'static,
) -> gpui::AnyElement {
    div()
        .id(SharedString::from(format!("settings-tab-{label}")))
        .px_3()
        .py_1()
        .rounded_md()
        .cursor_pointer()
        .text_sm()
        .bg(if active {
            SeancePalette::surface()
        } else {
            SeancePalette::bg_elevated()
        })
        .text_color(if active {
            SeancePalette::flame()
        } else {
            SeancePalette::text_dim()
        })
        .on_click(cx.listener(move |this, _, _, cx| on_click(this, cx)))
        .child(label)
        .into_any_element()
}

impl SettingsWindow {
    fn close_settings_window(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.clear_capture(cx);
        clear_settings_window();
        window.remove_window();
    }

    fn clear_capture(&mut self, cx: &mut Context<Self>) {
        self.capture = None;
        self.windows_capture = None;
        self.bind_error = None;
        self.row_error = None;
        let weak = cx.weak_entity();
        window_hotkeys::WindowHotkeys::set_settings_recorder(cx, weak, None, None);
    }

    fn render_font_page(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<gpui::AnyElement> {
        let families = self.monospace_fonts.clone();
        let selected = self.draft_family.clone();
        let size = self.draft_size;
        let preview_font = term_font::term_font_for_family(&selected);
        vec![
            div()
                .text_sm()
                .text_color(SeancePalette::text_dim())
                .child("Terminal panes, popouts, ghosts, and thumbnails use this face. App chrome keeps its own size.")
                .into_any_element(),
            div()
                .flex()
                .flex_col()
                .gap_1()
                .child(
                    div()
                        .text_xs()
                        .text_color(SeancePalette::text_faint())
                        .child("Monospace font"),
                )
                .children(families.into_iter().map(|family| {
                    let active = family == selected;
                    let pick = family.clone();
                    div()
                        .id(SharedString::from(format!("font-{family}")))
                        .px_2()
                        .py_1()
                        .rounded_md()
                        .cursor_pointer()
                        .bg(if active {
                            SeancePalette::surface()
                        } else {
                            SeancePalette::bg_elevated()
                        })
                        .text_sm()
                        .text_color(if active {
                            SeancePalette::flame()
                        } else {
                            SeancePalette::text()
                        })
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.draft_family = pick.clone();
                            cx.notify();
                        }))
                        .child(if active {
                            format!("✓ {family}")
                        } else {
                            family
                        })
                        .into_any_element()
                }))
                .into_any_element(),
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(
                    div()
                        .text_sm()
                        .child(format!("Size: {} px", size.round() as i32)),
                )
                .child(size_button("−", cx, |this, cx| {
                    this.draft_size = clamp_font_size(this.draft_size - 1.);
                    cx.notify();
                }))
                .child(size_button("+", cx, |this, cx| {
                    this.draft_size = clamp_font_size(this.draft_size + 1.);
                    cx.notify();
                }))
                .into_any_element(),
            div()
                .p_3()
                .rounded_lg()
                .bg(SeancePalette::bg_elevated())
                .font_family(preview_font.family)
                .font_features(preview_font.features.clone())
                .text_size(px(size))
                .child("Miiiii 00000  │ block ███  λ fn() — the quick brown fox")
                .into_any_element(),
            div()
                .flex()
                .gap_2()
                .child(action_button("Apply / Save", cx, |this, cx| {
                    apply_font_from_settings(
                        this.draft_family.clone(),
                        this.draft_size,
                        &this.installed,
                    );
                    cx.refresh_windows();
                    cx.notify();
                }))
                .child(action_button("Reset defaults", cx, |this, cx| {
                    reset_font_defaults(&this.installed);
                    let prefs = desktop_prefs().read().unwrap().clone();
                    this.draft_family = prefs.font_family;
                    this.draft_size = prefs.font_size;
                    cx.refresh_windows();
                    cx.notify();
                }))
                .into_any_element(),
            div()
                .text_xs()
                .text_color(SeancePalette::text_faint())
                .child(format!(
                    "Quick zoom: {}+ / {}+− / {}+0 (also in Keyboard).",
                    if cfg!(target_os = "macos") { "cmd" } else { "ctrl" },
                    if cfg!(target_os = "macos") { "cmd" } else { "ctrl" },
                    if cfg!(target_os = "macos") { "cmd" } else { "ctrl" },
                ))
                .into_any_element(),
        ]
    }

    fn render_colors_page(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<gpui::AnyElement> {
        let preview_scheme = self.read_color_scheme_from_fields(cx).unwrap_or_default();
        let preview_font = term_font::term_font();
        let preview_size = desktop_prefs().read().unwrap().font_size;
        let mut rows = Vec::new();
        rows.push(
            div()
                .text_sm()
                .text_color(SeancePalette::text_dim())
                .child("Terminal colors for panes, popouts, and overview thumbs. App chrome stays candlelit until you Apply.")
                .into_any_element(),
        );
        if self.color_import_busy {
            rows.push(
                div()
                    .text_sm()
                    .text_color(SeancePalette::text_dim())
                    .child("Loading import…")
                    .into_any_element(),
            );
        }
        for w in &self.color_import_warnings {
            rows.push(
                div()
                    .text_xs()
                    .text_color(SeancePalette::text_faint())
                    .child(w.clone())
                    .into_any_element(),
            );
        }
        if !self.color_import_profiles.is_empty() {
            rows.push(
                div()
                    .text_sm()
                    .text_color(SeancePalette::text_dim())
                    .child("Imported profiles — pick one to load into the draft (not applied until Apply):")
                    .into_any_element(),
            );
            for (i, scheme) in self.color_import_profiles.iter().enumerate() {
                let name = if scheme.name.is_empty() {
                    format!("Profile {}", i + 1)
                } else {
                    scheme.name.clone()
                };
                let active = i == self.color_import_idx;
                let idx = i;
                rows.push(
                    div()
                        .id(SharedString::from(format!("color-import-{i}")))
                        .px_2()
                        .py_1()
                        .rounded_md()
                        .cursor_pointer()
                        .bg(if active {
                            SeancePalette::surface()
                        } else {
                            SeancePalette::bg_elevated()
                        })
                        .text_sm()
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.color_import_idx = idx;
                            if let Some(scheme) = this.color_import_profiles.get(idx).cloned() {
                                this.sync_color_fields_from_scheme(scheme, cx);
                            }
                            this.color_apply_error = None;
                            cx.notify();
                        }))
                        .child(name)
                        .into_any_element(),
                );
            }
        }
        for (label, input) in &self.color_fields {
            let hex = input.read(cx).value().to_string();
            let swatch = colors::Rgb24::from_hex(&hex)
                .ok()
                .map(colors::rgb24_to_hsla)
                .unwrap_or(SeancePalette::surface());
            rows.push(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .w_32()
                            .text_xs()
                            .text_color(SeancePalette::text_faint())
                            .child(label.clone()),
                    )
                    .child(
                        div()
                            .size(px(18.))
                            .rounded_sm()
                            .border_1()
                            .border_color(SeancePalette::border())
                            .bg(swatch),
                    )
                    .child(div().flex_1().child(Input::new(input)))
                    .into_any_element(),
            );
        }
        rows.push(
            div()
                .p_3()
                .rounded_lg()
                .bg(colors::rgb24_to_hsla(preview_scheme.background))
                .text_color(colors::rgb24_to_hsla(preview_scheme.foreground))
                .font_family(preview_font.family)
                .font_features(preview_font.features.clone())
                .text_size(px(preview_size))
                .child("sample: error success λ fn() — the quick brown fox")
                .into_any_element(),
        );
        rows.push(
            div()
                .flex()
                .flex_wrap()
                .gap_2()
                .child(action_button("Apply / Save", cx, move |this, cx| {
                    match this.read_color_scheme_from_fields(cx) {
                        Ok(scheme) => match apply_color_scheme_from_settings(scheme) {
                            Ok(()) => {
                                this.color_apply_error = None;
                                cx.refresh_windows();
                            }
                            Err(e) => this.color_apply_error = Some(e),
                        },
                        Err(e) => this.color_apply_error = Some(e),
                    }
                    cx.notify();
                }))
                .child(action_button("Reset draft", cx, move |this, cx| {
                    let scheme = desktop_prefs()
                        .read()
                        .unwrap()
                        .terminal_color_scheme
                        .clone();
                    this.sync_color_fields_from_scheme(scheme, cx);
                    this.color_apply_error = None;
                    cx.notify();
                }))
                .child(action_button("Restore defaults", cx, move |this, cx| {
                    reset_color_scheme_defaults();
                    let scheme = desktop_prefs()
                        .read()
                        .unwrap()
                        .terminal_color_scheme
                        .clone();
                    this.sync_color_fields_from_scheme(scheme, cx);
                    this.color_apply_error = None;
                    cx.refresh_windows();
                    cx.notify();
                }))
                .child(action_button("Import file…", cx, move |this, cx| {
                    this.start_color_file_import(cx);
                }))
                .child(action_button(
                    "Import installed iTerm2…",
                    cx,
                    move |this, cx| {
                        this.start_installed_iterm_import(cx);
                    },
                ))
                .into_any_element(),
        );
        rows
    }

    fn start_color_file_import(&mut self, cx: &mut Context<Self>) {
        use gpui::PathPromptOptions;
        self.color_import_error = None;
        self.color_import_busy = true;
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Import iTerm2 colors".into()),
        });
        let weak = cx.weak_entity();
        cx.spawn(async move |_, cx| {
            let picked = match rx.await {
                Ok(Ok(Some(paths))) => paths.first().cloned(),
                _ => None,
            };
            let result = picked.map(|path| {
                colors::read_import_file_capped(&path).and_then(|bytes| {
                    colors::parse_iterm(&bytes, &path.to_string_lossy(), &ColorScheme::default())
                })
            });
            if let Some(view) = weak.upgrade() {
                view.update(cx, |this, cx| {
                    this.color_import_busy = false;
                    match result {
                        None => {}
                        Some(Ok(import)) => this.finish_color_import(import),
                        Some(Err(e)) => {
                            this.color_import_error = Some(e);
                            this.color_import_profiles.clear();
                        }
                    }
                    cx.notify();
                });
            }
        })
        .detach();
    }

    fn start_installed_iterm_import(&mut self, cx: &mut Context<Self>) {
        self.color_import_error = None;
        self.color_import_busy = true;
        let weak = cx.weak_entity();
        cx.spawn(async move |_, cx| {
            let import = cx
                .background_executor()
                .spawn(async { colors::load_installed_iterm(&ColorScheme::default()) })
                .await;
            if let Some(view) = weak.upgrade() {
                view.update(cx, |this, cx| {
                    this.color_import_busy = false;
                    match import {
                        Ok(import) => this.finish_color_import(import),
                        Err(e) => {
                            this.color_import_error = Some(e);
                            this.color_import_profiles.clear();
                        }
                    }
                    cx.notify();
                });
            }
        })
        .detach();
    }

    fn finish_color_import(&mut self, import: ImportResult) {
        self.color_import_warnings = import.warnings;
        self.color_import_profiles = import.schemes;
        self.color_import_idx = 0;
        if self.color_import_profiles.is_empty() {
            self.color_import_error = Some("No color profiles found in import.".into());
        }
    }

    fn render_keyboard_page(&self, mac: bool, cx: &mut Context<Self>) -> Vec<gpui::AnyElement> {
        let prefs = desktop_prefs().read().unwrap().clone();
        let mut rows = Vec::new();
        rows.push(
            div()
                .text_sm()
                .text_color(SeancePalette::text_dim())
                .child("Global app chords only — terminal copy/paste and TUI keys are unchanged.")
                .into_any_element(),
        );
        for action in AppAction::all_global() {
            if matches!(action, AppAction::OpenSettings) {
                continue;
            }
            let action_id = preferences::action_key(*action);
            let chords = chords_for_action(&prefs, *action);
            let label = action.label();
            let chord_text = chords
                .iter()
                .map(|c| c.display(mac))
                .collect::<Vec<_>>()
                .join(" · ");
            let capturing = self.capture == Some(*action);
            rows.push(
                div()
                    .id(SharedString::from(format!("settings-shortcut-{action_id}")))
                    .flex()
                    .items_center()
                    .gap_2()
                    .py_1()
                    .border_b_1()
                    .border_color(SeancePalette::border().opacity(0.35))
                    .child(div().flex_1().min_w_0().text_sm().child(label))
                    .child(
                        div()
                            .flex_none()
                            .max_w(px(220.))
                            .overflow_hidden()
                            .text_ellipsis()
                            .text_xs()
                            .text_color(SeancePalette::text_dim())
                            .child(if capturing {
                                "Press chord… (Esc cancel)".into()
                            } else {
                                chord_text
                            }),
                    )
                    .child(shortcut_button(
                        &format!("change-{action_id}"),
                        "Change",
                        cx,
                        move |this, window, cx| {
                            this.start_windows_capture(
                                SettingsCapture::AppShortcut(*action),
                                window,
                                cx,
                            );
                        },
                    ))
                    .child(shortcut_button(
                        &format!("reset-{action_id}"),
                        "Reset",
                        cx,
                        move |this, _window, cx| {
                            match reset_action(*action) {
                                Ok(()) => this.bind_error = None,
                                Err(BindError::Conflict(other)) => {
                                    this.bind_error = Some(format!(
                                        "Cannot reset: already used by “{}”.",
                                        other.label()
                                    ));
                                }
                                Err(BindError::Protected) => {
                                    this.bind_error = Some("That chord is reserved.".into());
                                }
                                Err(BindError::UnsafeBareKey) => {
                                    this.bind_error = Some(
                                        "Use a modifier chord — bare keys belong in terminals."
                                            .into(),
                                    );
                                }
                                Err(BindError::ConflictWindowHotkey) => {
                                    this.bind_error =
                                        Some("Already used by a window show/hide hotkey.".into());
                                }
                            }
                            cx.notify();
                        },
                    ))
                    .into_any_element(),
            );
        }
        rows.push(
            div()
                .pt_2()
                .child(action_button(
                    "Restore all keyboard defaults",
                    cx,
                    |this, cx| {
                        match reset_all_shortcuts() {
                            Ok(()) => this.bind_error = None,
                            Err(BindError::Conflict(other)) => {
                                this.bind_error = Some(format!(
                                    "Cannot reset all: “{}” still uses a default chord.",
                                    other.label()
                                ));
                            }
                            Err(BindError::ConflictWindowHotkey) => {
                                this.bind_error = Some(
                                    "Cannot reset all: a window show/hide hotkey uses a default chord."
                                        .into(),
                                );
                            }
                            Err(BindError::Protected) => {
                                this.bind_error = Some("That chord is reserved.".into());
                            }
                            Err(BindError::UnsafeBareKey) => {
                                this.bind_error = Some(
                                    "Use a modifier chord — bare keys belong in terminals.".into(),
                                );
                            }
                        }
                        cx.notify();
                    },
                ))
                .into_any_element(),
        );
        rows
    }

    fn render_windows_page(&mut self, mac: bool, cx: &mut Context<Self>) -> Vec<gpui::AnyElement> {
        let supported = window_hotkeys::WindowHotkeys::platform_supported();
        let prefs = desktop_prefs().read().unwrap().clone();
        let catalog = WindowHotkeys::catalog(cx);
        let mut rows = Vec::new();
        if !supported {
            rows.push(
                div()
                    .text_sm()
                    .text_color(SeancePalette::text_dim())
                    .child("Global window hotkeys are only supported on macOS. Definitions are saved but not registered on this platform.")
                    .into_any_element(),
            );
        }
        let main_label = prefs
            .main_window_hotkey
            .as_ref()
            .map(|c| c.display(mac))
            .unwrap_or_else(|| "Unassigned".into());
        let main_cap = self.windows_capture == Some(SettingsCapture::MainWindow);
        rows.push(
            div()
                .text_sm()
                .text_color(SeancePalette::text_dim())
                .child("System-wide show/hide for the primary Seance window.")
                .into_any_element(),
        );
        rows.push(window_row(
            "Main window",
            &main_label,
            main_cap,
            cx,
            |this, window, cx| {
                this.start_windows_capture(SettingsCapture::MainWindow, window, cx);
            },
            |this, cx| {
                let _ = WindowHotkeys::bind_main(cx, None);
                this.row_error = None;
                cx.notify();
            },
            |_this, cx| {
                WindowHotkeys::open_or_toggle_main(cx);
                cx.notify();
            },
        ));
        rows.push(
            div()
                .pt_2()
                .text_sm()
                .text_color(SeancePalette::text_dim())
                .child("Dedicated workspace windows — each stays on one circle.")
                .into_any_element(),
        );
        let pick = if self.pick_workspace.is_empty() {
            catalog.first().map(|(s, _)| s.clone()).unwrap_or_default()
        } else {
            self.pick_workspace.clone()
        };
        rows.push(
            div()
                .flex()
                .flex_col()
                .gap_1()
                .child(div().text_sm().child("Circle for new window:"))
                .children(catalog.iter().map(|(slug, label)| {
                    let active = pick == *slug;
                    let pick_slug = slug.clone();
                    div()
                        .id(SharedString::from(format!("ws-pick-{slug}")))
                        .px_2()
                        .py_1()
                        .rounded_md()
                        .cursor_pointer()
                        .bg(if active {
                            SeancePalette::surface()
                        } else {
                            SeancePalette::bg_elevated()
                        })
                        .text_sm()
                        .text_color(if active {
                            SeancePalette::flame()
                        } else {
                            SeancePalette::text()
                        })
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.pick_workspace = pick_slug.clone();
                            this.row_error = None;
                            cx.notify();
                        }))
                        .child(if active {
                            format!("✓ {label}")
                        } else {
                            label.clone()
                        })
                        .into_any_element()
                }))
                .child(action_button("Add window", cx, |this, cx| {
                    let pick = if this.pick_workspace.is_empty() {
                        WindowHotkeys::catalog(cx)
                            .first()
                            .map(|(s, _)| s.clone())
                            .unwrap_or_default()
                    } else {
                        this.pick_workspace.clone()
                    };
                    if pick.is_empty() {
                        this.row_error = Some("Choose a workspace first.".into());
                    } else if let Err(e) = WindowHotkeys::add_workspace_definition(cx, pick.clone())
                    {
                        this.row_error = Some(e);
                    } else {
                        this.row_error = None;
                        window_hotkeys::open_workspace_window(cx, pick.clone());
                    }
                    cx.notify();
                }))
                .into_any_element(),
        );
        for (i, def) in prefs.workspace_windows.iter().enumerate() {
            let label = catalog
                .iter()
                .find(|(s, _)| s == &def.workspace)
                .map(|(_, l)| l.as_str())
                .unwrap_or(def.workspace.as_str());
            let chord = def
                .shortcut
                .as_ref()
                .map(|c| c.display(mac))
                .unwrap_or_else(|| "Unassigned".into());
            let cap = self.windows_capture == Some(SettingsCapture::WorkspaceRow(i));
            let idx = i;
            rows.push(window_row(
                label,
                &chord,
                cap,
                cx,
                move |this, window, cx| {
                    this.start_windows_capture(SettingsCapture::WorkspaceRow(idx), window, cx);
                },
                move |this, cx| {
                    let _ = WindowHotkeys::bind_workspace(cx, idx, None);
                    this.row_error = None;
                    cx.notify();
                },
                move |_this, cx| {
                    WindowHotkeys::open_or_toggle_workspace(cx, idx);
                    cx.notify();
                },
            ));
            rows.push(
                div()
                    .pb_2()
                    .child(action_button("Remove window", cx, move |this, cx| {
                        this.clear_capture(cx);
                        WindowHotkeys::remove_workspace_definition(cx, idx);
                        this.row_error = None;
                        cx.notify();
                    }))
                    .into_any_element(),
            );
        }
        rows
    }

    fn start_windows_capture(
        &mut self,
        capture: SettingsCapture,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.capture = match &capture {
            SettingsCapture::AppShortcut(action) => Some(*action),
            _ => None,
        };
        let cap = capture.clone();
        self.windows_capture = Some(capture);
        self.row_error = None;
        self.bind_error = None;
        let weak = cx.weak_entity();
        window_hotkeys::WindowHotkeys::set_settings_recorder(cx, weak, Some(cap), None);
        window.focus(&self.focus_handle, cx);
        cx.notify();
    }
}

fn window_row(
    label: &str,
    chord: &str,
    capturing: bool,
    cx: &mut Context<SettingsWindow>,
    on_bind: impl Fn(&mut SettingsWindow, &mut Window, &mut Context<SettingsWindow>) + 'static,
    on_clear: impl Fn(&mut SettingsWindow, &mut Context<SettingsWindow>) + 'static,
    on_toggle: impl Fn(&mut SettingsWindow, &mut Context<SettingsWindow>) + 'static,
) -> gpui::AnyElement {
    let label = label.to_string();
    let chord = chord.to_string();
    div()
        .flex()
        .flex_col()
        .gap_1()
        .py_1()
        .border_b_1()
        .border_color(SeancePalette::border().opacity(0.35))
        .child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(
                    div()
                        .flex_1()
                        .text_sm()
                        .overflow_hidden()
                        .text_ellipsis()
                        .child(label),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(SeancePalette::text_dim())
                        .child(if capturing {
                            "Press chord… (Esc cancel)".into()
                        } else {
                            chord
                        }),
                ),
        )
        .child(
            div()
                .flex()
                .gap_2()
                .child(shortcut_button(
                    "bind",
                    "Bind",
                    cx,
                    move |this, window, cx| {
                        on_bind(this, window, cx);
                    },
                ))
                .child(shortcut_button("clear", "Clear", cx, move |this, _, cx| {
                    on_clear(this, cx);
                }))
                .child(shortcut_button(
                    "toggle",
                    "Open / hide",
                    cx,
                    move |this, _, cx| {
                        on_toggle(this, cx);
                    },
                )),
        )
        .into_any_element()
}

fn size_button(
    label: &'static str,
    cx: &mut Context<SettingsWindow>,
    on_click: impl Fn(&mut SettingsWindow, &mut Context<SettingsWindow>) + 'static,
) -> gpui::AnyElement {
    div()
        .id(SharedString::from(format!("settings-size-{label}")))
        .px_2()
        .py_1()
        .rounded_md()
        .cursor_pointer()
        .bg(SeancePalette::surface())
        .on_click(cx.listener(move |this, _, _, cx| on_click(this, cx)))
        .child(label)
        .into_any_element()
}

fn action_button(
    label: &'static str,
    cx: &mut Context<SettingsWindow>,
    on_click: impl Fn(&mut SettingsWindow, &mut Context<SettingsWindow>) + 'static,
) -> gpui::AnyElement {
    div()
        .id(SharedString::from(format!("settings-btn-{label}")))
        .px_3()
        .py_1p5()
        .rounded_md()
        .cursor_pointer()
        .text_sm()
        .text_color(SeancePalette::flame())
        .bg(SeancePalette::surface())
        .hover(|s| s.bg(SeancePalette::border()))
        .on_click(cx.listener(move |this, _, _, cx| on_click(this, cx)))
        .child(label)
        .into_any_element()
}

fn shortcut_button(
    id: &str,
    label: &'static str,
    cx: &mut Context<SettingsWindow>,
    on_click: impl Fn(&mut SettingsWindow, &mut Window, &mut Context<SettingsWindow>) + 'static,
) -> gpui::AnyElement {
    let id = id.to_string();
    div()
        .id(SharedString::from(format!("settings-btn-{id}")))
        .px_3()
        .py_1p5()
        .rounded_md()
        .cursor_pointer()
        .text_sm()
        .text_color(SeancePalette::flame())
        .bg(SeancePalette::surface())
        .hover(|s| s.bg(SeancePalette::border()))
        .on_click(cx.listener(move |this, _, window, cx| on_click(this, window, cx)))
        .child(label)
        .into_any_element()
}

impl SeanceApp {
    pub(super) fn open_settings_window(&mut self, cx: &mut Context<Self>) {
        if focus_existing_settings(cx) {
            return;
        }
        let bounds = gpui::Bounds::centered(None, gpui::size(px(720.), px(640.)), cx);
        let win = cx
            .open_window(
                gpui::WindowOptions {
                    window_bounds: Some(gpui::WindowBounds::Windowed(bounds)),
                    titlebar: Some(gpui::TitlebarOptions {
                        title: Some("seance — settings".into()),
                        ..Default::default()
                    }),
                    app_id: Some("seance".into()),
                    ..Default::default()
                },
                |window, cx| {
                    register_settings_window(window.window_handle());
                    let settings = cx.new(|cx| SettingsWindow::new(window, cx));
                    let weak = settings.downgrade();
                    window.on_window_should_close(cx, move |_, cx| {
                        clear_settings_window();
                        window_hotkeys::WindowHotkeys::set_settings_recorder(
                            cx,
                            weak.clone(),
                            None,
                            None,
                        );
                        true
                    });
                    window.focus(&settings.read(cx).focus_handle(cx), cx);
                    cx.new(|cx| Root::new(settings, window, cx))
                },
            )
            .expect("settings window");
        let _ = win;
    }

    pub(crate) fn dispatch_app_action(
        &mut self,
        action: AppAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        use AppAction::*;
        match action {
            NewSession => self.new_default_session(cx),
            KillPaneOrWorkspace => {
                if let Some(slug) = self.active_slug.clone() {
                    let ws = self
                        .panes
                        .iter()
                        .find(|p| p.slug == slug)
                        .map(|p| p.workspace.clone());
                    let last_in_ws = ws.as_ref().is_some_and(|w| {
                        self.panes.iter().filter(|p| p.workspace == *w).count() == 1
                    });
                    if last_in_ws {
                        if let Some(w) = ws {
                            self.kill_workspace(&w, window, cx);
                        }
                    } else {
                        self.kill_active_pane(cx);
                    }
                } else if let Some(ws) = self.selected_workspace.clone() {
                    if !self.panes.iter().any(|p| p.workspace == ws) {
                        self.kill_workspace(&ws, window, cx);
                    }
                }
            }
            ToggleNotes => self.toggle_notes_flip(window, cx),
            PinWorkspace => {
                if let Some(ws) = self.selected_workspace.clone() {
                    self.toggle_pin_workspace(&ws);
                    cx.notify();
                }
            }
            Popout => {
                if let Some(slug) = self.active_slug.clone() {
                    self.toggle_popout(&slug, cx);
                }
            }
            SelectTopWorkspace => self.select_top_workspace(window, cx),
            ToggleOverview => self.set_overview(!self.overview, cx),
            NavigatePaneUp => self.navigate_pane_directional("up", window, cx),
            NavigatePaneDown => self.navigate_pane_directional("down", window, cx),
            NavigatePaneLeft => self.navigate_pane_directional("left", window, cx),
            NavigatePaneRight => self.navigate_pane_directional("right", window, cx),
            CycleWorkspacePrev => self.cycle_workspace(-1, window, cx),
            CycleWorkspaceNext => self.cycle_workspace(1, window, cx),
            CyclePanePrev => self.cycle_pane(-1, window, cx),
            CyclePaneNext => self.cycle_pane(1, window, cx),
            PalettePrompts => {
                self.palette = super::PaletteMode::Prompts {
                    query: String::new(),
                    selected: 0,
                };
                let fh = self.focus_handle.clone();
                window.focus(&fh, cx);
                cx.notify();
            }
            PaletteJump => {
                self.palette = super::PaletteMode::Jump {
                    query: String::new(),
                    selected: 0,
                };
                let fh = self.focus_handle.clone();
                window.focus(&fh, cx);
                cx.notify();
            }
            ZoomPane => {
                if let Some(slug) = self.active_slug.clone() {
                    self.toggle_zoom(&slug, cx);
                }
            }
            RenameWorkspace => {
                if let Some(ws) = self.selected_workspace.clone() {
                    let label = self.workspace_label(&ws);
                    self.start_rename(
                        super::RenameTarget::Workspace(ws.clone()),
                        &label,
                        window,
                        cx,
                    );
                }
            }
            ShowLastFailed => {
                if let Some(slug) = self.active_slug.clone() {
                    self.show_last_failed(&slug, cx);
                }
            }
            RailWorkspace1 | RailWorkspace2 | RailWorkspace3 | RailWorkspace4 | RailWorkspace5
            | RailWorkspace6 | RailWorkspace7 | RailWorkspace8 | RailWorkspace9 => {
                if let Some(idx) = preferences::rail_index_for_action(action) {
                    self.select_nth_workspace(idx, window, cx);
                }
            }
            OpenSettings => self.open_settings_window(cx),
            TermZoomIn => {
                adjust_font_size(1);
                cx.refresh_windows();
            }
            TermZoomOut => {
                adjust_font_size(-1);
                cx.refresh_windows();
            }
            TermZoomReset => {
                reset_terminal_font_size();
                cx.refresh_windows();
            }
        }
    }
}
