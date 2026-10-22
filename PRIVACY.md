# Privacy

Repeat That handles other people's voices, so privacy is a core design rule.

## What the app does with audio

- Captured audio is held in a fixed-size in-memory ring buffer. It is never written to disk: no temp files, no cache,
  no crash dumps containing audio.
- Up to three small files are written to disk, and none has audio in it. The settings file (`settings.json` in the app's
  config folder, `%APPDATA%\me.roboroads.repeat-that` on Windows,
  `~/Library/Application Support/me.roboroads.repeat-that` on macOS, `~/.config/me.roboroads.repeat-that` on Linux)
  holds your settings: the chosen app's name and the path to its program file, the buffer length, the hotkeys, the
  output device, and your answer to the [update check](#update-checks) question. The duck journal (`duck.journal` in the
  app's local data folder, `%LOCALAPPDATA%\me.roboroads.repeat-that` on Windows, `~/.local/share/me.roboroads.repeat-that` on Linux)
  lists the volumes that capture lowered while ducking, so they can be restored after a crash. It holds identifiers of
  audio sessions (PipeWire streams on Linux) and volume levels, nothing else. macOS has no duck journal: macOS removes
  the app's mute by itself when Repeat That quits or crashes, and the app keeps no record of it. The **Start at login**
  switch adds a small file that says which app to start, a launch agent in `~/Library/LaunchAgents` on macOS and
  `~/.config/autostart/Repeat That.desktop` on Linux, and it goes away when you turn the switch off. The end-to-end
  test runs the app with its home, temp, config, data and cache folders in one scratch folder and checks that a run
  with capture and replay leaves no audio file in it.
- The buffer only holds the last X seconds of speech (configurable). Older audio is overwritten continuously, and the
  whole buffer is wiped when you stop capturing or quit the app.
- Nothing is uploaded. There is no telemetry and no account. Transcription (planned) runs on-device. The one network
  connection the app can make is the [update check](#update-checks), which is off until you say yes.
- The app only captures the application you explicitly select, and never your microphone. On macOS the system asks you
  before Repeat That may capture another app's audio, and you can take that permission back at any time, which ends the
  capture. While an app is ducked on macOS, Repeat That plays that app's audio again at the lower volume, straight from
  memory; that audio isn't kept either.

## Update checks

Repeat That can ask GitHub whether a newer version is out. It does that only after you agree, and it is the only thing
the app does on the network. A check sends no audio and none of your settings. What it does tell GitHub is listed here.

- The settings window asks "Check for updates automatically?" once. Until you answer, the app contacts nobody.
  **Not now** leaves automatic checking off. The **Check for updates automatically** switch under General changes the
  answer later, and the answer is saved in the settings file.
- With the switch on, the app checks 60 seconds after it starts and then every 24 hours while it runs. A check that
  fails is tried again after an hour. The time of the last check isn't saved, so each start checks once.
- **Check now** in the settings window and **Check for updates...** in the tray menu check at once, whatever the
  switch says, because you asked.
- A check is one request for a small file in the latest release on github.com, `latest-<system>-<cpu>.json`, for
  example `latest-windows-x86_64.json`. The file name is all the request says about your computer. The version you run
  isn't sent: the app compares it with the version in the file. The request carries the user agent
  `tauri-plugin-updater/<version>`, which names the update library and not Repeat That.
- GitHub sees the request, and so do the file servers it hands downloads to. They see your IP address, as they do for
  any download from GitHub, and
  [GitHub's privacy statement](https://docs.github.com/en/site-policy/privacy-policies/github-general-privacy-statement)
  applies. Nothing goes to a server of this project, because there isn't one.
- Nothing is downloaded until you click **Install and restart**. The download comes from the same release, and the
  app installs it only if it is signed with the project's update key, whose public half is built into the app. A file
  with another signature is refused. How the key is kept is in
  [MAINTAINING.md](docs/MAINTAINING.md#update-signing-key).

What happens after a check depends on how Repeat That was installed. The Windows installer, the macOS app and the
AppImage install the new version themselves. The `.deb` and `.rpm` packages only tell you that a version is out, and a
button opens its release page in your browser. So does a macOS app that runs from the disk image, and so does an
AppImage in a folder you can't write to. The Flatpak never checks, and neither does a build without the project's
update key, such as one you compile yourself.

An AppImage that has an update key creates an empty file next to itself for a moment when it starts, to find out
whether it is allowed to replace itself, and deletes it again at once.

## This website

The website at [repeat-that.roboroads.me](https://repeat-that.roboroads.me) is hosted on GitHub Pages. GitHub logs the
IP address of visitors to a Pages site, as described in
[GitHub's privacy statement](https://docs.github.com/en/site-policy/privacy-policies/github-general-privacy-statement).
The site itself sets no cookies, runs no analytics and loads nothing from other websites: no fonts, scripts, images or
embeds. The home page reads your browser's user agent to highlight the download for your operating system. That happens
in your browser and the result isn't sent anywhere. The download links go to GitHub, which sees the request when you
follow one. The introduction video, when there is one, comes from the site itself and isn't requested until you press
play.

## A note on recording laws

Laws about recording conversations differ by country and region. Some places require consent from everyone in the
conversation, and others treat short-lived buffers that are never stored differently from recordings. Repeat That is
designed to act like a short "memory aid" rather than a recorder, but you are responsible for using it in line with
the laws that apply to you and the policies of your employer or the platform you're using. When in doubt, let the
people you're talking to know you use it.

This is not legal advice.
