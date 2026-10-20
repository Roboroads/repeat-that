# Installing on Windows

Repeat That needs 64-bit Windows 10 version 2004 (build 19041) or newer, or Windows 11. Windows on ARM isn't
supported yet.

The app is free and volunteer-run, and the installers aren't code-signed yet (we're applying for free signing through
[SignPath Foundation](https://signpath.org)). Windows therefore shows a SmartScreen warning the first time you run one.
That's expected; the steps below get you past it.

## Download

Get the installer from the [Releases page](https://github.com/Roboroads/repeat-that/releases). Each release has:

| File | What it is |
| --- | --- |
| `Repeat-That_<version>_x64-setup.exe` | The normal installer. Installs for your user only and doesn't ask for administrator rights. |
| `Repeat-That_<version>_x64_en-US.msi` | For managed PCs and scripted installs. Installs for all users, so it asks for administrator rights. |
| `SHA256SUMS` | Checksums of the two installers, to [verify your download](verify-downloads.md). |

If you aren't sure, take the `.exe`. The two are separate installs, not two ways to get the same one:

| | `.exe` | `.msi` |
| --- | --- | --- |
| Installed for | Your user (under `%LOCALAPPDATA%`) | All users (under `C:\Program Files`) |
| Needs administrator rights | No | Yes |
| On uninstall | Offers to delete the app's data | Leaves the data of every user behind |

**Pick one and stay with it.** Installing the other kind next to it gives you two copies, two Start menu entries and two
entries under Installed apps. If you want to switch, uninstall the old one first.

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

There's no automatic updater yet. To update, download the newer installer of the same kind you installed (`.exe` or
`.msi`) and run it over the old version; your settings are kept.

To uninstall, use **Settings → Apps → Installed apps → Repeat That**. The `.exe` uninstaller offers to delete the app's
data. The `.msi` doesn't: it removes the program but leaves each user's data in place. The data is in
`%APPDATA%\me.roboroads.repeat-that` and holds your settings and the chosen app's name and path, never audio. Delete
that folder yourself if you want it gone after an `.msi` uninstall.

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
