# Installing on Linux

Repeat That runs on 64-bit x86 (x86_64) Linux with PipeWire. The oldest systems it is built and tested for are
Debian 13, Ubuntu 24.04 and Fedora 40; newer releases of those work too. Arm computers aren't supported yet.

Debian 12 and other systems with a glibc older than 2.38 can't run it: the speech detector (ONNX Runtime, built into
the app) needs glibc 2.38 and the C++ library of GCC 13. The `.deb` and the `.rpm` say so, so apt or dnf refuses to
install them there, and the AppImage stops at once with a "version ... not found" error. The
[Flatpak](#install-the-flatpak) brings its own libraries and runs there.

Capture, ducking and replay all go through PipeWire, with WirePlumber as its session manager. Ubuntu 24.04, Fedora 40
and GNOME on Debian 13 use both by default. On a system that still runs PulseAudio, Repeat That starts but the status
reads "Capture needs PipeWire, and it isn't running". Your distribution's documentation says how to switch, for Debian
see [wiki.debian.org/PipeWire](https://wiki.debian.org/PipeWire). `wpctl status` lists your sound devices when PipeWire
and WirePlumber are running.

## Download

Get the package for your system from the [Releases page](https://github.com/Roboroads/repeat-that/releases). Each
release has:

- `Repeat-That_<version>_amd64.deb` for Debian, Ubuntu and the distributions based on them.
- `Repeat-That-<version>-1.x86_64.rpm` for Fedora. It names the libraries it needs rather than Fedora's package names,
  so other rpm distributions may take it too, but only Fedora is tested.
- `Repeat-That_<version>_amd64.AppImage`, a single file that runs on other distributions without installing.
- `Repeat-That_<version>_x86_64.flatpak`, a [Flatpak](#install-the-flatpak) for any distribution with Flatpak.
- `SHA256SUMS` holds the checksum of each file, to [verify your download](verify-downloads.md).

The `.deb` and the `.rpm` are the better choice where they fit: they install the app's menu entry under the name the
desktop expects, and your package manager brings in what the app needs. The Flatpak is the choice for distributions
without those packages, and for those who prefer an app in a sandbox.

## Install the .deb (Debian, Ubuntu)

```sh
sudo apt install ./Repeat-That_<version>_amd64.deb
```

The `./` matters: without it, apt looks for a package of that name in its repositories. apt installs the libraries the
app needs (WebKitGTK, GTK, the tray library, PipeWire's and ALSA's client libraries). The package also recommends
PipeWire and WirePlumber themselves, which apt installs too unless you tell it not to.

## Install the .rpm (Fedora)

```sh
sudo dnf install ./Repeat-That-<version>-1.x86_64.rpm
```

dnf installs the same libraries, and PipeWire and WirePlumber as weak dependencies.

## Run the AppImage

```sh
chmod +x Repeat-That_<version>_amd64.AppImage
./Repeat-That_<version>_amd64.AppImage
```

The AppImage carries its own copy of WebKitGTK, GTK and most other libraries, from Ubuntu 24.04. It takes PipeWire's
and ALSA's client libraries and the C++ library from your system on purpose: a PipeWire client must match the PipeWire
that runs on your desktop. A desktop system with PipeWire has all three.

An AppImage mounts itself with FUSE. If it says it can't, install your distribution's `fuse3` package, or start it
with `--appimage-extract-and-run` added, which unpacks it to a temporary folder first.

Running the file doesn't add Repeat That to your app menu. A tool such as
[Gear Lever](https://flathub.org/apps/it.mijorus.gearlever) or AppImageLauncher can do that. Two things work
differently from the packages:

- On a Wayland desktop with the GlobalShortcuts portal (KDE Plasma, GNOME 48 and later), the desktop finds the app's
  name and icon for its shortcuts in `me.roboroads.repeat-that.desktop`, which only the `.deb` and the `.rpm` install.
  With the AppImage the shortcuts work, but the desktop may list them without the name or icon.
- **Start at login** and the command-line actions use the path of the AppImage file. If you move or rename the file,
  turn **Start at login** off and on again, and update any custom shortcuts that run it.

## Install the Flatpak

The Flatpak runs on any distribution with [Flatpak](https://flatpak.org/setup/) and PipeWire, including ones the
packages can't run on, such as Debian 12: it brings its own libraries (the GNOME 51 runtime). It was tried against
PipeWire 1.4. Download `Repeat-That_<version>_x86_64.flatpak` from the Releases page, then:

```sh
flatpak install ./Repeat-That_<version>_x86_64.flatpak
flatpak run me.roboroads.RepeatThat
```

The file names Flathub as the place to get the GNOME runtime from, so Flatpak offers to add it if it isn't set up yet,
and downloads the runtime (about 1 GB, shared with other Flatpaks that use it). Repeat That then appears in your app
menu. It isn't on Flathub yet, so `flatpak update` doesn't find newer versions: install the newer file the same way, with
`--reinstall` added if Flatpak says Repeat That is installed already.

A Flatpak runs in a sandbox, and Repeat That's is small: it has your display, your GPU, PipeWire's socket and the tray's
bus name, and nothing else. It has no network and can't see your files. PipeWire's socket is what lets it hear the apps
that play sound, capture the one you pick and lower its volume, which is what the app does in every package; any
Flatpak with the socket could do the same. Differences from the packages:

- **Commands for custom shortcuts** are `flatpak run me.roboroads.RepeatThat --replay`, `--stop` and `--catch-up`. The
  Hotkeys section shows them when your desktop can't give the app hotkeys.
- **Start at login** asks the system through the Background portal. The first time, your desktop asks whether to allow
  it, and the entry is made on the host and starts the app hidden, as in the other packages. The question can stay open
  as long as you like: the settings window and the tray go on working meanwhile. If you say no, or close it, the
  switch turns itself off again. The app can't read the entry back, so if you remove it in your desktop's settings,
  the switch in Repeat That stays on: turn it off and on again.
- **Running in the background.** With its window closed, Repeat That sits in the tray, which a desktop counts as a
  background app. At every start it asks the same portal for permission to run in the background: your desktop asks
  you once, and remembers the answer. If you said no, some desktops end an app like that when its window is closed, and
  the hotkeys end with it. Repeat That doesn't change what it does then, only says so on its error output. Allow it in
  your desktop's settings for the app, or with
  `flatpak permission-set background background me.roboroads.RepeatThat yes`.
- **After a crash during a replay**, the app you were catching up on stays at the lowered volume until you start Repeat
  That again, which then gives it back. The packages keep a small helper process alive that restores the volume at once,
  and a sandbox can't outlive its app. A `kill -9` or a power cut is the case: quitting Repeat That restores the volume
  as usual.
- **The button that opens your desktop's shortcut settings** only works where the desktop's portal can open them,
  because a Flatpak can't start programs on the host. Otherwise the window says where to find the settings.
- **App icons** in the list can fall back to the generic one: the sandbox can't read your icon theme.
- **Files**: settings are in `~/.var/app/me.roboroads.RepeatThat/config/me.roboroads.RepeatThat`, and the duck journal
  (no audio, see [PRIVACY.md](../PRIVACY.md)) in `~/.var/app/me.roboroads.RepeatThat/data/me.roboroads.RepeatThat`.

## First start

Start Repeat That from your app menu (or run the AppImage, or `flatpak run me.roboroads.RepeatThat`). It opens its settings window on the first run and puts an
icon in the tray. Closing the window keeps it running, and **Quit** in the tray menu ends it. On GNOME there is no
tray unless the AppIndicator and KStatusNotifierItem Support extension is on; without one, the window opens at every
start and has a **Quit Repeat That** button.

Start the app you want to catch up on (Discord, Teams, Firefox, ...), wait until it plays sound, and pick it in the
**App** list. Captured audio stays in memory and is never written to disk; see [PRIVACY.md](../PRIVACY.md).

The hotkeys depend on your session. On X11 they work as on Windows. On Wayland, KDE Plasma and GNOME 48 and later
ask you to confirm the three shortcuts in a system dialog on the first start. Other Wayland desktops (GNOME 46 on
Ubuntu 24.04, Sway, Hyprland) can't give an app global hotkeys, so you bind `repeat-that --replay` and the other
commands as custom shortcuts. [linux.md](linux.md) covers each case, the tray and start at login.

## Verify your download

Compare your download with the release's `SHA256SUMS` before you install it. In the folder with both files:

```sh
sha256sum -c SHA256SUMS --ignore-missing
```

Your file must show `OK`. More in [verify-downloads.md](verify-downloads.md).

## Updating and uninstalling

The AppImage can check for updates itself once you allow it: the settings window asks the first time, and General
has the switch and a **Check now** button. **Install and restart** replaces the AppImage file in place, which works
when you can write to the file and its folder. The `.deb` and `.rpm` check too, but a package can't replace itself: the
banner says a newer version is out, and **Download page** opens the release page in your browser. The Flatpak never
checks. What a check sends is in [PRIVACY.md](../PRIVACY.md#update-checks).

To update by hand, or from a version without the updater, download the newer package and install it the same way: apt
and dnf replace the old version. For the AppImage, replace the file, then turn **Start at login** off and on again if
the new file has another name (the name carries the version). For the Flatpak, install the newer file with
`flatpak install --reinstall ./Repeat-That_<version>_x86_64.flatpak`. Your settings are kept in each case.

To uninstall:

1. Turn off **Start at login** in the settings window, then quit Repeat That. If you removed the app with the switch
   still on, delete `~/.config/autostart/Repeat That.desktop` (for the Flatpak,
   `~/.config/autostart/me.roboroads.RepeatThat.desktop`).
2. Remove the app: `sudo apt remove repeat-that`, `sudo dnf remove repeat-that`, `flatpak uninstall
   me.roboroads.RepeatThat`, or delete the AppImage file.
3. Delete the settings folder, `~/.config/me.roboroads.repeat-that`, and `~/.local/share/me.roboroads.repeat-that`,
   which holds the duck journal (see [PRIVACY.md](../PRIVACY.md)). Neither holds audio. The Flatpak's are in
   `~/.var/app/me.roboroads.RepeatThat`, which `flatpak uninstall --delete-data me.roboroads.RepeatThat` removes.

## Build from source

The packages come from CI, but you can build your own on Debian 13 or later, or Ubuntu 24.04 or later. Install the
system packages listed under Building from source in the [README](../README.md#building-from-source), plus
[Rust](https://rustup.rs) and Node.js 22.19 or later, then:

```sh
git clone https://github.com/Roboroads/repeat-that
cd repeat-that/app
npm ci
npm run tauri build
```

The packages end up in `target/release/bundle/deb/`, `target/release/bundle/rpm/` and `target/release/bundle/appimage/`.
A build runs on systems with the same or a newer glibc than the one it was built on, which is why the release packages
are built on Ubuntu 24.04, whose glibc (2.39) is the oldest of the supported systems.
