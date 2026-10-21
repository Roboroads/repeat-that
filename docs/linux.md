# Repeat That on Linux

Linux support is planned for milestone 5. This page covers how Repeat That fits into a Linux desktop: where the hotkeys
come from, how to run its actions from the command line, and what happens when the desktop has no tray.

## Hotkeys

How the hotkeys work depends on the session you are logged in to.

- **X11.** The Hotkeys section works as it does on Windows. Click a field and press the combination you want.
- **Wayland on KDE Plasma, or GNOME 48 and later.** An app can't grab keys itself on Wayland. It tells the desktop which
  actions it has, and the desktop asks you which keys to give them (through the XDG GlobalShortcuts portal). On the
  first start a system dialog asks you to confirm the three shortcuts. Repeat That suggests Ctrl+Alt+Space,
  Ctrl+Alt+Backspace and Ctrl+Alt+Enter, and the desktop may offer something else. The Hotkeys section shows the keys
  you ended up with, or "Not assigned" for an action that has none, and has no recorder. To change a key, press
  **Change in system settings**. Depending on the version of the portal that opens the desktop's own shortcut page, or
  tells you where to look: in KDE it is System Settings → Shortcuts → Repeat That, in GNOME it is Settings → Keyboard →
  View and Customize Shortcuts → Apps. If you closed the first dialog without choosing, no action has a key and the
  button reads **Set up shortcuts**: it opens the dialog again. Other Wayland desktops that provide the portal work the
  same way. If the portal is slow to start at login, or starts later, Repeat That keeps looking for it and sets the
  shortcuts up as soon as it is there.
- **Wayland without the portal.** GNOME 46 (Ubuntu 24.04), Sway, Hyprland and others have no GlobalShortcuts portal, so
  Repeat That can't have global hotkeys there. The Hotkeys section says so and lists the three commands below, each with
  a Copy button. Bind them as custom shortcuts in your desktop, and they do the same job.

## Command-line actions

Three commands go to the Repeat That that is already running:

```sh
repeat-that --replay
repeat-that --stop
repeat-that --catch-up
```

The command depends on how Repeat That is installed, and the Hotkeys section shows the exact one for your install. For
a Flatpak it is `flatpak run me.roboroads.RepeatThat --replay`, for an AppImage it is the path of the AppImage file
followed by `--replay`, and for a `.deb` or `.rpm` it is `repeat-that --replay`.

Each command counts as one press of the matching hotkey. A second `--replay` while a replay plays steps back one
utterance, `--catch-up` speeds up from where the replay is and stops when it reaches live, and `--stop` ends the replay.
The commands never open the settings window. If Repeat That isn't running, the command starts it without a window and
runs the action once it is up. Nothing is buffered yet at that point, so a replay plays the short beep. The one
exception is the very first time you run Repeat That: it opens the window then, whatever started it, so that you can
pick the app to catch up on. Flags that
Repeat That doesn't know are ignored, and `--hidden` can be combined with an action.

### Binding the commands

- **GNOME.** Settings → Keyboard → View and Customize Shortcuts → Custom Shortcuts. Add a shortcut with the command as
  its command and press the key you want.
- **KDE Plasma.** System Settings → Shortcuts → Add New → Command or Script. Paste the command and set the shortcut
  on the Triggers tab.
- **Sway.** In the config file: `bindsym Ctrl+Alt+space exec repeat-that --replay`.
- **Hyprland.** In the config file: `bind = CTRL ALT, space, exec, repeat-that --replay`.

## Tray

The tray icon needs a StatusNotifierItem host, which KDE Plasma and most other desktops have. GNOME doesn't, unless the
AppIndicator and KStatusNotifierItem Support extension is installed and turned on.

Without a host, Repeat That has no tray menu to open the window from or to quit from. So it opens the settings window at
every launch (except with `--hidden`, and for a command-line action), and the General section has a notice and a **Quit
Repeat That** button. Closing the window only hides it: the hotkeys keep working, and starting Repeat That from the app
menu shows it again. If you turn the extension on later, the tray icon appears without a restart and the notice goes away.

## Start at login

The start-at-login switch writes an entry in `~/.config/autostart` that passes `--hidden`. Repeat That then starts
without a window, also on a desktop with no tray.
