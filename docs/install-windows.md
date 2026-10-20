# Installing on Windows

Repeat that?! needs 64-bit Windows 10 version 2004 (build 19041) or newer, or Windows 11. Windows on ARM isn't
supported yet.

The app is free and volunteer-run, and the installers aren't code-signed yet (we're applying for free signing through
[SignPath Foundation](https://signpath.org)). Windows therefore shows a SmartScreen warning the first time you run one.
That's expected; the steps below get you past it.

## Download

Get the installer from the [Releases page](https://github.com/Roboroads/repeat-that/releases). Each release has:

| File | What it is |
| --- | --- |
| `Repeat-that_<version>_x64-setup.exe` | The normal installer. Installs for your user only and doesn't ask for administrator rights. |
| `Repeat-that_<version>_x64_en-US.msi` | For managed PCs and scripted installs. Installs for all users, so it asks for administrator rights. |
| `SHA256SUMS` | Checksums of the two installers, to [verify your download](#verify-your-download). |

If you aren't sure, take the `.exe`.

## Install

1. Run the installer. Windows shows **Windows protected your PC** because the file isn't signed. Click **More info**,
   then **Run anyway**.
2. Follow the installer. If the WebView2 runtime that the settings window needs isn't on your PC yet (Windows 11 and
   up-to-date Windows 10 have it), the installer downloads it, so you need an internet connection for that one time.
3. Repeat that?! starts in the system tray and opens its settings window on the first run. If you can't see the tray icon,
   click the **^** arrow next to the clock.

After that, pick the app whose audio you want to keep (Discord, Teams, your browser, ...) in the settings window.
Captured audio stays in memory and is never written to disk; see [PRIVACY.md](../PRIVACY.md).

## Verify your download

Each release publishes a `SHA256SUMS` file. Compare your download with it before you run the installer. In PowerShell:

```powershell
Get-FileHash "$HOME\Downloads\Repeat-that_*_x64-setup.exe" -Algorithm SHA256
```

The `Hash` column must match the line for that file in `SHA256SUMS` (letter case doesn't matter). Do the same for the
`.msi` if you use it.

## Updating and uninstalling

There's no automatic updater yet. To update, download the newer installer and run it over the old version; your
settings are kept.

To uninstall, use **Settings → Apps → Installed apps → Repeat that**. The uninstaller offers to delete the app's data
(`%APPDATA%\me.roboroads.repeatthat`, which holds your settings and the chosen app's name and path, never audio).

## Build from source

Installers come from CI, but you can build your own:

```powershell
# needs Rust (https://rustup.rs), Node.js and the Visual Studio C++ build tools
git clone https://github.com/Roboroads/repeat-that
cd repeat-that\app
npm ci
npm run tauri build -- --bundles nsis,msi
```

The installers end up in `target\release\bundle\nsis\` and `target\release\bundle\msi\`.
