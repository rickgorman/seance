//! Native show/hide for main + named-Seance windows, including Overlay.
//!
//! Overlay (macOS): the window floats (`NSFloatingWindowLevel`) and follows
//! the user to whichever Space is current (`MoveToActiveSpace`, plus
//! `FullScreenAuxiliary` so it can sit over a fullscreen app). Showing moves
//! it onto the monitor under the pointer and orders it front BEFORE the app
//! activates, so activation finds the key window on the active Space and has
//! no reason to switch Spaces. Hiding hands focus back to the app that was
//! frontmost when it was shown — remembered by pid and looked up again at
//! hide time, never a retained pointer. When Seance as a whole loses
//! activation, visible overlays are ordered out without touching the app the
//! user went to. No Accessibility, event taps or AppleScript.
//!
//! The level and collection behavior a window had before Overlay touched it
//! are captured and restored verbatim, so GPUI's own flags survive a toggle.

/// A queued app-resign is stale — and must not hide anything — when an
/// overlay was shown after it was posted, or Seance is active again.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn resign_is_stale(posted_gen: u64, current_gen: u64, app_active: bool) -> bool {
    app_active || posted_gen != current_gen
}

#[cfg(not(target_os = "macos"))]
use gpui::{App, Window};

/// A window's level + collection behavior before Overlay changed them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NormalBehavior {
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    level: isize,
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    collection: usize,
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    hides_on_deactivate: bool,
}

pub enum ToggleAction {
    Hide,
    Show,
    GpuiFallback,
}

#[cfg(target_os = "macos")]
pub use mac::{
    configure, configure_companion, frontmost_other_app, hide, hide_quietly,
    install_resign_observer, pointer_display_id, raise_companion, show, toggle_action,
};

/// Logical window frame + the visible rect it should stay inside (AppKit points).
#[cfg(any(target_os = "macos", test))]
#[derive(Clone, Copy, Debug, PartialEq)]
struct Frame2d {
    x: f64,
    y: f64,
    w: f64,
    h: f64,
}

/// When an `NSWindow` jumps screens, keep its offset within the old visible
/// frame but clamp inside the new screen's visible frame (same math as
/// [`mac::move_to_pointer_screen`], pure for tests).
#[cfg(any(target_os = "macos", test))]
fn relocate_preserving_visible_offset(
    frame: Frame2d,
    from_vis: Frame2d,
    to_vis: Frame2d,
) -> Frame2d {
    let (dx, dy) = (frame.x - from_vis.x, frame.y - from_vis.y);
    let w = frame.w.min(to_vis.w);
    let h = frame.h.min(to_vis.h);
    let max_x = to_vis.x + to_vis.w - w;
    let max_y = to_vis.y + to_vis.h - h;
    Frame2d {
        x: (to_vis.x + dx).clamp(to_vis.x, max_x),
        y: (to_vis.y + dy).clamp(to_vis.y, max_y),
        w,
        h,
    }
}

#[cfg(target_os = "macos")]
mod mac {
    #![allow(unexpected_cfgs)]
    use super::{relocate_preserving_visible_offset, Frame2d, NormalBehavior, ToggleAction};
    use cocoa::appkit::NSApp;
    use cocoa::base::{id, nil, BOOL, NO, YES};
    use cocoa::foundation::{NSPoint, NSRect, NSString};
    use gpui::{App, Window};
    use objc::declare::ClassDecl;
    use objc::runtime::{Object, Sel};
    use objc::{class, msg_send, sel, sel_impl};
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::OnceLock;

    // AppKit constants (declared locally, as gpui_macos does).
    const NS_FLOATING_WINDOW_LEVEL: isize = 3;
    const CAN_JOIN_ALL_SPACES: usize = 1 << 0;
    const MOVE_TO_ACTIVE_SPACE: usize = 1 << 1;
    const FULL_SCREEN_PRIMARY: usize = 1 << 7;
    const FULL_SCREEN_AUXILIARY: usize = 1 << 8;

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

    /// Overlay flips collection behavior away from anything that would
    /// fight `MoveToActiveSpace` (JoinAllSpaces) or `FullScreenAuxiliary`
    /// (FullScreenPrimary — the two are mutually exclusive).
    pub(super) fn overlay_collection(normal: usize) -> usize {
        (normal & !(CAN_JOIN_ALL_SPACES | FULL_SCREEN_PRIMARY))
            | MOVE_TO_ACTIVE_SPACE
            | FULL_SCREEN_AUXILIARY
    }

