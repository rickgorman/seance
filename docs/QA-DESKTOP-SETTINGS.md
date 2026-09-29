# Desktop settings verification — 0.26.4

`./scripts/check.sh` passes formatting, zero compiler warnings, and 502 native
tests. The workspace suite passes 723 tests (502 native, 91 core, 130 web).
The release app and matching web assets build successfully. The web build
still reports three pre-existing warnings in unchanged web source files.

## Verified in the macOS app

- Colors opens without the former `Root::read` panic. Installed iTerm2 import
  displays three profiles; choosing one updates the draft. Closing the draft
  leaves saved colors unchanged.
- Command-W closes Settings from a focused color field and during shortcut
  recording. Settings can reopen normally afterward.
- Two temporary named Seances displayed exactly six and four distinct sidebar
  tabs. Moving a tab between the open windows changed their lists to seven and
  three. Renaming preserved membership and the shortcut.
- Definitions, names, membership, and shortcut assignments survived a GUI
  restart. Removing the two live test windows returned all 12 original tabs
  to Main and removed their temporary hotkeys.
- Overlay is configurable independently for Main and named Seances. Main opens
  with Overlay persisted. Settings remains correctly scaled and clickable on
  the external display after that startup, using a native auxiliary panel.
- All 43 original terminal processes and session identities survived the
  graceful daemon upgrade, GUI restarts, tab moves, and window removals.

The earlier font, zoom, keyboard remapping, sidebar-spacing, and divider fixes
were also visually checked. Tests cover color parsing/validation, legacy
migration, exclusive membership, stable hotkey targets, registration rollback,
projection, fold preservation, collection flags, and display geometry.

## Remaining physical-keyboard checks

App-targeted automation did not trigger macOS global hotkeys. These need a
physical keyboard check; they are not represented as completed UI tests:

- Summon and dismiss Main and two named Seances from another app, including a
  fullscreen Space, without changing the visible Space or raising the other
  Seance windows. Check focus return and hiding when leaving the app.
- Repeat across displays with different scaling, and after closing a window
  so the hotkey must recreate it.
- Press an already registered global chord during Settings capture and verify
  conflict feedback without toggling a window.

GitHub issues are disabled on the fork; this file records the follow-up.
Temporary test collections and bindings were removed. The existing Main
shortcut was preserved, with its new Overlay option enabled for review.
