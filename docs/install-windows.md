# Installing on Windows

Repeat That needs 64-bit Windows 10 version 2004 (build 19041) or newer, or Windows 11. Windows on ARM isn't
supported yet.

The app is free and volunteer-run, and the installer isn't code-signed yet (we're applying for free signing through
[SignPath Foundation](https://signpath.org)). Windows therefore shows a SmartScreen warning the first time you run it.
That's expected; the steps below get you past it.

## Download

Get the installer from the [Releases page](https://github.com/Roboroads/repeat-that/releases). Each release has:

- `Repeat-That_<version>_x64-setup.exe` is the installer. It installs for your user only (under `%LOCALAPPDATA%`) and
  doesn't ask for administrator rights.
- `SHA256SUMS` holds the checksum of the installer, to [verify your download](verify-downloads.md).

## Install

1. Run the installer. Windows shows **Windows protected your PC** because the file isn't signed. Click **More info**,
   then **Run anyway**.
2. Follow the installer. If the WebView2 runtime that the settings window needs isn't on your PC yet (Windows 11 and
   up-to-date Windows 10 have it), the installer downloads it, so you need an internet connection for that one time.
3. Repeat That starts in the system tray and opens its settings window on the first run. If you can't see the tray icon,
   click the **^** arrow next to the clock. Later you can start it from the **Repeat That** entry in the Start menu.

After that, pick the app whose audio you want to keep (Discord, Teams, your browser, ...) in the settings window.
Captured audio stays in memory and is never written to disk; see [PRIVACY.md](../PRIVACY.md).

## Verify your download

Compare your download with the release's `SHA256SUMS` before you run it; see [verify-downloads.md](verify-downloads.md).

## Updating and uninstalling

There's no automatic updater yet. To update, download the newer installer and run it over the old version; your settings
are kept.

To uninstall, use **Settings → Apps → Installed apps → Repeat That**. The uninstaller offers to delete the app's data.
The data is in `%APPDATA%\me.roboroads.repeat-that` and holds your settings and the chosen app's name and path, never
audio.

## Build from source

The installer comes from CI, but you can build your own:

```powershell
# needs Rust (https://rustup.rs), Node.js 22.19 or later and the Visual Studio C++ build tools
git clone https://github.com/Roboroads/repeat-that
cd repeat-that\app
npm ci
npm run tauri build
```

The installer ends up in `target\release\bundle\nsis\`.