    /// Settings beside overlays: present on EVERY Space (so wherever an
    /// overlay was summoned, including a fullscreen app's Space, Settings is
    /// already there — no per-show Space move to race) and allowed next to
    /// fullscreen windows. MoveToActiveSpace is cleared: AppKit forbids it
    /// together with CanJoinAllSpaces.
    pub(super) fn companion_collection(normal: usize) -> usize {
        (normal & !(MOVE_TO_ACTIVE_SPACE | FULL_SCREEN_PRIMARY))
            | CAN_JOIN_ALL_SPACES
            | FULL_SCREEN_AUXILIARY
    }

    pub fn configure(window: &Window, on: bool, normal: &mut Option<NormalBehavior>) {
        let Some(nswindow) = ns_window(window) else {
            return;
        };
        #[allow(deprecated)]
        unsafe {
            if on {
                let base = match normal {
                    Some(n) => *n,
                    None => {
                        let level: isize = msg_send![nswindow, level];
                        let collection: usize = msg_send![nswindow, collectionBehavior];
                        let hides: BOOL = msg_send![nswindow, hidesOnDeactivate];
                        let n = NormalBehavior {
                            level,
                            collection,
                            hides_on_deactivate: hides != NO,
                        };
                        *normal = Some(n);
                        n
                    }
                };
                let _: () =
                    msg_send![nswindow, setCollectionBehavior: overlay_collection(base.collection)];
                let _: () = msg_send![nswindow, setLevel: NS_FLOATING_WINDOW_LEVEL];
            } else if let Some(n) = normal.take() {
                let _: () = msg_send![nswindow, setLevel: n.level];
                let _: () = msg_send![nswindow, setCollectionBehavior: n.collection];
            }
        }
    }

