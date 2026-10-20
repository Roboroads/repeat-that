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
- [ ] Verified on real Windows hardware with the `probe` example (Discord, Teams, a browser)

## Milestone 4 — Desktop app → **v0.1 (Windows)**
- [ ] Tauri 2 shell, tray icon and menu
- [ ] Global hotkeys (replay/step back, stop, catch-up)
- [ ] App icons extracted from the executable for the app picker
- [ ] Settings UI: app picker, buffer length, VAD sensitivity, output device, hotkeys, speed, ducking, autostart
- [ ] Windows installers from CI, published with a `SHA256SUMS` file

## Milestone 5 — Linux
- [ ] PipeWire capture backend, app volume via PipeWire
- [ ] `.deb` package
- [ ] Wayland global shortcuts via XDG portal

## Milestone 6 — macOS
- [ ] Core Audio process tap backend (macOS 14.2+)
- [ ] Universal binary, `.dmg` signed with a Developer ID Application certificate and notarized
- [ ] The `.dmg` itself is signed, notarized and stapled, not only the `.app` inside it
- [ ] Capture works under the hardened runtime in a notarized build (`NSAudioCaptureUsageDescription`, entitlements as needed)
- [ ] The `.dmg` is listed in the release's `SHA256SUMS`
- [ ] Detect a missing or revoked audio-capture permission and show how to fix it

## Milestone 7 — Live transcript
- [ ] `whisper-rs` transcription of finished utterances (local models, downloaded on demand)
- [ ] Transcript panel / overlay, memory-only

## Milestone 8 — Website and going public
This milestone can run in parallel with 5–7.

- [ ] GitHub Pages site: landing page that says what the app does, download links to the latest release, and a link to `SHA256SUMS` with how to verify it
- [ ] Short demo video of a replay in a call (Discord or Teams), made with the [brag](https://github.com/latent-spaces/brag) plugin
- [ ] How-to-use guide: install per OS, picking an app, hotkeys, catch-up, ducking and a privacy summary
- [ ] Make the repository public, following the [going public checklist](docs/MAINTAINING.md#going-public-checklist)
- [ ] Apply for free open-source code signing from SignPath Foundation (Windows), with the website as the project page

The demo video uses a staged conversation between people who agreed to be in it. It never contains audio captured from
a real call with someone else (see [PRIVACY.md](PRIVACY.md)). Large media stays out of git history: the video is hosted
as a release asset or on YouTube, and any other big files go on a Pages-only branch.

## Later / ideas
- [ ] Translations (English, Dutch first)
- [ ] Auto-update
- [ ] Auto-detect which app is in an active call
- [ ] Capture multiple apps at once
- [ ] Small always-on-top mini player / waveform timeline
- [ ] Visual cue (tray icon change) when someone has been speaking while you were "away"
