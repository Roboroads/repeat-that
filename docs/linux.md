# Repeat That on Linux

This page covers how Repeat That fits into a Linux desktop: where the hotkeys come from, how to run its actions from the
command line, and what happens when the desktop has no tray. Installing the `.deb`, the `.rpm` or the AppImage is in
[install-linux.md](install-linux.md).

## Hotkeys

How the hotkeys work depends on the session you are logged in to.

- **X11.** The Hotkeys section works as it does on Windows. Click a field and press the combination you want.
- **Wayland on KDE Plasma, or GNOME 48 and later.** An app can't grab keys itself on Wayland. It tells the desktop which
  actions it has, and the desktop asks you which keys to give them (through the XDG GlobalShortcuts portal). On the
  first start a system dialog asks you to confirm the three shortcuts. Repeat That suggests Ctrl+Alt+Space,
  Ctrl+Alt+Backspace and Ctrl+Alt+Enter, and the desktop may offer something else. The Hotkeys section shows the keys
  you ended up with, or "Not assigned" for an action that has none, and has no recorder. To change a key, press
  **Change in system settings**. A version 2 portal opens the desktop's shortcut page itself. Plasma's portal is
  version 1, so there Repeat That starts the shortcuts module of System Settings (`systemsettings kcm_keys`, or
  `kcmshell6 kcm_keys`), and on GNOME it starts `gnome-control-center keyboard`. In a Flatpak that needs permission to
  start programs on the host (`flatpak-spawn --host`). When none of this is possible, the window only says where to
  look: in KDE it is System Settings → Shortcuts → Repeat That, in GNOME it is Settings → Keyboard → View and Customize
  Shortcuts → Apps. If you closed the first dialog without choosing, no action has a key and the
  button reads **Set up shortcuts**: it opens the dialog again. Other Wayland desktops that provide the portal work the
  same way. If the portal is slow to start at login, or starts later, Repeat That keeps looking for it and sets the
  shortcuts up as soon as it is there. Plasma 6.3's portal lists the shortcuts under the app ID
  (`me.roboroads.repeat-that`) after every start, so Repeat That gives them back the name "Repeat That" in kglobalaccel
  each time it starts. Plasma 6.4 and later get the name right themselves. The Flatpak does the same under its own
  ID, `me.roboroads.RepeatThat`, which the portal reads from the sandbox; for that it has permission to talk to
  kglobalaccel (`--talk-name=org.kde.kglobalaccel`) and uses it for nothing else. If a **default key is left
  unassigned**, another shortcut already has it: Plasma leaves out, without a word, a key that another component
  holds, and only offers a key when it first sees an action. This happens when Repeat That is installed twice, say the
  `.deb` or the AppImage next to the Flatpak: the other install keeps the same three keys under
  `me.roboroads.repeat-that`. Plasma keeps what it has for an action, empty included, so freeing the key later doesn't
  give it to the action: remove the entry you don't use in System Settings → Shortcuts, then press **Default** next to
  each unassigned action of the entry you keep (or give them other keys there). To see who has the keys, run
  `grep -B1 -A4 roboroads ~/.config/kglobalshortcutsrc`: each action reads `current key,default key,name`, so
  `replay=,Ctrl+Alt+Space,replay` lost its key and `replay=Ctrl+Alt+Space,Ctrl+Alt+Space,replay` has it.
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

In a Flatpak, the app can't write that file: the system does, when Repeat That asks through the Background portal, and
your desktop asks you first. The entry is `~/.config/autostart/me.roboroads.RepeatThat.desktop` and starts the Flatpak
with `--hidden`. The portal can't tell the app whether the entry is still there, so removing it in your desktop's
settings doesn't turn the switch off: turn it off and on again. If you answer no to the desktop's question, the switch
turns itself off again. The Flatpak also asks, at every start, for permission to run in the background (the portal
asks you once), see [install-linux.md](install-linux.md#install-the-flatpak).