    pub fn toggle_action(window: &Window, overlay: bool) -> ToggleAction {
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
            // An overlay parked on another Space counts as hidden: pressing
            // the hotkey brings it here instead of hiding it over there.
            let here: bool = !overlay || msg_send![nswindow, isOnActiveSpace];
            if visible && !mini && key && app_active && here {
                ToggleAction::Hide
            } else {
                ToggleAction::Show
            }
        }
    }

    /// The app in front right now, unless it's us — by pid, looked up
    /// again at hide time. Call BEFORE anything that could activate Seance
    /// (deminiaturize, opening a focused window).
    pub fn frontmost_other_app() -> Option<i32> {
        #[allow(deprecated)]
        unsafe {
            frontmost_other_pid()
        }
    }

    pub fn app_is_active() -> bool {
        #[allow(deprecated)]
        unsafe {
            let app: id = NSApp();
            let active: BOOL = msg_send![app, isActive];
            active != NO
        }
    }

    /// Show the window. Overlay: onto the pointer's screen, front + key on
    /// the CURRENT Space first, then `cx.activate(true)` (`activateIgnoringOtherApps`)
    /// — so activation has no reason to switch Spaces and does not raise every
    /// Seance window (unlike `ActivateAllWindows`).
    pub fn show(window: &mut Window, overlay: bool, cx: &mut App) {
        let Some(nswindow) = ns_window(window) else {
            window.activate_window();
            return;
        };
        #[allow(deprecated)]
        unsafe {
            let mini: bool = msg_send![nswindow, isMiniaturized];
            if mini {
                let _: () = msg_send![nswindow, deminiaturize: nil];
            }
            if !overlay {
                let _: () = msg_send![nswindow, makeKeyAndOrderFront: nil];
                window.activate_window();
                return;
            }
            SHOW_GEN.fetch_add(1, Ordering::SeqCst);
            move_to_pointer_screen(nswindow);
            let _: () = msg_send![nswindow, orderFrontRegardless];
            let _: () = msg_send![nswindow, makeKeyWindow];
        }
        cx.activate(true);
        // Native `setFrame` across displays does not always run GPUI's
        // `windowDidChangeScreen` / backing-property path before first paint
        // (notably hidden-first overlay windows). Resync scale + viewport.
        window.bounds_changed(cx);
    }

    /// Settings while any Overlay is enabled: one level above the overlays,
    /// on all Spaces ([`companion_collection`]), and `hidesOnDeactivate` so
    /// it never floats over another app once Seance loses activation. Off
    /// restores level, collection behavior and hidesOnDeactivate verbatim.
    pub fn configure_companion(window: &Window, on: bool, normal: &mut Option<NormalBehavior>) {
        let Some(nswindow) = ns_window(window) else {
            return;
        };
        #[allow(deprecated)]
        unsafe {
            if on {
                let base = match normal {
                    Some(n) => *n,
                    None => {
                        let level: isize = msg_send![nswindow, level];
                        let collection: usize = msg_send![nswindow, collectionBehavior];
                        let hides: BOOL = msg_send![nswindow, hidesOnDeactivate];
                        let n = NormalBehavior {
                            level,
                            collection,
                            hides_on_deactivate: hides != NO,
                        };
                        *normal = Some(n);
                        n
                    }
                };
                let _: () = msg_send![
                    nswindow,
                    setCollectionBehavior: companion_collection(base.collection)
                ];
                let _: () = msg_send![nswindow, setLevel: NS_FLOATING_WINDOW_LEVEL + 1];
                let _: () = msg_send![nswindow, setHidesOnDeactivate: YES];
            } else if let Some(n) = normal.take() {
                let _: () = msg_send![nswindow, setLevel: n.level];
                let _: () = msg_send![nswindow, setCollectionBehavior: n.collection];
                let flag: BOOL = if n.hides_on_deactivate { YES } else { NO };
                let _: () = msg_send![nswindow, setHidesOnDeactivate: flag];
            }
        }
    }

    /// Show Settings above the overlays in the overlay order: pointer's
    /// screen, front + key on the current Space, then `cx.activate(true)` —
    /// so activation never goes looking for a Space Settings was left on.
    pub fn raise_companion(window: &mut Window, cx: &mut App) {
        let Some(nswindow) = ns_window(window) else {
            return;
        };
        #[allow(deprecated)]
        unsafe {
            let mini: bool = msg_send![nswindow, isMiniaturized];
            if mini {
                let _: () = msg_send![nswindow, deminiaturize: nil];
            }
            move_to_pointer_screen(nswindow);
            let _: () = msg_send![nswindow, orderFrontRegardless];
            let _: () = msg_send![nswindow, makeKeyWindow];
        }
        cx.activate(true);
        window.bounds_changed(cx);
    }

    /// Display under the pointer — for `WindowOptions::display_id` at creation.
    pub fn pointer_display_id(cx: &App) -> Option<gpui::DisplayId> {
        let screen = screen_under_pointer()?;
        display_id_for_screen(screen)
            .map(gpui::DisplayId::from)
            .or_else(|| cx.primary_display().map(|d| d.id()))
    }

    fn screen_under_pointer() -> Option<id> {
        #[allow(deprecated)]
        unsafe {
            let mouse: NSPoint = msg_send![class!(NSEvent), mouseLocation];
            let screens: id = msg_send![class!(NSScreen), screens];
            let count: usize = msg_send![screens, count];
            for i in 0..count {
                let screen: id = msg_send![screens, objectAtIndex: i];
                let frame: NSRect = msg_send![screen, frame];
                if mouse.x >= frame.origin.x
                    && mouse.x < frame.origin.x + frame.size.width
                    && mouse.y >= frame.origin.y
                    && mouse.y < frame.origin.y + frame.size.height
                {
                    return Some(screen);
                }
            }
            None
        }
    }

    fn display_id_for_screen(screen: id) -> Option<u64> {
        if screen == nil {
            return None;
        }
        #[allow(deprecated)]
        unsafe {
            let device_description: id = msg_send![screen, deviceDescription];
            let key = NSString::alloc(nil).init_str("NSScreenNumber");
            let screen_number: id = msg_send![device_description, objectForKey: key];
            let _: () = msg_send![key, release];
            if screen_number == nil {
                return None;
            }
            let n: usize = msg_send![screen_number, unsignedIntegerValue];
            Some(n as u64)
        }
    }

    /// Hide via the hotkey. `prior` (overlay only) gets focus back first, so
    /// ordering our window out doesn't promote another Seance window.
    pub fn hide(window: &Window, prior: Option<i32>) {
        let Some(nswindow) = ns_window(window) else {
            return;
        };
        #[allow(deprecated)]
        unsafe {
            if let Some(pid) = prior {
                let app: id = msg_send![
                    class!(NSRunningApplication),
                    runningApplicationWithProcessIdentifier: pid
                ];
                if app != nil {
                    let terminated: bool = msg_send![app, isTerminated];
                    if !terminated {
                        let _: BOOL = msg_send![app, activateWithOptions: 0usize];
                    }
                }
            }
            let _: () = msg_send![nswindow, orderOut: nil];
        }
    }

    /// Order out without activating anything (the app already lost focus).
    pub fn hide_quietly(window: &Window) {
        let Some(nswindow) = ns_window(window) else {
            return;
        };
        #[allow(deprecated)]
        unsafe {
            let visible: bool = msg_send![nswindow, isVisible];
            if visible {
                let _: () = msg_send![nswindow, orderOut: nil];
            }
        }
    }

    unsafe fn frontmost_other_pid() -> Option<i32> {
        let workspace: id = msg_send![class!(NSWorkspace), sharedWorkspace];
        let front: id = msg_send![workspace, frontmostApplication];
        if front == nil {
            return None;
        }
        let pid: i32 = msg_send![front, processIdentifier];
        (pid > 0 && pid as u32 != std::process::id()).then_some(pid)
    }

    /// Keep the window's offset within its screen, but on the screen the
    /// pointer is on. Clamped so it stays fully inside the visible frame.
    unsafe fn move_to_pointer_screen(nswindow: id) {
        let target = screen_under_pointer().unwrap_or(nil);
        let current: id = msg_send![nswindow, screen];
        if target == nil || target == current {
            return;
        }
        let to: NSRect = msg_send![target, visibleFrame];
        let frame: NSRect = msg_send![nswindow, frame];
        let from_vis: NSRect = if current != nil {
            msg_send![current, visibleFrame]
        } else {
            to
        };
        let moved = relocate_preserving_visible_offset(
            Frame2d {
                x: frame.origin.x,
                y: frame.origin.y,
                w: frame.size.width,
                h: frame.size.height,
            },
            Frame2d {
                x: from_vis.origin.x,
                y: from_vis.origin.y,
                w: from_vis.size.width,
                h: from_vis.size.height,
            },
            Frame2d {
                x: to.origin.x,
                y: to.origin.y,
                w: to.size.width,
                h: to.size.height,
            },
        );
        let new_frame = NSRect::new(
            NSPoint::new(moved.x, moved.y),
            cocoa::foundation::NSSize::new(moved.w, moved.h),
        );
        // `display: YES` so AppKit reconciles backing scale with the new screen.
        let _: () = msg_send![nswindow, setFrame: new_frame display: YES];
    }

    static RESIGN_TX: OnceLock<futures::channel::mpsc::UnboundedSender<u64>> = OnceLock::new();
    /// Bumped by every overlay show; a resign posted under an older value
    /// predates that show.
    static SHOW_GEN: AtomicU64 = AtomicU64::new(0);

    extern "C" fn app_did_resign(_this: &Object, _sel: Sel, _note: id) {
        if let Some(tx) = RESIGN_TX.get() {
            let _ = tx.unbounded_send(SHOW_GEN.load(Ordering::SeqCst));
        }
    }

    /// Listen for `NSApplicationDidResignActiveNotification` — the app-level
    /// signal, so moving focus between Seance windows (or Settings) never
    /// counts as leaving.
    pub fn install_resign_observer(cx: &mut App) {
        if RESIGN_TX.get().is_some() {
            return;
        }
        let (tx, mut rx) = futures::channel::mpsc::unbounded::<u64>();
        if RESIGN_TX.set(tx).is_err() {
            return;
        }
        let Some(mut decl) = ClassDecl::new("SeanceOverlayResignObserver", class!(NSObject)) else {
            return;
        };
        #[allow(deprecated)]
        unsafe {
            decl.add_method(
                sel!(seanceAppDidResign:),
                app_did_resign as extern "C" fn(&Object, Sel, id),
            );
            let cls = decl.register();
            // Lives for the process; never released.
            let observer: id = msg_send![cls, new];
            let center: id = msg_send![class!(NSNotificationCenter), defaultCenter];
            let name = NSString::alloc(nil).init_str("NSApplicationDidResignActiveNotification");
            let _: () = msg_send![
                center,
                addObserver: observer
                selector: sel!(seanceAppDidResign:)
                name: name
                object: nil
            ];
        }
        cx.spawn(async move |cx| {
            use futures::StreamExt;
            while let Some(posted) = rx.next().await {
                // Activation is async, so `isActive` alone can still read
                // false right after a hotkey show; the generation can't.
                let current = SHOW_GEN.load(Ordering::SeqCst);
                if super::resign_is_stale(posted, current, app_is_active()) {
                    continue;
                }
                cx.update(super::super::window_hotkeys::WindowHotkeys::on_app_resigned);
            }
        })
        .detach();
    }
}

