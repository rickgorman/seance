# Desktop settings verification — 0.26.3

Automated checks pass: `./scripts/check.sh` (478 native tests, formatting,
zero compiler warnings) and `cargo test --workspace` (699 tests). The release
app and web assets build successfully. Installed iTerm2 preferences yielded
three color schemes without import warnings. A live graceful daemon upgrade
preserved all 43 original terminal processes and session identities.

Font selection, keyboard remapping, Command-minus/equal zoom, sidebar spacing,
and pane dividers were checked in the desktop app before this release.

## Pending manual checks

The macOS desktop became unavailable to UI automation (`cgWindowNotFound`
across applications), so the following checks remain open. GitHub issues are
disabled on the fork; this file records the follow-up.

- From another app, toggle the main Seance window using a configured global
  hotkey. Verify that an active window hides and a hidden/background window
  appears and receives focus.
- Configure two dedicated workspace windows with different hotkeys. Check
  their independent visibility, scoped session lists, recreation after closing,
  and that removing a window definition preserves its terminal sessions.
- While recording a shortcut in Settings, press a registered global hotkey.
  Check conflict feedback and confirm the target window does not hide.
- Edit terminal color hex values, inspect the preview, test invalid input,
  and verify Apply, Reset, and persistence after reopening the app.
- Import installed iTerm2 profiles and an `.itermcolors` file through the
  Colors tab. Choose a palette, apply it, and inspect text, ANSI colors,
  selection, and cursor rendering.

Restore temporary test hotkeys, window definitions, and colors after testing.
