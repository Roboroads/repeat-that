# Roadmap

## Milestone 1 — Bootstrap ✅
- [x] License (GPL-3.0-or-later), README, privacy notes, contributing guide, code of conduct
- [x] Architecture doc, roadmap, issue templates
- [x] Cargo workspace skeleton and CI (fmt, clippy, test on Linux/Windows/macOS)

## Milestone 2 — Core engine (`rt-core`)
- [ ] Resampler to 16 kHz mono
- [ ] Silero VAD integration with padding/hangover
- [ ] `SpeechBuffer`: utterance ring, capped by speech duration
- [ ] `Player`: replay latest, step back per press, play through to live
- [ ] Pitch-preserving time-stretch for catch-up (1.0–2.0×)
- [ ] Mock WAV capture backend + fixture-based tests

## Milestone 3 — Windows capture
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
- [ ] Universal binary, signed and notarized `.dmg`

## Milestone 7 — Live transcript
- [ ] `whisper-rs` transcription of finished utterances (local models, downloaded on demand)
- [ ] Transcript panel / overlay, memory-only

## Later / ideas
- [ ] Translations (English, Dutch first)
- [ ] Auto-update
- [ ] Code signing (SignPath for Windows)
- [ ] Auto-detect which app is in an active call
- [ ] Capture multiple apps at once
- [ ] Small always-on-top mini player / waveform timeline
- [ ] Visual cue (tray icon change) when someone has been speaking while you were "away"