#[cfg(not(target_os = "macos"))]
pub fn configure(_window: &Window, _on: bool, _normal: &mut Option<NormalBehavior>) {}

#[cfg(not(target_os = "macos"))]
pub fn toggle_action(_window: &Window, _overlay: bool) -> ToggleAction {
    ToggleAction::GpuiFallback
}

#[cfg(not(target_os = "macos"))]
pub fn show(window: &mut Window, _overlay: bool, cx: &mut App) {
    window.activate_window();
    let _ = cx;
}

#[cfg(not(target_os = "macos"))]
pub fn frontmost_other_app() -> Option<i32> {
    None
}

#[cfg(not(target_os = "macos"))]
pub fn configure_companion(_window: &Window, _on: bool, _normal: &mut Option<NormalBehavior>) {}

#[cfg(not(target_os = "macos"))]
pub fn pointer_display_id(_cx: &App) -> Option<gpui::DisplayId> {
    None
}

#[cfg(not(target_os = "macos"))]
pub fn raise_companion(_window: &mut Window, _cx: &mut App) {}

#[cfg(not(target_os = "macos"))]
pub fn hide(_window: &Window, _prior: Option<i32>) {}

#[cfg(not(target_os = "macos"))]
pub fn hide_quietly(_window: &Window) {}

