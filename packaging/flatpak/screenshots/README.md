# Screenshots

`settings.png` is the settings window of the Flatpak, for the AppStream metainfo (`me.roboroads.RepeatThat.metainfo.xml`
links to it on the `main` branch, and Flathub fetches it when it builds). It is a screenshot of this app, so it is under
the same license as the rest of the repository, GPL-3.0-or-later.

It was taken of the Flatpak running under a virtual display (Xvfb, 560 x 760), against a headless PipeWire where `pw-cat`
played the speech fixture of `crates/rt-core/tests/fixtures` as an app called "Meeting" (see
[MAINTAINING.md](../../../docs/MAINTAINING.md#flatpak)), after the app had buffered a few seconds of it, and cropped to
the window (480 x 720). Nothing was recorded or saved but the picture.

Retake it when the window changes, and keep the size in the metainfo in step with the file.
