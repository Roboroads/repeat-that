# Installing on macOS

Repeat That needs a Mac with Apple Silicon (M1 or later) running macOS 14.2 or later. Intel Macs aren't supported:
the speech detector needs ONNX Runtime, and Microsoft ships no x86_64 or universal build of it after 1.23.2.

macOS support is new and hasn't been tested on a real Mac yet. Nobody on the team has one. The app is built, signed
and notarized on GitHub's macOS runners, and the capture code runs against a virtual sound card there, but nobody has
used it the way you will. If something doesn't work, please
[file a macOS report](https://github.com/Roboroads/repeat-that/issues/new?template=macos.yml). If it works, say so in
the same form. That is what lets us remove this notice.

The app is signed with a Developer ID certificate and notarized by Apple, so it opens like any other app. It still
isn't in the App Store.

## Download

Get the disk image from the [website](https://repeat-that.roboroads.me), which links the latest one and shows its
checksum, or from the [Releases page](https://github.com/Roboroads/repeat-that/releases), which has every version. A
release with macOS support has:

- `Repeat-That_<version>_aarch64.dmg` is the disk image with the app in it.
- `SHA256SUMS` holds the checksum of the image, to [verify your download](verify-downloads.md).

The macOS build is allowed to finish after the rest of a release, so a release can be out for a while without its
`.dmg`. When it is added, `SHA256SUMS` gets one extra line. Check back later, or download `SHA256SUMS` again.

## Install

1. Open the `.dmg` and drag **Repeat That** onto the **Applications** folder next to it. Eject the image afterwards.
2. Open Repeat That from Applications. Because it came from the internet, macOS asks whether you are sure you want to
   open it. Click **Open**.
3. Repeat That has no Dock icon. It lives in the menu bar, near the clock, and opens its settings window on the first
   run. If the menu bar is full, macOS hides the icons that don't fit (behind the notch, on newer MacBooks). Opening
   Repeat That again from Applications or Spotlight shows the settings window either way. Closing the window keeps the
   app running, and **Quit** in the menu bar menu ends it.

## First capture and the permission

Start the app whose audio you want to keep (Discord, Teams, Safari, ...) and pick it in the **App** list of the
settings window. It shows up within a couple of seconds once it plays sound.

The first time Repeat That captures, macOS asks whether it may capture audio from other apps. The text under the
question is "Repeat That keeps the last few seconds of the app you pick in memory, so you can replay what was just
said. Audio is never saved." Click **Allow**. Until you answer, the window says it is waiting for you to allow audio
capture, and capture starts by itself once you do.

If you click **Don't Allow**, or turn the permission off later in System Settings, the window says that Repeat That
isn't allowed to capture audio, and shows what you can do. [macos.md](macos.md#audio-capture-permission) has the
details for macOS 14 and for macOS 15 and later. What was captured before stays in the buffer, so a replay still works.

## Hotkeys

The default hotkeys are the ones from Windows. The settings window calls the Option key **Alt**:

| Hotkey | What it does |
|---|---|
| `Ctrl+Alt+Space` (Control, Option, Space) | Replay, and a step back during a replay |
| `Ctrl+Alt+Enter` | Catch-up |
| `Ctrl+Alt+Backspace` | Stop |

If you have more than one input source (keyboard layout), macOS may already use Control+Option+Space to switch between
them, and then the replay hotkey does nothing. Change the hotkey in the Hotkeys section, or turn the macOS shortcut off
in System Settings > Keyboard > Keyboard Shortcuts > Input Sources. How replays work, and what the other settings do,
is in the [guide](guide.md).

## Volume during replay

While a replay plays, Repeat That lowers the volume of the app you picked, to 20% unless you change it. macOS has no
volume setting per app, so Repeat That mutes the app and plays its audio itself, at the lower volume, on the device the
app plays to. It switches over with a short fade, and for a few tens of milliseconds both are playing, which can sound
like a small volume bump. If you hear clicks or a bump when a replay starts or ends, set **Volume during replay** to
**0%**, which the slider shows as **Muted**. The app is then only muted during a replay and Repeat That plays nothing
of its own.

The slider goes down to 0% on macOS only. On Windows and Linux it stops at 5%, and a settings file from a Mac with 0%
in it is raised to 5% there.

If Repeat That can't open the device the app plays to, it mutes the app during the replay instead.

## Testing checklist if you have a Mac

Anything on this list that you can try helps, and so does a report that says it all worked. Open the
[macOS report form](https://github.com/Roboroads/repeat-that/issues/new?template=macos.yml) and name the items that
didn't work, with your macOS version.

1. Open the app from Applications. Only the standard "downloaded from the internet" question appears.
   `spctl -a -t exec -vv "/Applications/Repeat That.app"` in Terminal prints `source=Notarized Developer ID`.
2. `/Applications/Repeat\ That.app/Contents/MacOS/repeat-that --rt-self-check` prints one line of JSON like
   `{"vad":"silero","arch":"aarch64","os":"macos","version":"0.1.1","updater":true}` and exits with 0 (`echo $?`). A
   `"fallback"` there means the speech detector didn't load. `"updater"` says whether this build can check for
   updates, and `false` only means it has no update key. Paste the line into your report.
3. The menu bar icon shows, there is no Dock icon, and the settings window opens on the first run.
4. Play audio in Safari, Music or Discord. The app shows up in the App list with its icon and its own name (not
   "WebKit" or a helper process).
5. Pick it. macOS asks for the permission, the window says it is waiting, and after **Allow** it says "Capturing".
6. Let someone speak for ten seconds, then press the replay hotkey. The speech plays on your output device, a second
   press steps back, and catch-up and stop work.
7. Listen to the app's volume around a replay at 20% and at 0%: it drops when the replay starts, comes back about 300 ms
   after it ends, and doesn't click.
8. See the "not allowed" state. On macOS 15 and later, turn the permission off in System Settings > Privacy & Security >
   Screen & System Audio Recording while capturing. Within a few seconds the window says Repeat That isn't allowed
   to capture audio. On macOS 14 there is no switch, so run `tccutil reset AudioCapture me.roboroads.repeat-that`,
   quit and reopen Repeat That, start capturing and click **Don't Allow**. Either way, a replay of what was buffered
   still works, and **Ask again** brings the question back. On macOS 15 and later **Open System Settings** opens
   Privacy & Security too; on 14 only **Ask again** is there.
9. Start a replay and end Repeat That with `pkill -9 -x repeat-that`. The app's own sound is back at full volume within
   a second.
10. Turn on **Start at login**, log out and in. Repeat That starts in the menu bar without its window.
11. Install a newer version over the old one. The permission and the picked app are still there.
12. With a newer release out, open General in the settings window and click **Check now**. The banner offers the new
    version, and **Install and restart** replaces the app and starts the new one, with the permission and the picked
    app still there. Nobody has tried this on a Mac. Run the app from Applications, not from the disk image.

## Updating and uninstalling

Repeat That can check for updates itself once you allow it: the settings window asks the first time, and General has
the switch and a **Check now** button. When a newer version is out, a banner offers **Install and restart**. That only
works from Applications. Run straight from the disk image or the Downloads folder, the app says so and the banner
links to the download page instead. What a check sends is in [PRIVACY.md](../PRIVACY.md#update-checks).

To update by hand, or from a version without the updater, quit Repeat That, download the newer `.dmg` and drag the app
onto Applications again, replacing the old one. Your settings are kept, and the permission should be too, because every
release is signed with the same certificate.

To uninstall:

1. Turn off **Start at login** in the settings window, then choose **Quit** in the menu bar menu. If you removed the app
   with that switch still on, look in `~/Library/LaunchAgents` for a file with Repeat That in its name and delete it.
   macOS may also list Repeat That under System Settings > General > Login Items & Extensions.
2. Drag Repeat That from Applications to the Trash.
3. Remove the permission: `tccutil reset AudioCapture me.roboroads.repeat-that`.
4. Delete the settings folder, `~/Library/Application Support/me.roboroads.repeat-that`. It holds your settings and the
   chosen app's name and path, never audio.

## Build from source

See [macos.md](macos.md#build-from-source).