#[cfg(not(target_os = "macos"))]
pub fn install_resign_observer(_cx: &mut App) {}

#[cfg(test)]
mod tests {
    #[cfg(target_os = "macos")]
    use super::mac::{companion_collection, overlay_collection};
    use super::relocate_preserving_visible_offset;
    use super::resign_is_stale;
    use super::Frame2d;

    #[test]
    fn queued_resign_from_before_a_hotkey_show_does_not_hide_it() {
        // Posted at gen 4, then a show bumped to 5 while activation is
        // still in flight (isActive false): stale.
        assert!(resign_is_stale(4, 5, false));
        // Already active again: stale regardless of generation.
        assert!(resign_is_stale(5, 5, true));
        // Genuine: nothing shown since, and we really lost activation.
        assert!(!resign_is_stale(5, 5, false));
    }

    #[test]
    fn relocate_keeps_offset_when_both_screens_fit() {
        let frame = Frame2d {
            x: 120.0,
            y: 840.0,
            w: 720.0,
            h: 640.0,
        };
        let from = Frame2d {
            x: 0.0,
            y: 0.0,
            w: 1920.0,
            h: 1080.0,
        };
        let to = Frame2d {
            x: 1920.0,
            y: 0.0,
            w: 2560.0,
            h: 1440.0,
        };
        let out = relocate_preserving_visible_offset(frame, from, to);
        assert_eq!(out.x, 1920.0 + 120.0);
        // Offset 840 + height 640 exceeds 1440 visible — clamped to top of target.
        assert_eq!(out.y, 800.0);
        assert_eq!(out.w, 720.0);
        assert_eq!(out.h, 640.0);
    }

    #[test]
    fn relocate_clamps_when_window_wider_than_target_visible() {
        let frame = Frame2d {
            x: 0.0,
            y: 0.0,
            w: 900.0,
            h: 400.0,
        };
        let from = Frame2d {
            x: 0.0,
            y: 0.0,
            w: 1000.0,
            h: 800.0,
        };
        let to = Frame2d {
            x: 1000.0,
            y: 0.0,
            w: 500.0,
            h: 800.0,
        };
        let out = relocate_preserving_visible_offset(frame, from, to);
        assert_eq!(out.w, 500.0);
        assert_eq!(out.x, 1000.0);
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn settings_companion_joins_all_spaces_without_move_to_active_space() {
        let join_all = 1 << 0;
        let move_active = 1 << 1;
        let managed = 1 << 2;
        let primary = 1 << 7;
        let aux = 1 << 8;
        let out = companion_collection(move_active | primary | managed);
        assert_eq!(out & move_active, 0);
        assert_eq!(out & primary, 0);
        assert_eq!(out & (join_all | aux), join_all | aux);
        assert_eq!(out & managed, managed);
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn overlay_never_combines_join_all_spaces_with_move_to_active_space() {
        let join_all = 1 << 0;
        let move_active = 1 << 1;
        let primary = 1 << 7;
        let aux = 1 << 8;
        let managed = 1 << 2;
        let out = overlay_collection(join_all | primary | managed);
        assert_eq!(out & join_all, 0);
        assert_eq!(out & primary, 0);
        assert_ne!(out & move_active, 0);
        assert_ne!(out & aux, 0);
        // Unrelated flags GPUI (or AppKit) set survive.
        assert_ne!(out & managed, 0);
    }
}
