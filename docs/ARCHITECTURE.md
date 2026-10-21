# Architecture

```
 ┌──────────────┐   PCM frames   ┌──────────────────────────── rt-core ────────────────────────────┐
 │  rt-capture  │ ─────────────▶ │  VAD (Silero) ──▶ SpeechBuffer (ring of utterances, RAM only)   │
 │  per-OS app  │                │                         │                                       │
 │  audio tap   │                │                         ▼                                       │
 └──────────────┘                │  Player: step-back cursor, time-stretch (catch-up), to live     │
        ▲                        └──────────────────────────────┬──────────────────────────────────┘
        │ capture / duck                                        │ render
 ┌──────┴──────────────────── rt-engine (its own threads) ──────┴──────────────────────────────────┐
 │  capture ▶ worker ▶ buffer · player ▶ output device (cpal) · duck · reconnect · status          │
 └────────────────────────────────────────────────┬────────────────────────────────────────────────┘
                                                  │ commands / status
 ┌──────────────────────────────────────── app (Tauri 2) ──────────────────────────────────────────┐
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

Windows doesn't say when a volume change reaches the captured audio (process loopback reports no stream latency), so
the compensation can only switch at an estimate, which is milliseconds off. Ducking therefore fades the volume in
half-dB steps on a 10 ms grid, and each captured frame is amplified for the step it was played at, so being off leaves
the capture a step or so off for a few milliseconds, which is inaudible. Restoring fades back the same way, and
`set_app_volume` returns once the fade is done. What CI measured, and the fade's other rules, are in
`crates/rt-capture/src/ramp.rs`; the end-to-end test checks the result per millisecond.

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
    utterance, unless that ended over 10 s ago and is unrelated to what's said now. Pressing again during a replay
    steps one more utterance back.
  - *Play through to live*: after each utterance it plays the next, including everything recorded during the replay.
    Silences between utterances are replaced by a short fixed gap (~250 ms), so it still sounds natural and catches up
    even at 1.0×. When it reaches someone still talking, it follows their sentence as it comes in, at normal speed,
    waiting in silence until ~100 ms of new speech is in each time; it also waits while speech is pending. Waiting
    gives up after 2 s without new audio, in case capture stopped without closing the sentence. It ends once there is
    nothing left, and the listener is back to live audio. `Player::behind` says how long that takes.
  - *Catch-up*: playback speed from 0.5× to 2.0× with a streaming, pitch-preserving time-stretch (WSOLA,
    `Stretcher`). A speed change applies mid-utterance within 15 ms, without a click.

### `rt-engine` — capture, buffer and playback on their own threads
`Engine::new(source, output, config)` takes an `AppAudioSource`, an `OutputBackend` and an `EngineConfig` (buffer
length, sensitivity, replay and catch-up speed, duck level, output device) and starts the threads described under
[Threading](#threading). `engine.handle()` gives a cloneable `EngineHandle`: `set_target`, `set_paused`, `replay`,
`stop`, `toggle_catch_up` and `set_config` return at once; `list_apps` and `list_output_devices` block. `subscribe()`
delivers the `Status` (capture state, target, playback, how far behind live, buffered speech, output device or error,
VAD fallback) on every change and every 500 ms during a replay; a receiver that falls 64 statuses behind is
disconnected.

- **Target app**: remembered by executable path (case-insensitive on Windows), or by name when there is none (the mock
  source), never by PID, which changes when the app restarts. Without a running capture the control thread looks for
  it every 2 s: *waiting* while it's missing, *capturing* once found, *error* (retried) if the capture fails to start,
  *unsupported* (not retried) where capture can't work. When the app quits, the sentence in progress is closed and the
  buffer kept, so a replay still works while it restarts.
- **Replay**: the output stream is opened per replay (the chosen device, else the default) and closed once the replay
  caught up with live and its tail played, so the app doesn't hold the device while idle and follows default-device
  changes. A device that fails mid-replay is replaced by the default, from where the player was. With nothing buffered
  a soft beep plays instead (two 80 ms tones at about -18 dBFS, generated in code).
- **Ducking**: a replay ducks the app (20% by default) as it starts; the volume comes back 300 ms after the replay ends
  or stops, unless another replay starts within those 300 ms. Without any output device the replay is aborted and
  nothing is ducked.
- **Catch-up**: toggles between catch-up speed (1.5×) and replay speed mid-replay; from idle it starts a replay at
  catch-up speed. It resets when the replay ends.
- **Wipes**: pausing, choosing another app and dropping the engine stop capture and playback, give the app its volume
  back, and wipe the buffer. A capture in another format (sample rate, channels) starts a new buffer.
- **Settings apply live**: a smaller buffer evicts at once, a new sensitivity applies from the next VAD frame, neither
  wipes anything. Speeds apply at once; the duck level and the output device at the next replay.
- **Outputs**: `CpalOutput` (the sound card), `NullOutput` (real-time, discarded; for machines without audio) and
  `TestOutput` (collected in memory, optionally faster than real time; for tests).

`cargo run -p rt-engine --example engine_probe -- <app>` runs the engine on a real app (Windows; elsewhere the
fixture plays as a mock app) and replays on Enter.

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
All of it lives in `rt-engine`; the app only calls `EngineHandle` and listens to the status.

```
capture thread (rt-capture) --FrameSink--> rtrb queue, 2 s (full: dropped and counted, never blocks)
worker thread:  drains every 10 ms --> Recorder::push in 50 ms parts --> SpeechBuffer   (Mutex<Recorder>)
player thread:  Player::render 10 ms blocks --> resample to the device rate --> rtrb queue, 80 ms ahead
cpal callback:  pops mono, copies it to every channel, silence when the queue runs dry
control thread: target, pause, reconnect poll every 2 s, capture start/stop (blocking OS calls)
duck thread:    the blocking set_app_volume fades, latest level wins
```

- The capture callback and the output callback never block or allocate: they only push to and pop from lock-free
  single-producer, single-consumer queues (`rtrb`). A change of the capture format goes into a second queue with the
  position in the stream where it happens, so the worker reads every sample in its own format.
- Pausing or choosing another app wipes the buffer before the capture stops (which can take a while), and the worker
  drops what the stopping capture still delivers, so a replay pressed meanwhile finds nothing.
- A duck that fails (other than for an app that's gone) is retried every 500 ms. When the target reconnects with a new
  PID during a replay, the new process is ducked too.
- The recorder sits behind a `Mutex` shared by the worker and the player thread. Each holds it for one 50 ms part or
  one 10 ms block, so neither waits long.
- Listing apps, starting a capture (seconds on Windows) and ducking (a fade of a few hundred milliseconds) block, so
  they have threads of their own and never hold up a replay or a hotkey.

## Releases
GitHub Actions builds:
- Windows: NSIS `.exe` (per-user install, no admin rights), x64 only for v0.1 (ARM64 later: `ort` has no CPU build for it yet)
- Linux: `.deb` (+ AppImage later)
- macOS: universal `.dmg`, signed with a Developer ID Application certificate and notarized with `notarytool` (the
  ticket is stapled to the app), so Gatekeeper opens it without extra steps. The certificate and notarization
  credentials are GitHub Actions secrets, listed in [MAINTAINING.md](MAINTAINING.md#macos-signing-and-notarization).
- Every release publishes a `SHA256SUMS` file so users can verify downloads
  ([verify-downloads.md](verify-downloads.md)).

macOS ties the audio-capture permission to the app's signing identity. A Developer ID signature stays the same from
one release to the next, so an update doesn't reset the permission. The user can still deny it or revoke it later, so
the app should detect that capture returns no audio and point the user to the permission setting.

Milestone 6 has to make capture work under the hardened runtime, which notarization requires and Tauri turns on by
default (`bundle.macOS.hardenedRuntime`). Apple's
[documentation for Core Audio taps](https://developer.apple.com/documentation/coreaudio/capturing-system-audio-with-core-audio-taps)
asks for the `NSAudioCaptureUsageDescription` key in `Info.plist` (Tauri merges `app/src-tauri/Info.plist` into the
bundle's) and names no entitlement. The sample app [AudioCap](https://github.com/insidegui/AudioCap) uses taps with the
hardened runtime and ships `com.apple.security.device.audio-input`, but it is also sandboxed, so that doesn't prove the
entitlement is needed here. Put entitlements in `app/src-tauri/Entitlements.plist`, set through
`bundle.macOS.entitlements`, test on a signed, notarized build, and keep only what capture turns out to need.

Tauri signs the `.dmg` with the same identity, but it notarizes and staples only the `.app` inside it. Milestone 6 also
has to notarize and staple the `.dmg` in the release workflow (`xcrun notarytool submit` with the same API key, then
`xcrun stapler staple`), and check the file that gets uploaded:
`spctl -a -t open --context context:primary-signature -v` and `xcrun stapler validate` on the `.dmg`, and
`xcrun stapler validate` on the app inside.

There is no auto-updater yet (it is on the roadmap under later ideas). When it arrives, the Tauri updater will verify
updates with its own key pair, stored as a GitHub Actions secret and separate from any code signing.

The Windows installer is not code signed yet, so Windows shows a SmartScreen warning. Open-source code signing on
Windows: apply to [SignPath Foundation](https://signpath.org).
