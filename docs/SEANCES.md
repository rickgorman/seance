# Separate Seances and overlay windows

Open Settings with **Command-comma** (Control-comma on Linux), then choose
**Windows**. Create a named Seance such as **Client work**, **JoyRudder**, or
**Personal**, and choose the sidebar tabs it should contain.

Each tab belongs to one place. Selecting a tab moves it into that Seance;
unselecting it returns it to the main window. Main always shows unassigned
tabs. Each Seance can have several tabs, and each tab can contain several
terminal sessions. Creating a tab or session inside a Seance keeps it there.

Use **Open** to show a named window, and **Bind** to record its system-wide
visibility shortcut on macOS. Renaming a Seance preserves its tabs and hotkey.
Removing one closes its window and returns its tabs to Main; its terminal
sessions keep running. Existing single-tab window definitions migrate when
the updated app reads them.

## Overlay

Enable **Overlay** for Main or any named Seance to summon that window above
the current Mac Space, on the screen under the pointer. Pressing its hotkey
again hides it and returns focus to the previous app. Switching to another
app also hides visible overlays. Switching between Seance windows and Settings
keeps them available. Turning Overlay off restores ordinary window behavior.

The app must be running for global hotkeys to work. Overlay and global hotkeys
use native macOS window and hotkey APIs; no Accessibility permission is needed.

Seance names, tab membership, hotkeys, and overlay choices live in the local
desktop preferences. Terminal processes and their session data remain with
the existing daemon.
