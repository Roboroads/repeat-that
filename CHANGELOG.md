# Changelog

## [0.2.0](https://github.com/Roboroads/repeat-that/compare/v0.1.0...v0.2.0) (2026-10-22)


### ⚠ BREAKING CHANGES

* dispatch capture backends per OS ([#36](https://github.com/Roboroads/repeat-that/issues/36))

### Features

* **app:** add a "not allowed" capture state, muted ducking on macOS and an untested notice ([#43](https://github.com/Roboroads/repeat-that/issues/43)) ([db0f93c](https://github.com/Roboroads/repeat-that/commit/db0f93cfb0b0c3b584f39d4f4add9303188fc1ab))
* **app:** add a self-check that reports the speech detector ([#41](https://github.com/Roboroads/repeat-that/issues/41)) ([d18141c](https://github.com/Roboroads/repeat-that/commit/d18141c8f7e4b690da66e09a17b34d059ee02f9e))
* **app:** ask before checking for updates and show them in the settings window ([#55](https://github.com/Roboroads/repeat-that/issues/55)) ([9dbcba8](https://github.com/Roboroads/repeat-that/commit/9dbcba849fc4109d35c65765c66d995f48b69a31))
* **app:** check for and install updates, off until an update key is set ([#60](https://github.com/Roboroads/repeat-that/issues/60)) ([d208ae7](https://github.com/Roboroads/repeat-that/commit/d208ae76c23aa89b8fc25a4d13acf62a5a655b67))
* **app:** Wayland portal hotkeys, command-line actions and a no-tray fallback ([#38](https://github.com/Roboroads/repeat-that/issues/38)) ([04f4ec3](https://github.com/Roboroads/repeat-that/commit/04f4ec36ff651ee7479d8ce5e6238539b481d0d0))
* build a Flatpak and prepare the Flathub submission ([#51](https://github.com/Roboroads/repeat-that/issues/51)) ([1a3d943](https://github.com/Roboroads/repeat-that/commit/1a3d943e8dd486e2d75a75046b3e5acfc5ea43a8))
* **capture:** capture and duck apps through a Core Audio tap on macOS ([#40](https://github.com/Roboroads/repeat-that/issues/40)) ([3af5ccc](https://github.com/Roboroads/repeat-that/commit/3af5ccc24ac31b3ae28fe9da1254e156bca90bba))
* **capture:** capture and duck apps through PipeWire on Linux ([#39](https://github.com/Roboroads/repeat-that/issues/39)) ([a280469](https://github.com/Roboroads/repeat-that/commit/a280469441939d12ff66ed37929c433e47da0aaf))
* ship Linux AppImage, deb and rpm packages ([#47](https://github.com/Roboroads/repeat-that/issues/47)) ([844f82c](https://github.com/Roboroads/repeat-that/commit/844f82c58f74f4a9dff31427b30e8544f66a1f1c))


### Bug Fixes

* **app:** name the shortcuts Repeat That on KDE Plasma 6.3 ([#45](https://github.com/Roboroads/repeat-that/issues/45)) ([5953dae](https://github.com/Roboroads/repeat-that/commit/5953daed2f843638f6a3d27edf8f9697de8afd64))
* **app:** point Flatpak users at the release page for updates ([#68](https://github.com/Roboroads/repeat-that/issues/68)) ([573cd1d](https://github.com/Roboroads/repeat-that/commit/573cd1d362efff58262b46ca4c1ce2a4eed5091e))
* **capture:** address the review of PipeWire capture and ducking on Linux ([#44](https://github.com/Roboroads/repeat-that/issues/44)) ([6e62540](https://github.com/Roboroads/repeat-that/commit/6e62540fa4c6194edbe1b10154539867dce76595))
* **ci:** pass secrets to the called release workflows so the macOS build can sign ([#69](https://github.com/Roboroads/repeat-that/issues/69)) ([dfd9e2d](https://github.com/Roboroads/repeat-that/commit/dfd9e2de798c85d08b274a8106d71a7257e67103))


### Code Refactoring

* dispatch capture backends per OS ([#36](https://github.com/Roboroads/repeat-that/issues/36)) ([7a3fd32](https://github.com/Roboroads/repeat-that/commit/7a3fd32803927430fea88cf74dc6aed553b881a9))

## [0.1.0](https://github.com/Roboroads/repeat-that/compare/v0.0.1...v0.1.0) (2026-10-21)


### ⚠ BREAKING CHANGES

* **core:** play through to live, including the sentence in progress ([#10](https://github.com/Roboroads/repeat-that/issues/10))

### Features

* **app:** add the settings window ([#14](https://github.com/Roboroads/repeat-that/issues/14)) ([8792366](https://github.com/Roboroads/repeat-that/commit/8792366c83d82bbbbe69e1941c8649299885496d))
* **app:** add the Tauri desktop shell with tray menu ([#9](https://github.com/Roboroads/repeat-that/issues/9)) ([241787a](https://github.com/Roboroads/repeat-that/commit/241787a8cbc838d11f75ef94f22dab7104b1c3c4))
* **app:** connect the engine to the tray, hotkeys and settings ([#30](https://github.com/Roboroads/repeat-that/issues/30)) ([7e35345](https://github.com/Roboroads/repeat-that/commit/7e35345a06fd4ac5b7ce0dac88ef6edc90fb4f54))
* **app:** persist settings and register global hotkeys ([#15](https://github.com/Roboroads/repeat-that/issues/15)) ([c598024](https://github.com/Roboroads/repeat-that/commit/c598024a1c1bb8b7c2683f8ea9ae410febd781c1))
* **capture:** add Windows process loopback capture, app list, ducking and mock backend ([#3](https://github.com/Roboroads/repeat-that/issues/3)) ([6a8e607](https://github.com/Roboroads/repeat-that/commit/6a8e60799534a0366dbc53645552fed392ded3f7))
* **capture:** extract app icons from executables ([#8](https://github.com/Roboroads/repeat-that/issues/8)) ([d31bb58](https://github.com/Roboroads/repeat-that/commit/d31bb589125ca7a69b1da91bffa8644e79a5d73c))
* **core:** add VAD segmentation, speech buffer and replay player ([#2](https://github.com/Roboroads/repeat-that/issues/2)) ([f34f782](https://github.com/Roboroads/repeat-that/commit/f34f782f55eba410f83c99af25ae21d857bd06a7))
* **core:** play through to live, including the sentence in progress ([#10](https://github.com/Roboroads/repeat-that/issues/10)) ([01c583b](https://github.com/Roboroads/repeat-that/commit/01c583b09ab05867dda4353c66edd4ef18a1c461))
* **engine:** wire capture, speech buffer and playback into an audio engine ([#29](https://github.com/Roboroads/repeat-that/issues/29)) ([1ae0ebc](https://github.com/Roboroads/repeat-that/commit/1ae0ebc97d7f293543218ef3af235f650e22da25))


### Bug Fixes

* **capture:** make the duck fade test robust against timing jitter ([#11](https://github.com/Roboroads/repeat-that/issues/11)) ([18016f3](https://github.com/Roboroads/repeat-that/commit/18016f3ce48c731237bc425cc089dfe47e24add8))
