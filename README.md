# Repeat that?!

**Missed what someone just said on a call? Press a key and hear it again.**

Repeat that?! keeps a short rolling buffer of the audio coming out of one app — Discord, Teams, Zoom, Slack, your browser —
skips the silence, and replays the last thing that was said whenever you press a hotkey. No awkward
"sorry, could you repeat that?" needed.

It's built for people whose attention wanders during calls (hello, ADHD 👋), but it's useful for anyone dealing with bad
connections, accents, background noise, or just a moment of distraction.

> **Status:** early development. Nothing to install yet — see the [roadmap](ROADMAP.md).

## Features

| Feature | Status |
|---|---|
| Capture audio from a single chosen application | 🚧 MVP (Windows first) |
| Rolling buffer of the last *X* seconds of **speech** (silence trimmed via voice activity detection) | 🚧 MVP |
| Global hotkey to replay; press again to step back one more sentence | 🚧 MVP |
| Keeps recording while you replay, so you don't miss what comes next | 🚧 MVP |
| Catch-up mode: replay at 1.25–2× speed (pitch-preserved), then back to live | 🚧 MVP |
| Duck or mute the call while replaying | 🚧 MVP |
| Choose which output device the replay plays on | 🚧 MVP |
| Tray icon & tray menu controls | 🚧 MVP |
| Live transcript of recent speech (local Whisper, fully offline) | 🗺️ Planned |
| Linux (PipeWire) and macOS support | 🗺️ Planned |

## Platform support

| Platform | Minimum version | Capture method | Status |
|---|---|---|---|
| Windows (x64, ARM64) | Windows 10 2004 / 11 | WASAPI process loopback | 🚧 In progress |
| Linux (Debian/Ubuntu, `.deb`) | Debian 12 / Ubuntu 22.04 with PipeWire | PipeWire per-stream capture | 🗺️ Planned |
| macOS (Intel + Apple Silicon, universal) | macOS 14.2 | Core Audio process taps | 🗺️ Planned |

**macOS note:** macOS releases will be signed with a Developer ID certificate and notarized by Apple, so they open like
any other app. The audio-capture permission and the steps to build on macOS (Xcode command-line tools, Rust,
Node.js) are in [docs/macos.md](docs/macos.md).

Releases starting with v0.1 include a `SHA256SUMS` file. See [docs/verify-downloads.md](docs/verify-downloads.md) to
check a download.

## Privacy

Recorded audio **only ever lives in memory**. It is never written to disk, never sent anywhere, and is wiped as soon as you
stop capturing or quit the app. Transcription (when it lands) runs locally on your machine. See [PRIVACY.md](PRIVACY.md),
including a note on recording laws.

## Building from source

The app is a [Tauri 2](https://tauri.app) application with a Rust core and a Svelte + TypeScript UI.

```sh
# Rust crates and tests (any OS)
cargo test --workspace

# The desktop app, with live reload (needs Node.js 22 LTS or later and the system packages below)
cd app
npm install
npm run tauri dev
```

The app starts in the tray. The first launch also opens the settings window; closing it keeps the app running in the
tray, and Quit in the tray menu ends it. Capture and replay are not wired up yet.

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

Repeat that?! is free software, licensed under the [GNU General Public License v3.0 or later](LICENSE).
