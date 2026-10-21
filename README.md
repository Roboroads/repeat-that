# Repeat That

**Missed what someone just said on a call? Press a key and hear it again.**

Repeat That keeps a short rolling buffer of the audio coming out of one app — Discord, Teams, Zoom, Slack, your browser —
skips the silence, and replays the last thing that was said whenever you press a hotkey. No awkward
"sorry, could you repeat that?" needed.

It's built for people whose attention wanders during calls (hello, ADHD 👋), but it's useful for anyone dealing with bad
connections, accents, background noise, or just a moment of distraction.

> **Status:** version 0.1 for Windows is feature-complete and has been tested on real Windows hardware. The first
> release is `v0.1.0`. Linux and macOS are planned, see the [roadmap](ROADMAP.md).

## Install (Windows)

You need 64-bit Windows 10 version 2004 or newer, or Windows 11. Download `Repeat-That_<version>_x64-setup.exe` from the
[Releases page](https://github.com/Roboroads/repeat-that/releases) and run it. It installs for your user only and
doesn't ask for administrator rights.

The installer isn't code-signed yet, so Windows shows a SmartScreen warning the first time: click **More info**, then
**Run anyway**. Compare your download with the release's `SHA256SUMS` before you run it, see
[docs/verify-downloads.md](docs/verify-downloads.md). The full steps, including updating and uninstalling, are in
[docs/install-windows.md](docs/install-windows.md).

## Using it

Repeat That lives in the system tray. The first launch also opens the settings window; closing it keeps the app running,
and Quit in the tray menu ends it.

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

Pressing the hotkey of the other mode while a replay plays switches the speed from where the replay is, without
starting over. Jumping back from the oldest utterance in the buffer goes to the start of that utterance. The app keeps
listening while you replay, so what is said in the meantime plays right after, and the replay ends when it has caught up
with the call.

While a replay plays, the app's volume drops to 20% (you can change that or turn it off), and it comes back about
300 ms after the replay ends. All three hotkeys can be changed in the settings, and the settings window warns when
another program already uses a combination.

## Features

- Captures the audio of one app you choose, never your microphone and never the rest of your system sound (Windows,
  WASAPI process loopback).
- Keeps the last 10 to 120 seconds of **speech** (30 by default) in memory. Silence is cut out with voice activity
  detection, and a sensitivity setting handles quiet or noisy speakers.
- Replay, step back one utterance at a time, catch up at 1.25 to 2× speed with the pitch kept.
- Ducks the call while a replay plays, so you don't hear both.
- Plays replays on the output device you pick, and falls back to the default device when it is unplugged.
- Tray icon and menu (Replay, Stop, Catch-up, Pause capture, Settings, Quit). The tooltip says how far behind live a
  replay is.
- Optional start with Windows, in the tray.
- Waits for the app when it isn't running and picks it up again when it starts. Speech buffered before it quit can
  still be replayed.

Planned: a live transcript of recent speech (local Whisper, fully offline), Linux (PipeWire) and macOS support. See the
[roadmap](ROADMAP.md).

## Platform support

| Platform | Minimum version | Capture method | Status |
|---|---|---|---|
| Windows (x64) | Windows 10 2004 / 11 | WASAPI process loopback | v0.1 |
| Linux (Debian/Ubuntu, `.deb`) | Debian 12 / Ubuntu 22.04 with PipeWire | PipeWire per-stream capture | Planned |
| macOS (Intel + Apple Silicon, universal) | macOS 14.2 | Core Audio process taps | Planned |

On Linux and macOS a debug build of the app runs, but there is no capture backend yet: it plays a speech recording as a
fake app called `speech_8s`, which is what the end-to-end test uses.

**macOS note:** macOS releases will be signed with a Developer ID certificate and notarized by Apple, so they open like
any other app. The audio-capture permission and the steps to build on macOS (Xcode command-line tools, Rust,
Node.js) are in [docs/macos.md](docs/macos.md).

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
tray, and Quit in the tray menu ends it. On Linux and macOS there is no capture backend yet, so a debug build plays a
speech recording as a fake app, `speech_8s`. Add `REPEAT_THAT_NULL_OUTPUT=1` on a machine without an audio device.

System packages (the [Tauri prerequisites](https://tauri.app/start/prerequisites/) cover Windows and macOS):

```sh
# Debian / Ubuntu
sudo apt install pkg-config libwebkit2gtk-4.1-dev libgtk-3-dev libayatana-appindicator3-dev librsvg2-dev \
  libasound2-dev libxdo-dev
```

See [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) for how the pieces fit together.

## Contributing

Contributions are very welcome — see [CONTRIBUTING.md](CONTRIBUTING.md). Please follow our
[Code of Conduct](CODE_OF_CONDUCT.md).

## License

Repeat That is free software, licensed under the [GNU General Public License v3.0 or later](LICENSE).
