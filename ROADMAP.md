# Roadmap

## Milestone 1 — Bootstrap ✅
- [x] License (GPL-3.0-or-later), README, privacy notes, contributing guide, code of conduct
- [x] Architecture doc, roadmap, issue templates
- [x] Cargo workspace skeleton and CI (fmt, clippy, test on Linux/Windows/macOS)

## Milestone 2 — Core engine (`rt-core`) ✅
- [x] Resampler to 16 kHz mono for the VAD
- [x] Silero VAD integration (bundled model, ONNX Runtime) with padding, hysteresis and pause tolerance
- [x] `SpeechBuffer`: utterance ring, capped by speech duration, stable ids
- [x] `Player`: replay latest, step back per press, play through to live, gaps between utterances
- [x] Pitch-preserving time-stretch (WSOLA) for catch-up (0.5–2.0×)
- [x] Tests with synthetic audio and a real speech fixture; `trim_wav` example to hear the result

## Milestone 3 — Windows capture
- [x] Mock WAV capture backend (for developing the app on any OS)
- [x] Enumerate audio sessions → list of apps (name, exe path for the icon, PID), grouped by process tree
- [x] WASAPI process loopback capture (include process tree)
- [x] Per-app volume for ducking (`ISimpleAudioVolume`), faded, with the capture kept at full level
- [x] Ducked apps restored after a crash (duck guard process, duck journal)
- [x] End-to-end test on a virtual audio device in CI
- [x] Verified on real Windows hardware with the `probe` example (Discord, Teams, a browser)

## Milestone 4 — Desktop app → **v0.1 (Windows)**
- [x] Tauri 2 shell, tray icon and menu
- [x] Global hotkeys (replay/step back, stop, catch-up)
- [x] App icons extracted from the executable for the app picker
- [x] Settings UI: app picker, buffer length, VAD sensitivity, output device, hotkeys, speed, ducking, autostart
- [x] Windows installer from CI, published with a `SHA256SUMS` file

## Milestone 5 — Linux
- [x] PipeWire capture backend, app volume via PipeWire, tested against a real PipeWire in CI
- [x] Wayland global shortcuts via XDG portal (KDE Plasma, GNOME 48+), command-line actions for desktops without it
- [x] `.deb` package (Debian 13+, Ubuntu 24.04+)
- [x] `.rpm` package (Fedora 40+)
- [x] AppImage
- [x] Packages installed and started in CI on Debian 13, Ubuntu 24.04, Fedora 40 and Fedora 44 (Debian 12 is too old
  for the speech detector's ONNX Runtime, and CI checks that apt refuses the `.deb` there)
- [x] The packages are listed in the release's `SHA256SUMS` (checked in a dry run of the publish workflow and a
  rehearsal release; no real release has had them yet)
- [x] Flatpak (`Repeat-That_<version>_x86_64.flatpak`) built in CI from `packaging/flatpak`, checked and linted as for
  Flathub, and listed in the release's `SHA256SUMS`
- [ ] Flatpak on Flathub (the files are ready, see [Flathub](docs/MAINTAINING.md#flathub); the submission and the
  verification of `roboroads.me` are the maintainer's)

## Milestone 6 — macOS (built, not yet tested on real hardware)
Nobody on the team has a Mac, so macOS is released as untested: GitHub's macOS runners are the only test so far, and the
release notes, README, settings window and [install guide](docs/install-macos.md) say so and link to a
[macOS report form](https://github.com/Roboroads/repeat-that/issues/new?template=macos.yml).

- [x] Core Audio process tap backend (macOS 14.2+), tested on a macOS runner with a virtual sound card
- [x] Apple Silicon `.dmg`, signed with a Developer ID Application certificate and notarized (Apple Silicon only: Microsoft
  ships no x86_64 or universal ONNX Runtime after 1.23.2)
- [x] The `.dmg` itself is signed, notarized and stapled, not only the `.app` inside it
- [ ] Capture works under the hardened runtime in a notarized build (`NSAudioCaptureUsageDescription`, entitlements as
  needed). The build has both, and its self-check passes, but no notarized build has captured audio yet
- [x] The `.dmg` is listed in the release's `SHA256SUMS` (checked in a dry run of the publish workflow; no real release
  has had one yet)
- [x] Detect a missing or revoked audio-capture permission and show how to fix it
- [x] Ducking without a per-app volume: the app is muted and its audio replayed at the duck level, down to 0%
- [x] "Untested" notices and the macOS issue form
- [ ] Try it on a real Mac (see the
  [checklist](docs/install-macos.md#testing-checklist-if-you-have-a-mac)), then remove the "untested" notices

## Milestone 7 — Live transcript
- [ ] `whisper-rs` transcription of finished utterances (local models, downloaded on demand)
- [ ] Transcript panel / overlay, memory-only

## Milestone 8 — Website and going public
This milestone starts after milestone 4, because the site needs the v0.1 release to link to, and can run in
parallel with 5–7.

- [ ] Make the repository public, following the [going public checklist](docs/MAINTAINING.md#going-public-checklist) (on a free plan, GitHub Pages needs a public repository)
- [ ] GitHub Pages site: landing page that says what the app does, download links to the latest release (whose `SHA256SUMS` ships with the installers from milestone 4), and a link to [how to verify a download](docs/verify-downloads.md)
- [ ] Short demo video of a replay in a call (Discord or Teams)
- [ ] How-to-use guide: install per OS, picking an app, hotkeys, catch-up, ducking and a privacy summary
- [ ] Apply for free open-source code signing from SignPath Foundation (Windows), with the website as the project page

The demo video never contains audio captured from someone else's call. It uses the mock backend with synthetic or
licensed speech, or a staged conversation recorded outside the app with everyone's consent. That is a consented
exception for marketing material only, and the app itself still never writes audio to disk (see
[PRIVACY.md](PRIVACY.md)). Large media stays out of git history, Pages-only branches included: the video and any other
big files go to release assets or an external host. The site plays the video from a `<video>` tag on a release asset, or
from a `youtube-nocookie.com` embed that only loads after a click, so opening the page contacts no third party.

## Later / ideas
- [ ] Translations (English, Dutch first)
- [ ] Auto-update
- [ ] Auto-detect which app is in an active call
- [ ] Capture multiple apps at once
- [ ] Small always-on-top mini player / waveform timeline
- [ ] Visual cue (tray icon change) when someone has been speaking while you were "away"
