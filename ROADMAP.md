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
- [ ] Mock WAV capture backend (for developing the app on any OS)
- [ ] Enumerate audio sessions → list of apps (name, icon, PID)
- [ ] WASAPI process loopback capture (include process tree)
- [ ] Per-app volume for ducking (`ISimpleAudioVolume`)

## Milestone 4 — Desktop app → **v0.1 (Windows)**
- [ ] Tauri 2 shell, tray icon and menu
- [ ] Global hotkeys (replay/step back, stop, catch-up)
- [ ] Settings UI: app picker, buffer length, VAD sensitivity, output device, hotkeys, speed, ducking, autostart
- [ ] Windows installers from CI

## Milestone 5 — Linux
- [ ] PipeWire capture backend, app volume via PipeWire
- [ ] `.deb` package
- [ ] Wayland global shortcuts via XDG portal

## Milestone 6 — macOS
- [ ] Core Audio process tap backend (macOS 14.2+)
- [ ] Universal binary, ad-hoc signed `.dmg` (no paid Apple notarization)
- [ ] `SHA256SUMS` published with every release
- [ ] Detect a missing or reset audio-capture permission and show how to fix it

## Milestone 7 — Live transcript
- [ ] `whisper-rs` transcription of finished utterances (local models, downloaded on demand)
- [ ] Transcript panel / overlay, memory-only

## Later / ideas
- [ ] Translations (English, Dutch first)
- [ ] Auto-update
- [ ] Windows code signing (free via SignPath Foundation for OSS)
- [ ] Auto-detect which app is in an active call
- [ ] Capture multiple apps at once
- [ ] Small always-on-top mini player / waveform timeline
- [ ] Visual cue (tray icon change) when someone has been speaking while you were "away"
