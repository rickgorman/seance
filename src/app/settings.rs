//! Desktop settings window — font + global shortcuts (device-local prefs).

use std::collections::HashSet;
use std::sync::Mutex;

use crate::theme::SeancePalette;
use gpui::{
    div, prelude::*, px, AnyWindowHandle, App, Context, FocusHandle, Focusable, Render,
    SharedString, Window,
};

use super::preferences::{
    self, adjust_font_size, apply_font_from_settings, chords_for_action, clamp_font_size,
    desktop_prefs, reset_action, reset_all_shortcuts, reset_font_defaults,
    reset_terminal_font_size, save_error, try_bind, AppAction, BindError,
};
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
    Keyboard,
}

/// Separate window so capture never leaks into live PTYs.
pub struct SettingsWindow {
    page: SettingsPage,
    focus_handle: FocusHandle,
    draft_family: String,
    draft_size: f32,
    monospace_fonts: Vec<String>,
    capture: Option<AppAction>,
    bind_error: Option<String>,
    polled_save_err: Option<String>,
    installed: HashSet<String>,
}

impl SettingsWindow {
    pub fn new(_window: &mut Window, cx: &mut Context<Self>) -> Self {
        let installed: HashSet<String> = cx.text_system().all_font_names().into_iter().collect();
        let prefs = desktop_prefs().read().unwrap().clone();
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
            bind_error: None,
            polled_save_err,
            installed,
        }
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

        div()
            .id("settings-window")
            .size_full()
            .flex()
            .flex_col()
            .bg(SeancePalette::bg())
            .text_color(SeancePalette::text())
            .track_focus(&self.focus_handle)
            .capture_key_down(
                cx.listener(|this, event: &gpui::KeyDownEvent, _window, cx| {
                    if this.capture.is_some() {
                        if event.keystroke.key == "escape" {
                            this.capture = None;
                            this.bind_error = None;
                            cx.notify();
                            cx.stop_propagation();
                            return;
                        }
                        let action = this.capture;
                        let chord = preferences::keystroke_to_chord(&event.keystroke);
                        if let Some(action) = action {
                            match try_bind(action, vec![chord], cfg!(target_os = "macos")) {
                                Ok(()) => {
                                    this.capture = None;
                                    this.bind_error = None;
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
                                Err(BindError::Conflict(other)) => {
                                    this.bind_error =
                                        Some(format!("Already used by “{}”.", other.label()));
                                }
                            }
                        }
                        cx.notify();
                        cx.stop_propagation();
                    }
                }),
            )
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
                                    cx.notify();
                                },
                            ))
                            .child(tab_button(
                                "Keyboard",
                                page == SettingsPage::Keyboard,
                                cx,
                                |this, cx| {
                                    this.page = SettingsPage::Keyboard;
                                    cx.notify();
                                },
                            )),
                    ),
            )
            .child(
                div()
                    .id("settings-scroll")
                    .flex_1()
                    .overflow_y_scroll()
                    .p_4()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .children({
                        let mut top = Vec::new();
                        if let Some(msg) = save_err {
                            top.push(
                                div()
                                    .text_sm()
                                    .text_color(SeancePalette::danger())
                                    .child(format!("Could not save desktop.json: {msg}"))
                                    .into_any_element(),
                            );
                        }
                        if let Some(msg) = self.bind_error.clone() {
                            top.push(
                                div()
                                    .text_sm()
                                    .text_color(SeancePalette::danger())
                                    .child(msg)
                                    .into_any_element(),
                            );
                        }
                        top
                    })
                    .children(match page {
                        SettingsPage::Font => self.render_font_page(window, cx),
                        SettingsPage::Keyboard => self.render_keyboard_page(mac, cx),
                    }),
            )
    }
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
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.draft_family = pick.clone();
                            cx.notify();
                        }))
                        .child(family)
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
                    .child(div().flex_1().text_sm().child(label))
                    .child(div().text_xs().text_color(SeancePalette::text_dim()).child(
                        if capturing {
                            "Press chord… (Esc cancel)".into()
                        } else {
                            chord_text
                        },
                    ))
                    .child(shortcut_button(
                        &format!("change-{action_id}"),
                        "Change",
                        cx,
                        move |this, window, cx| {
                            this.capture = Some(*action);
                            this.bind_error = None;
                            window.focus(&this.focus_handle, cx);
                            cx.notify();
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
                    |_, cx| {
                        reset_all_shortcuts();
                        cx.notify();
                    },
                ))
                .into_any_element(),
        );
        rows
    }
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
                    window.on_window_should_close(cx, |_, _| {
                        clear_settings_window();
                        true
                    });
                    let settings = cx.new(|cx| SettingsWindow::new(window, cx));
                    window.focus(&settings.read(cx).focus_handle(cx), cx);
                    settings
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
