# Repeat That on macOS

Repeat That runs on Macs with Apple Silicon and macOS 14.2 or later. It captures one app's audio through a Core Audio
process tap, and releases are signed with a Developer ID certificate and notarized by Apple, so the app opens like any
other once it is in Applications. How to install it is in [install-macos.md](install-macos.md).

**macOS support hasn't been tested on a real Mac yet.** Nobody on the team has one, so CI on GitHub's macOS runners is
the only test so far. If something doesn't work, please
[file a macOS report](https://github.com/Roboroads/repeat-that/issues/new?template=macos.yml). The notice stays in the
README, the settings window and the release notes until someone confirms that it works.

Only Apple Silicon Macs are supported. Microsoft ships no x86_64 or universal ONNX Runtime after 1.23.2, and the speech
detector needs it, so there is no Intel or universal build.

## Audio-capture permission

Repeat That needs permission to capture other apps' audio. The first time you start capturing, macOS asks for it, and
capture starts by itself once you click **Allow**. If you clicked **Don't Allow**, or turned the permission off later,
the status bar in the settings window says "Repeat That isn't allowed to capture audio" and shows what you can do.
Capture is tried again every 2 seconds, so it resumes as soon as the permission is back, and the buffer is kept, so a
replay still works in the meantime.

What the window offers depends on your macOS version:

- **macOS 15 and later.** **Open System Settings** opens Privacy & Security. Scroll to **Screen & System Audio
  Recording** and turn on Repeat That (audio only is enough). If capturing doesn't resume once it is on, quit and reopen
  Repeat That. **Ask again** is there too.
- **macOS 14.x.** Only **Ask again**. Click it and choose **Allow**. System Settings has no switch for this permission
  on 14, and the Screen Recording switch there is a different permission (Apple lists `ScreenCapture` and
  `AudioCapture` as separate services), so turning it on doesn't help.

**Ask again** does what the command below does, and then makes macOS show its question once more. It works only in the
installed app, because macOS knows the app by its bundle identifier. If the button fails, or you run Repeat That from a
terminal, reset the permission from Terminal, then quit and reopen Repeat That and start capturing again:

```sh
tccutil reset AudioCapture me.roboroads.repeat-that
```

`AudioCapture` is the service name that Apple's
[tccutil documentation](https://developer.apple.com/documentation/xcode/resetting-access-to-protected-resources-in-macos)
lists for capturing system audio, and `me.roboroads.repeat-that` is the app's bundle identifier.

If you turn the permission off while Repeat That is capturing, macOS ends the capture, and the window says so within a
few seconds.

Two things are written assumptions, because nobody could try them. **Open System Settings** opens the Privacy &
Security page and not the audio pane itself, because Apple documents no link to that pane. And Repeat That finds out
whether the permission is given with two functions of macOS's private TCC framework, which it loads at run time. If a
macOS version doesn't have them, Repeat That can't tell and just tries to capture, so a missing permission shows up as
a capture that doesn't start, not as the "not allowed" message.

## Which apps show up

An app is listed once it has used Core Audio. A program that isn't an app bundle is listed only while it plays sound.
Browsers and chat apps play through helper processes, and Repeat That groups those under the app that macOS holds
responsible for them, so Safari's WebKit helper is part of Safari and Discord's helpers are part of Discord. If macOS
doesn't answer that, Repeat That groups by the outermost `.app` bundle instead, and Safari's audio may then be listed as
"WebKit".

## Ducking

macOS has no volume setting per app. During a replay Repeat That mutes the app through a second tap and plays the
app's audio itself, at the volume you chose, on the device the app plays to. The capture keeps its full level, because
taps record before the mute. At 0% ("Muted") the app is only muted. If you hear clicks or a volume bump, use 0%. The
reason, and the numbers CI measured, are in [ARCHITECTURE.md](ARCHITECTURE.md#macos).

## Build from source

You need Apple's command-line tools, Rust and Node.js 22.19 or later.

```sh
xcode-select --install
curl https://sh.rustup.rs -sSf | sh
git clone https://github.com/Roboroads/repeat-that && cd repeat-that
cd app && npm install && npm run tauri build
```

The disk image ends up in `target/release/bundle/dmg/` and the app in `target/release/bundle/macos/`, at the root of
the repository because the app crate is a member of the Cargo workspace. CI builds with `--target
aarch64-apple-darwin`, which puts them under `target/aarch64-apple-darwin/release/bundle/` instead. There is nothing
else to download: the speech detector's ONNX Runtime is linked into the app.

An app you build yourself isn't quarantined, so Gatekeeper doesn't check it.

It isn't signed with our Developer ID certificate either, at most ad-hoc signed, and an ad-hoc signature changes with
every build. macOS ties the audio-capture permission to the signature, so after a rebuild it may stop honoring the
permission you granted, and capture returns silence. Reset the permission as described under
[Audio-capture permission](#audio-capture-permission) and grant it again.

A debug build (`npm run tauri dev` in `app/`) captures real apps too. macOS asks about the program that started it,
usually your terminal, and **Ask again** doesn't work there. To see the "not allowed" state without a Mac, run it with
`REPEAT_THAT_MOCK_PERMISSION=denied` on any OS (see [CONTRIBUTING.md](../CONTRIBUTING.md#running-the-end-to-end-test)).

The built app answers `--rt-self-check`, which prints one line of JSON about the speech detector and the machine and
exits with 0 only when the detector loaded:

```sh
"/Applications/Repeat That.app/Contents/MacOS/repeat-that" --rt-self-check
```
