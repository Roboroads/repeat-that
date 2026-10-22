# Privacy

Repeat That handles other people's voices, so privacy is a core design rule, not an afterthought.

## What the app does with audio

- **Memory only.** Captured audio is held in a fixed-size in-memory ring buffer. It is never written to disk — no temp
  files, no cache, no crash dumps containing audio.
- **Two small files on disk, neither with audio.** The settings file (`settings.json` in the app's config folder,
  `%APPDATA%\me.roboroads.repeat-that` on Windows, `~/Library/Application Support/me.roboroads.repeat-that` on macOS)
  holds your settings: the chosen app's name and the path to its program file, the buffer length, the hotkeys, the
  output device. The duck journal (`duck.journal` in the app's local data folder,
  `%LOCALAPPDATA%\me.roboroads.repeat-that` on Windows) lists the volumes that capture lowered while ducking, so they
  can be restored after a crash. It holds identifiers of audio sessions and volume levels, nothing else. macOS has no
  duck journal, so there it is one file: macOS removes the app's mute by itself when Repeat That quits or crashes, and
  the app keeps no record of it. The end-to-end test runs the app with its home, temp, config, data and cache folders in
  one scratch folder and checks that a run with capture and replay leaves no audio file in it.
- **Short-lived.** The buffer only holds the last *X* seconds of speech (configurable). Older audio is overwritten
  continuously, and the whole buffer is wiped when you stop capturing or quit the app.
- **Local only.** Nothing is uploaded. There is no telemetry and no account. Transcription (planned) runs on-device.
- **One app at a time.** The app only captures the application you explicitly select, and never your microphone. On
  macOS the system asks you before Repeat That may capture another app's audio, and you can take that permission back
  at any time, which ends the capture. While an app is ducked on macOS, Repeat That plays that app's audio again at the
  lower volume, straight from memory; that audio isn't kept either.

## A note on recording laws

Laws about recording conversations differ by country and region. Some places require consent from everyone in the
conversation; others treat short-lived buffers that are never stored differently from recordings. Repeat That is
designed to act like a short "memory aid" rather than a recorder, but **you are responsible for using it in line with
the laws that apply to you and the policies of your employer or the platform you're using**. When in doubt, let the
people you're talking to know you use it.

This is not legal advice.
