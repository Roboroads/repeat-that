# Repeat That

**Missed what someone just said on a call? Press a key and hear it again.**

A copy of the "System requirements" section of the README, for tests. The site reads the real README at build time.

## System requirements

### Windows

| Requirement | Minimum |
|---|---|
| Operating system | Windows 10 version 2004 (build 19041) or newer, or Windows 11 |
| Processor | 64-bit x86 (x64). Windows on ARM isn't supported yet |
| WebView2 runtime | Windows 11 and up-to-date Windows 10 have it. If it's missing, the installer downloads it, so you need an internet connection that one time |

### macOS

| Requirement | Minimum |
|---|---|
| Operating system | macOS 14.2 or newer |
| Processor | Apple Silicon (M1 or later). Intel Macs aren't supported |
| Permission | Audio capture from other apps, which macOS asks for the first time Repeat That captures |

macOS support is built but hasn't been tested on a real Mac yet.

### Linux

| Requirement | Minimum |
|---|---|
| Operating system | Debian 13, Ubuntu 24.04 or Fedora 40, or newer. Debian 12 is too old: the speech detector needs glibc 2.38 and the libstdc++ of GCC 13 |
| Processor | 64-bit x86 (x86_64). Arm isn't supported yet |
| Audio | PipeWire with WirePlumber. On PulseAudio the app starts but can't capture |
| Hotkeys | X11, or Wayland with the GlobalShortcuts portal (KDE Plasma, GNOME 48 and later). On other Wayland desktops, such as GNOME 46 on Ubuntu 24.04, you bind the `repeat-that --replay`, `--catch-up` and `--stop` commands yourself |

Other distributions can use the AppImage if they have glibc 2.38 or newer.

The steps to install are in [docs/install-windows.md](docs/install-windows.md),
[docs/install-macos.md](docs/install-macos.md) and [docs/install-linux.md](docs/install-linux.md). The Linux hotkeys are
explained in [docs/linux.md](docs/linux.md).

## Install (Windows)

You need 64-bit Windows 10 version 2004 or newer, or Windows 11. This line must not end up in the requirements.
