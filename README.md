# Repeat That

**Missed what someone just said on a call? Press a key and hear it again.**

Website and downloads: [repeat-that.roboroads.me](https://repeat-that.roboroads.me)

Repeat That keeps a short rolling buffer of the audio coming out of one app — Discord, Teams, Zoom, Slack, your browser —
skips the silence, and replays the last thing that was said whenever you press a hotkey. No awkward
"sorry, could you repeat that?" needed.

It's built for people whose attention wanders during calls (hello, ADHD 👋), but it's useful for anyone dealing with bad
connections, accents, background noise, or just a moment of distraction.

> **Status:** version 0.1 for Windows is feature-complete and has been tested on real Windows hardware. The first
> release is `v0.1.0`. macOS support is built but **hasn't been tested on a real Mac yet**, see
> [Install (macOS)](#install-macos). Linux (PipeWire) is new in the releases after `v0.1.0`, as a `.deb`, an `.rpm`, an
> AppImage and a Flatpak, see [Install (Linux)](#install-linux).

## System requirements

### Windows

| Requirement | Minimum |
|---|---|
| Operating system | Windows 10 version 2004 (build 19041) or newer, or Windows 11 |
| Processor | 64-bit x86 (x64). Windows on ARM isn't supported yet |
| WebView2 runtime | Windows 11 and up-to-date Windows 10 have it. If it's missing, the installer downloads it, so you need an internet connection that one time |

### macOS

| Requirement | Minimum |
|---|---|
| Operating system | macOS 14.2 or newer |
| Processor | Apple Silicon (M1 or later). Intel Macs aren't supported |
| Permission | Audio capture from other apps, which macOS asks for the first time Repeat That captures |

macOS support is built but hasn't been tested on a real Mac yet.

### Linux

| Requirement | Minimum |
|---|---|
| Operating system | Debian 13, Ubuntu 24.04 or Fedora 40, or newer. Debian 12 is too old: the speech detector needs glibc 2.38 and the libstdc++ of GCC 13 |
| Processor | 64-bit x86 (x86_64). Arm isn't supported yet |
| Audio | PipeWire with WirePlumber. On PulseAudio the app starts but can't capture |
| Hotkeys | X11, or Wayland with the GlobalShortcuts portal (KDE Plasma, GNOME 48 and later). On other Wayland desktops, such as GNOME 46 on Ubuntu 24.04, you bind the `repeat-that --replay`, `--catch-up` and `--stop` commands yourself |

Other distributions can use the AppImage if they have glibc 2.38 or newer.

The steps to install are in [docs/install-windows.md](docs/install-windows.md),
[docs/install-macos.md](docs/install-macos.md) and [docs/install-linux.md](docs/install-linux.md). The Linux hotkeys are
explained in [docs/linux.md](docs/linux.md).

## Install (Windows)

You need 64-bit Windows 10 version 2004 or newer, or Windows 11. Download `Repeat-That_<version>_x64-setup.exe` from the
[Releases page](https://github.com/Roboroads/repeat-that/releases) and run it. It installs for your user only and
doesn't ask for administrator rights.

The installer isn't code-signed yet, so Windows shows a SmartScreen warning the first time: click **More info**, then
**Run anyway**. Compare your download with the release's `SHA256SUMS` before you run it, see
[docs/verify-downloads.md](docs/verify-downloads.md). The full steps, including updating and uninstalling, are in
[docs/install-windows.md](docs/install-windows.md).

## Install (macOS)

You need a Mac with Apple Silicon and macOS 14.2 or later; Intel Macs aren't supported. Releases after `v0.1.0` have a
`Repeat-That_<version>_aarch64.dmg` on the [Releases page](https://github.com/Roboroads/repeat-that/releases). Open it
and drag Repeat That to Applications. The app is signed with a Developer ID certificate and notarized by Apple, so it
opens like any other app. The first time it captures, macOS asks whether Repeat That may capture audio from other apps.
Compare your download with the release's `SHA256SUMS`, see [docs/verify-downloads.md](docs/verify-downloads.md). The
full steps, including the permission, updating and uninstalling, are in [docs/install-macos.md](docs/install-macos.md).

**macOS support is new and hasn't been tested on a real Mac yet.** Nobody on the team has one, so the app has only been
built and checked on GitHub's macOS runners. If something doesn't work, please
[file a macOS report](https://github.com/Roboroads/repeat-that/issues/new?template=macos.yml).

## Install (Linux)

You need 64-bit x86 Linux with PipeWire and WirePlumber: Debian 13, Ubuntu 24.04, Fedora 40 or newer. Debian 12 is
too old (the speech detector needs glibc 2.38), except for the Flatpak. Releases after `v0.1.0` have four files on the
[Releases page](https://github.com/Roboroads/repeat-that/releases):

- `Repeat-That_<version>_amd64.deb` for Debian and Ubuntu: `sudo apt install ./Repeat-That_<version>_amd64.deb`
- `Repeat-That-<version>-1.x86_64.rpm` for Fedora: `sudo dnf install ./Repeat-That-<version>-1.x86_64.rpm`
- `Repeat-That_<version>_amd64.AppImage` for other distributions: make it executable (`chmod +x`) and run it
- `Repeat-That_<version>_x86_64.flatpak` for any distribution with Flatpak: `flatpak install ./Repeat-That_<version>_x86_64.flatpak`

Compare your download with the release's `SHA256SUMS`, see [docs/verify-downloads.md](docs/verify-downloads.md). The
full steps, including the AppImage's differences, updating and uninstalling, are in
[docs/install-linux.md](docs/install-linux.md). How the hotkeys work on X11 and Wayland, the command-line actions and
desktops without a tray are in [docs/linux.md](docs/linux.md).

## Using it

Repeat That lives in the system tray (the menu bar on a Mac). The first launch also opens the settings window; closing
it keeps the app running, and Quit in the tray menu ends it.

1. Start the app you want to catch up on (Discord, Teams, a browser tab with a video). It shows up in the **App** list
   within a couple of seconds.
2. Pick it. The list folds into the app you chose, and a click on that row opens the list again. Repeat That now keeps
   the last 30 seconds of its speech in memory (you can change the length).
3. When you miss something, press a hotkey:

| Default hotkey | What it does |
|---|---|
| `Ctrl+Alt+Space` | **Replay.** Plays the last thing that was said. Pressed again during a replay, it jumps back one utterance (a stretch of speech between pauses). |
| `Ctrl+Alt+Enter` | **Catch-up.** The same replay at a faster speed (1.5× by default, same pitch). It stops as soon as it reaches live. Pressed again during a catch-up, it jumps back one utterance. |
| `Ctrl+Alt+Backspace` | **Stop.** Ends the replay at once. |

On a Mac, Alt is the Option key. On Linux under Wayland the desktop assigns the keys, or you bind the commands
`repeat-that --replay`, `--catch-up` and `--stop` yourself, see [docs/linux.md](docs/linux.md).

Pressing the hotkey of the other mode while a replay plays switches the speed from where the replay is, without
starting over. Jumping back from the oldest utterance in the buffer goes to the start of that utterance. The app keeps
listening while you replay, so what is said in the meantime plays right after, and the replay ends when it has caught up
with the call.

While a replay plays, the app's volume drops to 20% (you can change that or turn it off), and it comes back about
300 ms after the replay ends. All three hotkeys can be changed in the settings, and the settings window warns when
another program already uses a combination.

The [user guide](docs/guide.md) covers every setting, what differs per operating system and what to do when a replay
beeps instead of playing.

## Features

- Captures the audio of one app you choose, never your microphone and never the rest of your system sound (WASAPI
  process loopback on Windows, Core Audio process taps on macOS, PipeWire on Linux).
- Keeps the last 10 to 120 seconds of **speech** (30 by default) in memory. Silence is cut out with voice activity
  detection, and a sensitivity setting handles quiet or noisy speakers.
- Replay, step back one utterance at a time, catch up at 1.25 to 2× speed with the pitch kept.
- Ducks the call while a replay plays, so you don't hear both.
- Plays replays on the output device you pick, and falls back to the default device when it is unplugged.
- Tray icon and menu (Replay, Stop, Catch-up, Pause capture, Settings, Quit). The tooltip says how far behind live a
  replay is.
- Optional start with Windows, or at login on a Mac or on Linux, in the tray.
- Waits for the app when it isn't running and picks it up again when it starts. Speech buffered before it quit can
  still be replayed.

Planned: a live transcript of recent speech (local Whisper, fully offline) and the Flatpak on Flathub. See the
[roadmap](ROADMAP.md).

## Platform support

| Platform | Minimum version | Capture method | Status |
|---|---|---|---|
| Windows (x64) | Windows 10 2004 / 11 | WASAPI process loopback | v0.1 |
| Linux (x64: `.deb`, `.rpm`, AppImage, Flatpak) | Debian 13 / Ubuntu 24.04 / Fedora 40 with PipeWire | PipeWire per-stream capture | After v0.1.0 |
| macOS (Apple Silicon only) | macOS 14.2 | Core Audio process taps | Built, untested |

macOS is Apple Silicon only: Microsoft ships no x86_64 or universal ONNX Runtime after 1.23.2, and the speech detector
needs it.

**macOS note:** macOS releases are signed with a Developer ID certificate and notarized by Apple, so they open like any
other app. macOS support hasn't been tested on a real Mac yet; please
[report problems](https://github.com/Roboroads/repeat-that/issues/new?template=macos.yml). The audio-capture permission
and the steps to build on macOS (Xcode command-line tools, Rust, Node.js) are in [docs/macos.md](docs/macos.md).

## Privacy

Recorded audio **only ever lives in memory**. It is never written to disk, never sent anywhere, and is wiped as soon as you
stop capturing or quit the app. Transcription (when it lands) runs locally on your machine. See [PRIVACY.md](PRIVACY.md),
including a note on recording laws.

## Building from source

The app is a [Tauri 2](https://tauri.app) application with a Rust core and a Svelte + TypeScript UI.

```sh
# Rust crates and tests (any OS)
cargo test --workspace

# The desktop app, with live reload (needs Node.js 22.19 or later and the system packages below)
cd app
npm install
npm run tauri dev
```

The app starts in the tray. The first launch also opens the settings window; closing it keeps the app running in the
tray, and Quit in the tray menu ends it. `REPEAT_THAT_MOCK_WAV=<file.wav>` plays a speech recording as a fake app
instead of capturing real ones, on any OS. Add `REPEAT_THAT_NULL_OUTPUT=1` on a machine without an audio device.

System packages (the [Tauri prerequisites](https://tauri.app/start/prerequisites/) cover Windows and macOS):

```sh
# Debian / Ubuntu
sudo apt install pkg-config libwebkit2gtk-4.1-dev libgtk-3-dev libayatana-appindicator3-dev librsvg2-dev \
  libasound2-dev libxdo-dev libpipewire-0.3-dev libspa-0.2-dev libclang-dev clang
```

On Linux, `npm run tauri build` makes the `.deb`, the `.rpm` and the AppImage, see
[docs/install-linux.md](docs/install-linux.md#build-from-source).

See [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) for how the pieces fit together.

## Contributing

Contributions are very welcome — see [CONTRIBUTING.md](CONTRIBUTING.md). Please follow our
[Code of Conduct](CODE_OF_CONDUCT.md).

## License

Repeat That is free software, licensed under the [GNU General Public License v3.0 or later](LICENSE).
