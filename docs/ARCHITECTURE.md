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

`platform_source()` returns the backend for the current OS, and `platform_source_with(PlatformOptions { duck_journal })`
does so with the options each OS decides what to do with (Windows and Linux: a duck journal, which also starts the duck
guard, except in a Flatpak; macOS ignores them, see [macOS](#macos)).
`HAS_PLATFORM_BACKEND` says whether this OS has one at all.

**One module per OS.** `lib.rs` includes exactly one OS module under the name `os`: `windows`, `linux`, `macos`, or
`unsupported` on an OS without a backend (it refuses everything, and `HAS_PLATFORM_BACKEND` is false there). Every OS module has the same
items, and the platform-neutral entry points only call these:

| Item | Used by |
|---|---|
| `HAS_BACKEND: bool` | `HAS_PLATFORM_BACKEND`, which the app uses to choose between the backend, a mock and "unsupported" |
| `HAS_DUCK_GUARD: bool` | `duck_guard::run_if_requested()`: where there is no guard, a request for one is left to the app |
| `EXE_PATHS_IGNORE_CASE: bool` | `same_exe_path()`, which the engine uses to find its target app again (`true` on Windows) |
| `source(PlatformOptions) -> Result<Box<dyn AppAudioSource>>` | `platform_source()` and `platform_source_with()` |
| `icon(exe, size) -> Option<IconImage>` | `app_icon()` |
| `run_duck_guard(journal, parent) -> bool` | `duck_guard::run_if_requested()`, which `main` calls first thing; `true` once a guard has run |

A new backend adds its module, a `use <module> as os;` line and its OS to the `not(any(..))` of `unsupported` in
`lib.rs`, and nothing else in the platform-neutral code. A `pub mod` gives tests and tools the backend's own types
(`rt_capture::windows::WasapiSource`, `rt_capture::linux::PipeWireSource`, `rt_capture::macos::CoreAudioSource`).

When the OS refuses capture, a backend returns `CaptureError::PermissionDenied`, whose message says how to allow it
(its `Display` puts "audio capture is not allowed:" in front, like the other variants name their kind). The engine
turns it into its own state, *not allowed*, which is retried like an error and keeps the buffer (see
[`rt-engine`](#rt-engine--capture-buffer-and-playback-on-their-own-threads)). If the backend refuses already while the
app sets it up, the app's source sets it up again on its next use (the engine's reconnect poll, an app list), so capture
starts as soon as the permission is given. `Unsupported` is final. Only macOS asks the user for this permission;
`rt_capture::permission` has the calls for it on every OS (`status`, `help`, `ask_again`, `open_settings`), which
return `Unsupported` everywhere else, so they can be tested on Linux.

| Backend | API | Notes |
|---|---|---|
| Windows | WASAPI `ActivateAudioInterfaceAsync` with `AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK` (`windows` crate) | Win10 2004+. Uses `PROCESS_LOOPBACK_MODE_INCLUDE_TARGET_PROCESS_TREE` — Discord/Teams/browsers play audio from child processes. Captures 48 kHz stereo `f32`; the audio engine converts. |
| Linux | PipeWire 0.3.65+ (`pipewire` crate): a capture stream of our own, linked by hand to the output ports of the app's playback streams | Apps that play through the PulseAudio API (most) come through pipewire-pulse. Systems with only PulseAudio get "Capture needs PipeWire". Captures stereo `f32` at the graph rate. |
| macOS | Core Audio process taps (`AudioHardwareCreateProcessTap`, macOS 14.2+) via `objc2`: a private stereo tap of the app's processes, inside a private aggregate device on the app's own output device | Needs the audio-capture TCC permission and an `NSAudioCaptureUsageDescription`. Apple Silicon only. Below macOS 14.2 every call returns `Unsupported`. There is no per-app volume, so ducking replays the app's audio ourselves, see [macOS](#macos). |
| Mock | Plays a WAV file (or samples in memory) as if it were an app | Used for tests and development on any OS. |

**Apps are process trees.** The app list groups audio sessions by their top-most process: a session's process is
walked up through parents with the same executable (Discord, Chrome), and from helpers like WebView2 to the process that
started them (new Teams). That top-most PID is what gets captured, including its whole tree.

Ducking (`set_app_volume`) also lives here, because it is per-OS and per-app (Windows: `ISimpleAudioVolume` on every
audio session in the app's process tree). The level is relative: the backend remembers each session's volume before
ducking and restores it at `1.0`, so the user's own per-app volume is kept. The lowest level is 5% (`MIN_DUCK_LEVEL`): a
muted app can't be captured. The platform's lowest level is `PLATFORM_MIN_DUCK_LEVEL`, which is `MIN_DUCK_LEVEL`
everywhere but macOS, where it is 0 (see [macOS](#macos)); the app takes its slider and its settings limit from it, and
a settings file from a Mac that holds 0 is raised to 5% on the other systems. On Windows, process loopback captures the
app *after* its volume, so captures of a ducked app are amplified by `1 / level`. Speech that arrives during a replay is
still buffered at full level.

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

**Linux.** The source talks to PipeWire from a thread of its own, which keeps the registry, every playback stream's
volume, the capture streams and their links; the trait's calls send it commands and wait for the answer (5 s at most).
No PipeWire socket means `Unsupported` ("Capture needs PipeWire"), a socket that doesn't answer a `Backend` error the
engine retries; a capture whose connection drops ends, and the next call connects again.

- *The app list.* Every `Stream/Output/Audio` node is a stream of some app. Its PID is the one the daemon saw on the
  client's socket (`pipewire.sec.pid`), except for PulseAudio apps, whose client is pipewire-pulse: there it is the PID
  the app gives (`application.process.id`), if `/proc` agrees about its binary. Where `/proc` shows the process, the
  app is its process tree, as on Windows; otherwise (in a Flatpak, or a PulseAudio app inside one) the app is every
  stream with the same `application.process.binary`, else `application.name`, and has no executable. Names come from
  the `.desktop` file of the executable or binary, else `application.name`. Our own process tree isn't listed.
- *Capture.* Our capture stream never connects on its own; the source links the output ports of the app's streams to
  it (mono to both sides, surround folded into stereo) and links new streams as they come. Only ports of
  `Stream/Output/Audio` nodes are ever linked, so a microphone or any other source never ends up in a capture; a test
  checks this with a virtual microphone playing a tone. The capture ends once the app's streams are gone and its
  process too.
- *Ducking.* A stream's volume is its node's `channelVolumes`, scaled alike so it keeps its balance; mute is never
  touched. Streams apply their volume before their output ports, so the capture is ducked too and compensated like on
  Windows, but PipeWire applies a volume from the next graph cycle on, so the fade steps once per cycle (its length,
  measured by the capture) and each cycle gets one gain. An asynchronous stream (`node.async`, like `pw-cat`) made the
  audio we capture in the cycle before, so its gain is the level from one cycle earlier. CI's measurements are in
  `ramp.rs`. A stream the app opens during a duck is ducked as soon as its volume is known.
- *What WirePlumber remembers.* WirePlumber stores each stream's volume under a key (its media role, else app ID, app
  name, media name or node name) and gives it to the next stream with that key. A stream that comes up at the ducked
  volume of one of ours with the same key is taken as ducked from that one's original, so it is neither ducked twice
  nor left down. The duck journal names streams by that key, so a stream that comes back ducked after a crash is found
  and restored once it has played for a moment (WirePlumber sets the remembered volume a moment after the stream
  appears).
- *Crash safety.* The duck guard is the app's executable started again, or `$APPIMAGE` when it runs from an AppImage,
  whose mount goes away with the app; it waits with `pidfd_open` and `poll`. In a Flatpak (`FLATPAK_ID`, or
  `/.flatpak-info`) there is no guard, because it would die with the sandbox: the journal restores ducked apps at the
  next start.
- *Icons.* `app_icon` maps the executable's file name to a `.desktop` file (its `Exec`, `TryExec`, `StartupWMClass` or
  file name) in `$XDG_DATA_HOME` and `$XDG_DATA_DIRS`, and looks its `Icon=` up in the desktop's icon theme (from GTK's
  or KDE's settings) and `hicolor`: the smallest PNG at least the size asked for. SVG-only apps get the generic icon.

`cargo run -p rt-capture --example probe -- list | capture <app> [--duck [level]] | duck <app> [level]` tries the
backend on real apps without writing audio to disk. It also has `icon <app> [size]` and `permission [--ask-again]`.
`permission` answers `Unsupported` outside macOS, and `--ask-again` fails everywhere in the probe, because asking again
needs the bundle identifier of the installed app.

#### macOS

The source (`CoreAudioSource`, in `crates/rt-capture/src/macos/`) calls Core Audio directly through the
`objc2` crates. It has no duck journal and no duck guard: the taps are private to the app, so macOS removes them, and
the mute with them, when the app quits or crashes (`HAS_DUCK_GUARD` is false, and `run_duck_guard` returns `false`).
CI checks this by killing a ducking process with `kill -9`.

- *The app list.* The audio process objects come from `kAudioHardwarePropertyProcessObjectList`. Each process is
  grouped under the process macOS holds responsible for it (`responsibility_get_pid_responsible_for_pid`, an
  undocumented function loaded with `dlsym`), which puts Safari's WebKit helper under Safari and Chrome's or Discord's
  helpers under their app. If that function is missing, the group is the process's outermost `.app` bundle, and Safari's
  audio may then be listed as "WebKit". Each group is one `AudioApp`: the responsible PID, the bundle's display name and
  the bundle path as `exe_path` (which also gives the icon, through `NSWorkspace`). An app is listed once it has used
  Core Audio; a program that isn't an app bundle is listed only while it plays. Our own process isn't listed.
  The grouping is pure code over a list of `(pid, path, responsible pid)` and is unit tested on every OS.
- *Capture.* `start` checks the permission, makes a private stereo tap (`CATapDescription`) of the group's processes,
  and puts it in a private aggregate device whose main subdevice is the app's own output device, then runs an IOProc on
  it. The IOProc interleaves what the tap delivers and hands it to the `FrameSink` with the tap's format. It never
  blocks or allocates; settings reach it through atomics. A watcher thread follows the app's processes as they come and
  go (a property listener, with a 500 ms poll as a fallback) and checks the permission every 2 seconds. The capture
  ends by itself when the app has no process left, when its device goes away, or when the permission is taken back; the
  engine then looks for the app again as on Windows, or goes to *not allowed*, which `CaptureHandle::refused` tells it.
  Dropping the capture destroys the IOProc, the aggregate and the tap, in that order.
- *The permission.* TCC's `kTCCServiceAudioCapture`. macOS has no public call that says whether it was given, so the
  backend loads `TCCAccessPreflight` and `TCCAccessRequest` from the private TCC framework at run time. Allowed starts
  the capture. Not asked yet makes macOS show its question (once per run, and again on `ask_again`) and returns the
  retried error "Waiting for you to allow audio capture in the macOS dialog". Denied returns `PermissionDenied`, with
  the steps for this macOS version as its message. If the functions are missing, the backend assumes it may capture and
  goes on: detection is then off, which is a written assumption. `ask_again` runs `/usr/bin/tccutil reset AudioCapture
  <bundle id>` and then asks; it works only in the installed app, because it needs the bundle identifier.
  `open_settings` opens `x-apple.systempreferences:com.apple.preference.security`, which is Privacy & Security and not
  the audio pane, because no deep link to it is documented. From macOS 15 on both work; on 14.x only `ask_again` does.
- *Ducking.* macOS has no per-app volume, so a ducked app is muted and its audio is played again by us, on the app's
  own device, at the duck level. A mute has to go through a tap that is being read, and switching the capture tap's mute
  dropped captured audio, so the mute is a second tap in an aggregate device of its own, which exists only while the app
  is ducked. Taps record before the mute, so the capture keeps its full level and needs no compensation. The takeover
  order (`passthrough.rs`, a pure state machine that is unit tested) never leaves a gap: our replay fades in over 5 ms
  to full level, is heard for about 20 ms, then the app is muted, 20 ms later our replay fades to the duck level over
  about 150 ms. Restoring runs the other way. For that moment the app is up to 6 dB louder than it should be; CI
  measured +4.2 to +5.0 dB for 15 to 40 ms (the on-device check allows +7 dB for 100 ms, because 5 ms windows of its
  noise vary by about 1.3 times on their own). At level 0 nothing is played: ducking only mutes and restoring only
  unmutes. If the app's device can't be opened, the app is only muted. The level goes down to 0 on macOS, where
  `PLATFORM_MIN_DUCK_LEVEL` is 0; on Windows and Linux capture sees the app's volume, so a muted app couldn't be
  captured and the minimum stays 5%.
- *Versions.* Capturing needs macOS 14.2. Below it `source` returns `Unsupported("Capturing apps needs macOS 14.2 or
  later")`, and the bundle refuses to open (`minimumSystemVersion`).

### `rt-core` — platform-independent logic
- **Downmix and resampling**: captured audio is mixed to mono (kept at the capture rate for playback) and resampled
  to 16 kHz for the VAD.
- **VAD**: Silero VAD, bundled (`models/silero_vad.onnx`, MIT) and run with ONNX Runtime through `ort`, which is
  statically linked, so nothing extra ships next to the app. Frames are classified as speech or non-speech, with
  hangover/padding (~200 ms) so that word edges aren't clipped. On macOS `ort` links the static prebuilt ONNX Runtime it
  has for `aarch64-apple-darwin`, the only macOS target it has one for: Microsoft publishes no x86_64 or universal macOS
  build after 1.23.2, and `ort` 2.0.0-rc.15 needs 1.31. That is why the app is Apple Silicon only. Because it is linked
  in, there is no library to bundle, load or sign, and the self-check of the signed, hardened-runtime build shows Silero
  loading. If Silero fails anyway, the engine falls back to a simple energy detector and says so (`vad_fallback`);
  `--rt-self-check` (below) reports it.
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
  *not allowed* (retried, buffer kept) while the OS doesn't let the app capture audio (macOS), *unsupported* (not
  retried) where capture can't work. When the app quits, the sentence in progress is closed and the buffer kept, so a
  replay still works while it restarts.
- **Replay**: the output stream is opened per replay (the chosen device, else the default) and closed once the replay
  caught up with live and its tail played, so the app doesn't hold the device while idle and follows default-device
  changes. A device that fails mid-replay is replaced by the default, from where the player was. With nothing buffered
  a soft beep plays instead (two 80 ms tones at about -18 dBFS, generated in code).
- **Ducking**: a replay ducks the app (20% by default, at least `PLATFORM_MIN_DUCK_LEVEL`) as it starts; the volume
  comes back 300 ms after the replay ends or stops, unless another replay starts within those 300 ms. Without any
  output device the replay is aborted and nothing is ducked.
- **Catch-up**: from idle it starts a replay at catch-up speed (1.5×), which ends as soon as it has caught up with
  live instead of following the speaker. Each hotkey pressed again during a replay at its own speed steps one
  utterance back (at the oldest one, back to its start); pressed during a replay at the other speed, it switches to
  its own speed from where the replay is. Catch-up resets when the replay ends.
- **Wipes**: pausing, choosing another app and dropping the engine stop capture and playback, give the app its volume
  back, and wipe the buffer. A capture in another format (sample rate, channels) starts a new buffer.
- **Settings apply live**: a smaller buffer evicts at once, a new sensitivity applies from the next VAD frame, neither
  wipes anything. Speeds apply at once; the duck level and the output device at the next replay.
- **Outputs**: `CpalOutput` (the sound card; on Linux through cpal's PipeWire host when PipeWire runs, so the device
  list shows its sinks by name, else ALSA), `NullOutput` (real-time, discarded; for machines without audio) and
  `TestOutput` (collected in memory, optionally faster than real time; for tests).

`cargo run -p rt-engine --example engine_probe -- <app>` runs the engine on a real app (on an OS without a backend the
fixture plays as a mock app) and replays on Enter.

### `rt-transcribe` (planned)
`whisper-rs` (whisper.cpp) on utterances as they finish, producing a scrollable "what was just said" transcript.
Models are downloaded on demand; transcripts follow the same memory-only rule.

### `app` — Tauri 2 desktop shell
- Tray icon with menu: *Replay*, *Stop*, *Catch-up*, *Pause capture*, *Settings*, *Quit*. The status line, tooltip
  (with "N s behind live" during a replay), icon and the hotkeys next to the items follow the engine's status.
- Global hotkeys via `tauri-plugin-global-shortcut` (the tray menu is the fallback where global shortcuts are
  unavailable, e.g. some Wayland compositors).
- Settings via `tauri-plugin-store`: target app, buffer length, VAD sensitivity, output device, hotkeys, speed, duck
  level, autostart. Every change goes to the engine at once; pausing isn't stored, so every launch captures.
- The engine starts at launch with the stored settings. Its source is the OS backend, `WasapiSource` on Windows and
  `PipeWireSource` on Linux (with a duck journal of volumes in the app's local data directory, and a duck guard) and
  `CoreAudioSource` on macOS (neither). On an OS without a backend, debug builds play the rt-core speech fixture as a
  fake app, `speech_8s`; `REPEAT_THAT_MOCK_WAV=<file.wav>` plays another WAV on any OS, and in debug
  builds `REPEAT_THAT_NULL_OUTPUT=1` plays replays into nothing and `REPEAT_THAT_MOCK_PERMISSION=denied` refuses
  captures as macOS does without the permission, until "Ask again" in the window. Quit stops the engine, which wipes
  the buffer and gives the app its volume back.
- While capture is *not allowed*, the status bar shows the backend's sentence on how to allow it and the buttons the
  OS supports (`capture_permission_help`): "Open System Settings" on macOS 15 and later, "Ask again" on 14.2 and later.
  Both are commands in `app/src-tauri/src/permission.rs`, so the opener plugin stays limited to this project's GitHub
  pages. When macOS takes the permission back during a capture, the capture ends and says so
  (`CaptureHandle::refused`), and the engine goes straight to *not allowed*.
- On macOS the app is a menu bar app: `LSUIElement` in `Info.plist` and the `Accessory` activation policy mean no Dock
  icon and no app menu bar, and opening the app again from Finder or Spotlight (`RunEvent::Reopen`) shows the settings
  window. Start at login is a LaunchAgent (`tauri-plugin-autostart`). The Ducking slider goes down to 0%, shown as
  "Muted", and the General section shows a notice that macOS support is untested, with a link to the issue form
  (`.github/ISSUE_TEMPLATE/macos.yml`). Both come from `isMac()` in the window and from `PLATFORM_MIN_DUCK_LEVEL` in the
  settings, which clamps a level the OS can't use.
- The engine's status, the hotkey states and whether settings can be saved go to the window as one `status` event
  (contract in `app/src/lib/api.ts`, Rust side in `app/src-tauri/src/ipc.rs`).
- Hidden argument `--rt-self-check` (`app/src-tauri/src/self_check.rs`): the app loads Silero the way the engine does,
  prints one JSON line `{"vad":"silero"|"fallback","arch":..,"os":..,"version":..}` and exits before Tauri starts. The
  exit code is 0 only when Silero loaded and ran one frame of silence. CI runs it on every OS, and the macOS bundle
  check runs it on the built app. It plays and captures nothing. In debug builds `REPEAT_THAT_MOCK_VAD_FALLBACK=1` reports the
  fallback without loading Silero. A Windows release build has no console, so it prints nothing there and the exit code
  is all a caller sees: run `start /wait repeat-that.exe --rt-self-check` and read `%ERRORLEVEL%`, or
  `Start-Process -Wait -PassThru repeat-that.exe --rt-self-check` in PowerShell and read `$p.ExitCode`.
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
- Windows: NSIS `.exe` (per-user install, no admin rights), x64 only for v0.1 (ARM64 later: `ort` has no CPU build for
  it yet)
- Linux: `Repeat-That_<version>_amd64.deb`, `Repeat-That-<version>-1.x86_64.rpm` and
  `Repeat-That_<version>_amd64.AppImage`, x64 only. `build-linux.yml` builds them in an `ubuntu:24.04` container
  (glibc 2.39, the oldest of the supported systems) and installs them on Debian 13, Ubuntu 24.04, Fedora 40 and
  Fedora 44 before they count. Linux is a required platform of `publish-release.yml`. A Flatpak comes later.
- macOS: `Repeat-That_<version>_aarch64.dmg` for Apple Silicon only (no Intel or universal build, see the VAD note
  under [`rt-core`](#rt-core--platform-independent-logic)). It is signed with a Developer ID Application certificate and
  notarized, with the ticket stapled to the `.app` and to the `.dmg`, so Gatekeeper opens it without extra steps.
  `build-macos.yml` builds it, with the certificate and notarization credentials from the `apple-signing` Environment,
  listed in [MAINTAINING.md](MAINTAINING.md#macos-signing-and-notarization). macOS is an optional platform of
  `publish-release.yml`: a failed macOS job doesn't hold back the required platforms, and the `.dmg` is attached later
  (the [late attach](MAINTAINING.md#late-macos-attach)).
- Every release publishes a `SHA256SUMS` file so users can verify downloads
  ([verify-downloads.md](verify-downloads.md)).

**The macOS build modes.** `build-macos.yml` picks one of three, in a job that runs before a macOS runner is asked for:

| Mode | When | Signing | Result |
|---|---|---|---|
| Dry run | a pull request, or a run without a tag on a branch other than `main` | ad hoc (`APPLE_SIGNING_IDENTITY=-`), not notarized; no Environment, so no secrets | a workflow artifact |
| Signed dry run | a run on `main` without a tag | Developer ID, notarized; uses the `apple-signing` Environment | a workflow artifact |
| Release | a run on `main` with a tag, called by `publish-release.yml` or started by hand for a late attach | as the signed dry run, plus the check that the app version matches the tag | `release-macos-<file>` for `publish-release.yml`, and the "untested" paragraph in the release notes |

A signed build only builds a commit that is on `main`, because it runs that commit's build scripts while the signing
secrets are in reach. An ad hoc build can't be opened from a download without a right-click, so it is for trying the
build, not for a release. Every mode sets `MACOSX_DEPLOYMENT_TARGET=14.2`, so the binary's minimum matches
`LSMinimumSystemVersion`. Release mode also adds the paragraph that says macOS hasn't been tested on a real Mac to the
release notes, once (`macos-release-note.sh`, behind the marker `<!-- macos-untested -->`).

**What the build checks.** `check-macos-bundle.sh` mounts the finished `.dmg` read-only and checks the app inside, so
what is checked is what ships. In every mode: one `.app`; `NSAudioCaptureUsageDescription`, `LSUIElement` and
`LSMinimumSystemVersion` 14.2 in `Info.plist`; an arm64-only binary built for no newer macOS than that; `codesign
--verify --deep --strict`; the hardened runtime on; the entitlements exactly those of `Entitlements.plist`; and
`--rt-self-check` reporting Silero on `aarch64` with the bundle's version. Silero is required even in the ad hoc mode,
because it is linked in and a fallback would mean something broke. Signed builds also check that the app and the image
are signed by the Developer ID identity, that `spctl` accepts both as "Notarized Developer ID", and that both have their
ticket stapled.

macOS ties the audio-capture permission to the app's signing identity. A Developer ID signature stays the same from
one release to the next, so an update shouldn't reset the permission (nobody has tried this on a Mac). The user can
still deny it or revoke it later, which the backend sees through TCC and the window explains (see [macOS](#macos)).

Notarization requires the hardened runtime, which Tauri turns on (`bundle.macOS.hardenedRuntime`, also set in
`tauri.macos.conf.json`). Apple's
[documentation for Core Audio taps](https://developer.apple.com/documentation/coreaudio/capturing-system-audio-with-core-audio-taps)
asks for the `NSAudioCaptureUsageDescription` key in `Info.plist` (Tauri merges `app/src-tauri/Info.plist` into the
bundle's) and names no entitlement. The sample app [AudioCap](https://github.com/insidegui/AudioCap) uses taps with the
hardened runtime and ships `com.apple.security.device.audio-input`, but it is also sandboxed, so that doesn't prove the
entitlement is needed here. `app/src-tauri/Entitlements.plist` therefore lists that one entitlement, set through
`bundle.macOS.entitlements`. It gives the app no access by itself (the user's permission still decides), so keeping
it costs nothing if capture doesn't need it. This is a written assumption: the capture test in CI runs an unsigned test binary, outside the hardened runtime, and nobody has
tried a notarized build on a Mac. If capture turns out to work without the entitlement, remove it; if it needs more,
add it, and the bundle check keeps the list in `Entitlements.plist` and in the signed app the same.

Tauri signs the app with the hardened runtime and the entitlements, notarizes the `.app` and staples its ticket, and
then signs the `.dmg`, but it doesn't notarize the `.dmg`. The workflow does that (`xcrun notarytool submit --wait`
with the same API key, then `xcrun stapler staple`), so there are two notarizations per signed build.

**The Linux packages.** The bundle settings are in `app/src-tauri/tauri.linux.conf.json`. Tauri adds WebKitGTK, GTK
and the AppIndicator library (loaded at run time for the tray) to the dependencies; the config adds PipeWire's and
ALSA's client libraries, which the binary links, with the `t64` package names of Debian 13 and Ubuntu 24.04 as
alternatives on the `.deb` and sonames on the `.rpm`, and recommends PipeWire and WirePlumber. It also asks for glibc
2.38 and GCC 13's libstdc++ (`libc6 (>= 2.38)`, `libstdc++6 (>= 13)`, or `libc.so.6(GLIBC_2.38)(64bit)` and
`libstdc++.so.6(GLIBCXX_3.4.31)(64bit)`): the prebuilt ONNX Runtime that `ort` links in statically needs them, so Debian
12 (glibc 2.36, GCC 12) can't run the app, and its package manager refuses the package rather than install a binary
that won't start. The desktop entry is
installed as `me.roboroads.repeat-that.desktop`, the app ID the GlobalShortcuts portal is given, so the desktop can
name the app's shortcuts and match its windows. The bundler always makes an entry of its own named after the product,
`Repeat That.desktop`; the packages ship it hidden (`linux/hidden.desktop`), and the AppImage, whose root entry it
would be, gets the real entry in its place. The AppImage leaves out `libpipewire-0.3` (linuxdeploy's exclude list has
it), so the system's client library talks to the system's PipeWire and loads plugins of its own version.

The AppImage's launcher puts its own folders in front of `LD_LIBRARY_PATH`, `PATH`, `XDG_DATA_DIRS` and sets the GTK
variables to them, which the app needs. Host programs the app starts (the desktop's shortcut settings in
`shortcut_page.rs`, the browser for a link through the `open_link` command) inherit nothing of that: `host_env.rs`
takes every entry that points into the AppImage out of their environment, as otherwise a newer system's `gio` or D-Bus
tools would load the AppImage's older libraries and fail. Links go through `open_link`, which opens this project's
pages only, rather than through the opener plugin, which the window no longer has a permission for.

`check-linux-packages.sh` unpacks all three and checks names, versions, dependencies, the files the `.deb` and the
`.rpm` install (the same in both), both desktop entries with `desktop-file-validate`, the binary's glibc and libstdc++
needs against what the packages ask for, and the AppImage's root entry, icon and libraries. `smoke-linux-package.sh` then installs the package with the distribution's
package manager in a fresh container of each supported system, without recommended packages, checks `ldd` and runs
`--rt-self-check` from the package and from the AppImage, and uninstalls again. On Debian 12 it checks that apt
refuses the `.deb`.

There is no auto-updater yet (it is on the roadmap under later ideas). When it arrives, the Tauri updater will verify
updates with its own key pair, stored as a GitHub Actions secret and separate from any code signing.

The Windows installer is not code signed yet, so Windows shows a SmartScreen warning. Open-source code signing on
Windows: apply to [SignPath Foundation](https://signpath.org).
