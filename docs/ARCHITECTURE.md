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
amplified by `1 / level`. Speech that arrives during a replay is still buffered at full level.

Windows doesn't say when a volume change reaches the captured audio (process loopback reports no stream latency; on CI
it lands 27–37 ms after the call), so the compensation can only switch at an estimate. Ducking therefore fades the
volume in steps of under 1 dB, one every 10 ms (130 ms for a duck to 25%, up to 290 ms for 5%), and each captured frame
is amplified for the step it was played at. A few milliseconds of error then leave the capture one step off at most,
which is inaudible; CI checks that no millisecond strays by more than 1.5 dB. Restoring fades back the same way, and
`set_app_volume` returns once the fade is done.

Windows remembers per-app volumes, so ducked apps are restored however the app ends: on a normal exit, dropping the
source restores them; after a crash or kill, a silent duck guard process (`WasapiSource::with_duck_guard`, the app's
own executable started with hidden arguments) restores them as soon as the app is gone; after a power loss, or if the
guard died too, the next start restores them from the duck journal (`WasapiSource::with_duck_journal`, volumes and
session identifiers only, never audio). Sessions whose volume the user changed by hand meanwhile are left alone.

`cargo run -p rt-capture --example probe -- list | capture <app> [--duck] | duck <app>` tries the backend on real
apps without writing audio to disk.

### `rt-core` — platform-independent logic
- **Downmix and resampling**: captured audio is mixed to mono (kept at the capture rate for playback) and resampled
  to 16 kHz for the VAD.
- **VAD**: Silero VAD, bundled (`models/silero_vad.onnx`, MIT) and run with ONNX Runtime through `ort`, which is
  statically linked, so nothing extra ships next to the app. Frames are classified as speech or non-speech, with hangover/padding (~200 ms) so that word
  edges aren't clipped.
- **`Segmenter`**: splits the stream into *utterances* (contiguous speech segments) while they are spoken. Once
  speech reaches 250 ms, an utterance opens and audio is appended to it as it comes in; shorter bursts (a cough, a
  click) are dropped. A pause longer than 700 ms closes it, trimmed to the padding. A monologue is split every 15 s,
  where the speech goes on, so stepping back stays fine-grained; replay plays the parts on without a gap. Silence
  inside a pause is held back until speech resumes or the utterance closes, so nothing published is ever taken back.
  Detection settings (sensitivity) can change while recording, without wiping anything.
- **`SpeechBuffer`**: an in-memory ring of utterances, capped at a configurable total *speech* duration (default
  30 s). The oldest utterances are evicted first. The newest one can be *open*: the sentence still being spoken, which
  is never evicted. A *speech pending* flag says speech has started that isn't long enough to be an utterance yet.
- **`Player`**: renders the replay in small blocks from a position inside the current utterance.
  - *Replay*: starts at the sentence in progress if it already has 1.5 s of audio, otherwise at the last finished
    utterance. Pressing again during a replay steps one more utterance back.
  - *Play through to live*: after each utterance it plays the next, including everything recorded during the replay.
    Silences between utterances are replaced by a short fixed gap (~250 ms), so it still sounds natural and catches up
    even at 1.0×. When it reaches someone still talking, it follows their sentence as it comes in, at normal speed,
    waiting in silence until ~100 ms of new speech is in each time; it also waits while speech is pending. It ends
    once there is nothing left, and the listener is back to live audio. `Player::behind` says how long that takes.
  - *Catch-up*: playback speed from 0.5× to 2.0× with a streaming, pitch-preserving time-stretch (WSOLA,
    `Stretcher`). A speed change applies mid-utterance within 15 ms, without a click.

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
