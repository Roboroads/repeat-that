# Privacy

Repeat That handles other people's voices, so privacy is a core design rule, not an afterthought.

## What the app does with audio

- **Memory only.** Captured audio is held in a fixed-size in-memory ring buffer. It is never written to disk — no temp
  files, no cache, no crash dumps containing audio.
- **Only volumes on disk.** The one thing capture writes to disk is a short list of the app volumes it lowered while
  ducking, so they can be restored after a crash. It never contains audio.
- **Short-lived.** The buffer only holds the last *X* seconds of speech (configurable). Older audio is overwritten
  continuously, and the whole buffer is wiped when you stop capturing or quit the app.
- **Local only.** Nothing is uploaded. There is no telemetry and no account. Transcription (planned) runs on-device.
- **One app at a time.** The app only captures the application you explicitly select, and never your microphone.

## A note on recording laws

Laws about recording conversations differ by country and region. Some places require consent from everyone in the
conversation; others treat short-lived buffers that are never stored differently from recordings. Repeat That is
designed to act like a short "memory aid" rather than a recorder, but **you are responsible for using it in line with
the laws that apply to you and the policies of your employer or the platform you're using**. When in doubt, let the
people you're talking to know you use it.

This is not legal advice.
