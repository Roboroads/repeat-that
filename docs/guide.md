# Using Repeat That

Repeat That keeps the last 30 seconds of speech from one app in memory (the length is a setting) and plays it back when
you press a hotkey. This guide goes through the first launch, the hotkeys and every setting, and ends with what to try
when something doesn't work. It covers Windows, macOS and Linux, and says so where they differ. Installing is in the
install guides for [Windows](install-windows.md), [macOS](install-macos.md) and [Linux](install-linux.md).

## First launch

Repeat That runs in the background and puts an icon in the system tray (the menu bar on a Mac). The first launch also
opens the settings window. Closing that window doesn't quit the app, and the hotkeys keep working. Quit is in the tray
menu.

Where the icon is depends on the system:

- Windows. The notification area at the right end of the taskbar. If you can't see it, click the ^ arrow next to the
  clock. A left click opens the settings window and a right click opens the menu.
- macOS. The menu bar, near the clock. There is no Dock icon. If the menu bar is full, macOS hides the icons that don't
  fit (behind the notch, on newer MacBooks). Opening Repeat That again from Applications or Spotlight shows the settings
  window either way. A click on the icon opens the menu, which has Settings in it.
- Linux. The tray of your desktop, which needs a StatusNotifierItem host. KDE Plasma has one. GNOME doesn't, unless the
  AppIndicator and KStatusNotifierItem Support extension is installed and turned on. Without a tray the settings window
  opens at every start and the General section has a Quit Repeat That button, see [linux.md](linux.md#tray).

The tray menu has a status line (the app it captures and how much speech is buffered, or how far behind live a replay
is), then Replay, Catch-up, Stop, Pause capture, Settings and Quit. The icon changes while it captures and while it is
paused.

## Pick an app

1. Start the app you want to catch up on (Discord, Teams, a browser tab with a video) and let it play some sound.
2. Find it in the App list of the settings window. It shows up within a couple of seconds, and the Refresh button
   asks again.
3. Click it. The list folds into the row of the app you chose, and a click on that row opens the list again.

The status line at the top of the window then says "Capturing" and the app's name. Repeat That captures that app and
nothing else: not your microphone and not the rest of your system sound. An app that plays through helper programs, such
as a browser, Discord or Teams, counts as one app, so everything it plays is captured. Picking another app, or pausing,
discards what was buffered.

Repeat That finds the app again by the location of its program file, not by the running process. When the app quits, the
window says something like "Waiting for Discord", and capture starts by itself within a couple of seconds of the app
coming back. What was buffered before it quit can still be replayed in the meantime.

### Windows

Windows 10 version 2004 or newer, or Windows 11. There is nothing to allow.

### macOS

The first time Repeat That captures, macOS asks whether it may capture audio from other apps. Click Allow. Until you
answer, the window says it is waiting. If you click Don't Allow, or switch the permission off later, the window says
that Repeat That isn't allowed to capture audio. It offers an Ask again button, and on macOS 15 and later an Open System
Settings button too. The buffer is kept meanwhile, so a replay still works. The steps for each macOS version are in
[macos.md](macos.md#audio-capture-permission).

macOS support hasn't been tested on a real Mac yet. If something doesn't work,
[file a macOS report](https://github.com/Roboroads/repeat-that/issues/new?template=macos.yml).

### Linux

Capture goes through PipeWire, with WirePlumber as its session manager. Apps that play through the PulseAudio interface
(most do) reach PipeWire through pipewire-pulse and work. On a system that still runs PulseAudio, the window says
"Capture needs PipeWire, and it isn't running". The minimum systems and the fix are in
[install-linux.md](install-linux.md).

## Replay what you missed

| Default hotkey | Action | What it does |
|---|---|---|
| `Ctrl+Alt+Space` | Replay | Plays the last thing that was said. Pressed again, it steps back one utterance. |
| `Ctrl+Alt+Enter` | Catch-up | The same replay at a faster speed, which ends as soon as it reaches live. |
| `Ctrl+Alt+Backspace` | Stop | Ends the replay at once. |

On a Mac, Alt is the Option key. The settings window has Replay and Stop buttons, and the tray menu has all three
actions, for when you'd rather click.

An utterance is a stretch of speech between pauses. A replay starts at the utterance being spoken right now if it has
run for 1.5 seconds. Otherwise it starts at the last one that ended, unless that was more than 10 seconds ago and a new
one has started. It doesn't stop after that utterance. It plays everything said since, including what is said while it
plays, and the silences in between are shortened to about a quarter of a second. When it reaches someone who is still
talking it follows them at normal speed, and it ends when there is nothing left to play. The app keeps listening
throughout.

Pressing Replay again steps back one utterance. A pause of about 0.7 seconds ends an utterance, and a long monologue is
cut every 15 seconds so that you can still step back inside it. Stepping back from the oldest utterance in the buffer
restarts that one. Bursts shorter than a quarter of a second, such as a cough or a click, aren't kept.

Catch-up is for when you have fallen behind and want to get back to live without sitting through the replay at normal
speed. It plays at 1.5x by default with the pitch kept, and it ends the moment it has caught up. Pressing Catch-up again
during a catch-up steps back one utterance, just like Replay does. Pressing the other hotkey while a replay plays
switches the speed from where the replay is, without starting over.

While a replay plays, the settings window and the tray tooltip say "Replaying" or "Catching up" and how far behind live
it is. When nothing is buffered, a replay plays a short beep of two falling tones instead, see
[the beep](#the-replay-beeps).

## Change the hotkeys

On Windows, on macOS and on Linux under X11, the Hotkeys section works the same way. Click the field of an action and
press the combination you want. A combination needs Ctrl, Alt or the Windows (Cmd on a Mac) key held with another key,
though function keys also work alone. Escape cancels. Backspace on its own clears the field, which leaves that action
without a hotkey. Reset to defaults puts all three back.

Hotkeys work in every program, not only while Repeat That's window is in front. When another program already uses a
combination, the field says "In use by another app" and the hotkey does nothing until you pick another one.

On macOS, if you have more than one input source (keyboard layout), macOS may already use Control+Option+Space to switch
between them. Then the replay hotkey does nothing. Change the hotkey, or switch the macOS shortcut off. In System
Settings it is under Keyboard, Keyboard Shortcuts, Input Sources.

On Linux under Wayland, an app can't grab keys itself. KDE Plasma and GNOME 48 and later have a portal for it: the
desktop asks you to confirm three shortcuts the first time Repeat That starts, the Hotkeys section shows the keys it
gave them, and a button there opens the desktop's own shortcut settings. Other Wayland desktops (GNOME 46 on Ubuntu
24.04, Sway, Hyprland) can't give an app global hotkeys. There the Hotkeys section lists three commands,
`repeat-that --replay`, `repeat-that --catch-up` and `repeat-that --stop`, each with a Copy button, and you bind them as
custom shortcuts in your desktop. [linux.md](linux.md#hotkeys) has the details for each desktop.

## Volume during a replay

While a replay plays, Repeat That lowers the volume of the app you picked, so the replay is easy to hear over the live
call. The volume comes back about 300 milliseconds after the replay ends or is stopped. The switch "Lower the app's
volume during a replay" turns it off, and the slider "Volume during replay" sets the level. The default is 20%.

The slider goes from 5% to 100% on Windows and Linux. A muted app can't be captured there, so 5% is the lowest. On a Mac
it goes down to 0%, shown as "Muted". macOS has no volume setting per app, so Repeat That mutes the app and plays its
sound itself at the lower volume. If you hear a click or a small bump when a replay starts or ends, set the slider to
0%, and the app is only muted. The reason is in [macos.md](macos.md#ducking).

If Repeat That is killed or crashes during a replay, the volume is restored without you doing anything. On macOS the
system removes the mute when the app is gone. On Windows, and on Linux with the `.deb`, the `.rpm` or the AppImage, a
helper process puts the volume back, and after a power cut the next start does. A Flatpak (there isn't one yet) can't
run a helper, so there the volume comes back the next time Repeat That starts.

## Other settings

| Setting | Range | Default |
|---|---|---|
| Buffer length | 10 to 120 seconds | 30 seconds |
| Sensitivity | Low, Normal or High | Normal |
| Replay speed | 0.75x to 1.5x | 1x |
| Catch-up speed | 1.25x to 2x | 1.5x |
| Volume during replay | 5% to 100% (0% to 100% on macOS) | 20% |
| Output device | any listed device | System default |
| Start at login | on or off | off |

Settings apply at once, except the volume and the output device, which apply from the next replay.

Buffer length is seconds of speech, not seconds of clock time. Silence isn't kept, so 30 seconds of buffer can cover
more than 30 seconds of the call. A longer buffer uses more memory, and a shorter one drops the oldest speech right
away.

Sensitivity is how readily the speech detector calls a sound speech. Low ignores background noise but may miss quiet
speech. Normal is balanced for most calls. High catches quiet speech but may also pick up background noise. Changing it
doesn't wipe the buffer.

Replay speed and catch-up speed are the speeds of the two actions. Both keep the pitch.

Output device is where replays play. The list refreshes when you open it, and a replay uses the device only while it
plays. If the chosen device is unplugged, replays use the system default and the window says so.

Pause capture is in the settings window and the tray menu. It stops capturing, discards the buffer, ends a replay that
is playing and gives the app its volume back. Press it again to resume. It needs an app to be picked first.

Start at login is called Start with Windows on Windows. Repeat That then starts in the tray without opening the window.
On Linux it passes `--hidden` to do that. If you move or rename an AppImage, turn the switch off and on again, see
[install-linux.md](install-linux.md#run-the-appimage).

## Privacy in short

- Audio only ever lives in memory. It is never written to disk and never sent anywhere.
- The buffer is wiped when you pause, pick another app or quit.
- Only the app you pick is captured, and never the microphone.
- Two small files are saved: your settings (including the name and location of the app you picked) and, on Windows and
  Linux, a journal of the volumes that were lowered, so they can be restored after a crash.
- There is no account and no telemetry.

The details are in [PRIVACY.md](../PRIVACY.md), which also has
[a note on recording laws](../PRIVACY.md#a-note-on-recording-laws): the laws on recording conversations differ between
countries, and you are responsible for following the ones that apply to you.

## When something doesn't work

### The replay beeps

Two short falling tones mean the buffer is empty. Common reasons:

- You just picked the app or resumed from a pause. Both start with an empty buffer.
- The app hasn't played any speech since then, or the speech was too quiet for the sensitivity setting. Try High.
- The speech was played by a different app than the one you picked.

### The app isn't in the list

Apps show up once they have played sound, so start the call or the video, then press Refresh. If the window says it
couldn't list the apps, the message after it says why. On Linux, "Capture needs PipeWire, and it isn't running" means
the system still runs PulseAudio, see [install-linux.md](install-linux.md).

### The window says it is waiting for the app

The app you picked isn't running, or hasn't played any sound yet. Capture starts by itself when it does. Repeat That
finds the app by the location of its program file, so if the app was updated into another folder or you installed it
somewhere else, pick it again from the list.

### Can't capture the app

The window says "Can't capture" and the app's name, with the reason under it. Repeat That tries again every 2 seconds,
so a temporary problem clears by itself. If it doesn't,
[open an issue](https://github.com/Roboroads/repeat-that/issues/new/choose) with the message and your system.

### A hotkey does nothing

Look at the Hotkeys section. "In use by another app" means another program has the combination, so choose another. On
macOS, the input source shortcut can take Control+Option+Space, see above. On Linux under Wayland, the desktop assigns
the keys, or you bind the commands yourself, see [linux.md](linux.md#hotkeys). A hotkey you cleared with Backspace says
"Not set".

### Replays are silent

If the window says "Replays have nowhere to play", Repeat That found no output device. Connect one, or choose a device
under Output device. If the call app itself stays quiet after a replay, see the volume section above.

### The window says the speech detection model didn't load

Repeat That then uses a simpler detector, so some speech may be missed. Replays still work. If you see this, please
[open an issue](https://github.com/Roboroads/repeat-that/issues/new/choose) with your system and version.

### Anything else

Check [the open issues](https://github.com/Roboroads/repeat-that/issues) and open a new one if yours isn't there. Say
which version and operating system you use. Security problems go to the private channel in
[SECURITY.md](../SECURITY.md).
