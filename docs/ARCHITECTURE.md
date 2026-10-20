# Architecture

```
 ┌──────────────┐   PCM frames   ┌──────────────────────────── rt-core ────────────────────────────┐
 │  rt-capture  │ ─────────────▶ │  VAD (Silero) ──▶ SpeechBuffer (ring of utterances, RAM only)   │
 │  per-OS app  │                │                         │                                       │
 │  audio tap   │                │                         ▼                                       │
 └──────────────┘                │  Player: step-back cursor, time-stretch (catch-up), output dev  │
        ▲                        └──────────────────────────────┬──────────────────────────────────┘
        │ select app / duck                                     │ commands / state events
 ┌──────┴──────────────────────────── app (Tauri 2) ────────────┴──────────────────────────────────┐
 │  tray menu · global hotkeys · settings store · updater         Svelte UI: app picker, settings  │
 └─────────────────────────────────────────────────────────────────────────────────────────────────┘
```

## Crates

### `rt-capture` — "give me the audio of process X"
One trait, several backends selected with `cfg(target_os)`:

```rust
pub trait AppAudioSource: Send + Sync {
    fn list_apps(&self) -> Result<Vec<AudioApp>>;           // apps with an audio session (name, PID, exe path)
    fn start(&self, app: &AudioApp, sink: FrameSink) -> Result<Box<dyn CaptureHandle>>;
    fn set_app_volume(&self, app: &AudioApp, level: f32) -> Result<()>;  // ducking, relative to the user's volume
}
```

`platform_source()` returns the backend for the current OS.

| Backend | API | Notes |
|---|---|---|
| Windows | WASAPI `ActivateAudioInterfaceAsync` with `AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK` (`windows` crate) | Win10 2004+. Uses `PROCESS_LOOPBACK_MODE_INCLUDE_TARGET_PROCESS_TREE` — Discord/Teams/browsers play audio from child processes. Captures 48 kHz stereo `f32`; the audio engine converts. |
| Linux | PipeWire (`pipewire` crate): connect a capture stream to the app's output node | PulseAudio-only systems are out of scope initially. |
| macOS | Core Audio process taps (`AudioHardwareCreateProcessTap`, macOS 14.2+) via `objc2` | Needs the audio-capture TCC permission and an `NSAudioCaptureUsageDescription`. |
| Mock | Plays a WAV file (or samples in memory) as if it were an app | Used for tests and development on any OS. |

**Apps are process trees.** The app list groups audio sessions by their top-most process: a session's process is
walked up through parents with the same executable (Discord, Chrome), and from helpers like WebView2 to the process that
started them (new Teams). That top-most PID is what gets captured, including its whole tree.

Ducking (`set_app_volume`) also lives here, because it is per-OS and per-app (Windows: `ISimpleAudioVolume` on every
audio session in the app's process tree). The level is relative: the backend remembers each session's volume before
ducking and restores it at `1.0`, so the user's own per-app volume is kept. The lowest level is 5%: a muted app can't
be captured. On Windows, process loopback captures the app *after* its volume, so captures of a ducked app are
amplified by `1 / level`. Speech that arrives during a replay is still buffered at full level. Windows remembers
per-app volumes, so ducked apps are restored however the app ends: on a normal exit, dropping the source restores them;
after a crash or kill, a silent duck guard process (`WasapiSource::with_duck_guard`, the app's own executable started
with hidden arguments) restores them as soon as the app is gone; after a power loss, or if the guard died too, the next
start restores them from the duck journal (`WasapiSource::with_duck_journal`, volumes and session identifiers only,
never audio). Sessions whose volume the user changed by hand meanwhile are left alone.

`cargo run -p rt-capture --example probe -- list | capture <app> [--duck] | duck <app>` tries the backend on real
apps without writing audio to disk.

### `rt-core` — platform-independent logic
- **Downmix and resampling**: captured audio is mixed to mono (kept at the capture rate for playback) and resampled
  to 16 kHz for the VAD.
- **VAD**: Silero VAD, bundled (`models/silero_vad.onnx`, MIT) and run with ONNX Runtime through `ort`, which is
  statically linked, so nothing extra ships next to the app. Frames are classified as speech or non-speech, with hangover/padding (~200 ms) so that word
  edges aren't clipped.
- **`SpeechBuffer`**: an in-memory ring of *utterances* (contiguous speech segments), capped at a configurable
  total *speech* duration (default 30 s). The oldest utterances are evicted first. Silences between utterances are
  replaced by a short fixed gap (~250 ms) on playback so it still sounds natural.
- **`Player`**: plays from a cursor positioned at an utterance boundary.
  - *Replay*: jump to the start of the latest utterance. Pressing again within a short window steps one more
    utterance back.
  - *Catch-up*: playback speed from 1.0× to 2.0× with pitch-preserving time-stretch (WSOLA).
  - Recording keeps running during playback. Speech that arrives during a replay is appended and can be played through
    to "live".

### `rt-transcribe` (planned)
`whisper-rs` (whisper.cpp) on utterances as they finish, producing a scrollable "what was just said" transcript.
Models are downloaded on demand; transcripts follow the same memory-only rule.

### `app` — Tauri 2 desktop shell
- Tray icon with menu: *Replay*, *Stop*, *Catch-up*, *Pause capture*, *Settings*, *Quit*.
- Global hotkeys via `tauri-plugin-global-shortcut` (the tray menu is the fallback where global shortcuts are
  unavailable, e.g. some Wayland compositors).
- Settings via `tauri-plugin-store`: target app, buffer length, VAD sensitivity, output device, hotkeys, speed, duck
  level, autostart.
- Svelte + TypeScript UI in `app/src`, built with Vite.

## Threading
- Capture backends deliver frames on their own real-time thread into a lock-free SPSC queue.
- A worker thread drains the queue, runs resample + VAD, and appends to `SpeechBuffer` (behind a `Mutex`; contention is
  low because only the player reads it).
- Playback runs on the `cpal` output callback, pulling from a pre-rendered (time-stretched) queue fed by the player
  thread.

## Releases
GitHub Actions builds:
- Windows: NSIS `.exe` + `.msi` (x64, ARM64)
- Linux: `.deb` (+ AppImage later)
- macOS: universal `.dmg`, **ad-hoc signed** (`bundle.macOS.signingIdentity = "-"`) and **not notarized**: the project
  doesn't use a paid Apple Developer account. Apple Silicon refuses to run unsigned code at all, so the ad-hoc signature
  is needed. Users open the app once via *Open Anyway* or by removing the quarantine flag. See
  [install-macos.md](install-macos.md).
- Every release publishes a `SHA256SUMS` file so users can verify downloads before bypassing Gatekeeper.

Ad-hoc signatures change with every build, so macOS may ask for the audio-capture permission again after an update. The
app should detect that capture returns no audio and point the user to the permission setting.

The Tauri updater verifies updates with its own key pair (stored as a GitHub Actions secret), which works without Apple
signing.

Open-source code signing on Windows: apply to [SignPath Foundation](https://signpath.org).
