# Repeat That on macOS

macOS support is planned for milestone 6 and needs macOS 14.2 or later. Releases will be signed with a Developer ID
certificate and notarized by Apple, so the app opens like any other once it is in Applications.

## Audio-capture permission

Repeat That needs permission to capture other apps' audio. The first time you start capturing, macOS asks for it. If
you clicked "Don't Allow", reset the permission from Terminal, then quit and reopen Repeat That and start capturing
again. macOS asks once more:

```sh
tccutil reset AudioCapture me.roboroads.repeat-that
```

`AudioCapture` is the service name that Apple's
[tccutil documentation](https://developer.apple.com/documentation/xcode/resetting-access-to-protected-resources-in-macos)
lists for capturing system audio, and `me.roboroads.repeat-that` is the app's bundle identifier.

Use this route on macOS 14. The Screen Recording switch there is a different permission from audio capture (Apple lists
`ScreenCapture` and `AudioCapture` as separate services), so turning it on doesn't help.

On macOS 15 and later you can also turn the permission on in System Settings → Privacy & Security →
**Screen & System Audio Recording**, where an app can be allowed to record the screen and audio, or only audio.

## Build from source

You need Apple's command-line tools, Rust and Node.js 22.19 or later. The `app/` directory these steps use isn't on
`main` yet (milestone 4), so they work once it lands.

```sh
xcode-select --install
curl https://sh.rustup.rs -sSf | sh
git clone https://github.com/Roboroads/repeat-that && cd repeat-that
cd app && npm install && npm run tauri build -- --bundles app,dmg
```

The app ends up in `target/release/bundle/macos/` and the disk image in `target/release/bundle/dmg/`, at the root of
the repository because the app crate is a member of the Cargo workspace. For a universal build (Apple Silicon and
Intel in one app), add both Rust targets and pass the target to Tauri:

```sh
rustup target add aarch64-apple-darwin x86_64-apple-darwin
npm run tauri build -- --target universal-apple-darwin --bundles app,dmg
```

That output is in `target/universal-apple-darwin/release/bundle/`, again with `macos/` and `dmg/` inside.

An app you build yourself isn't quarantined, so Gatekeeper doesn't check it.

It isn't signed with our Developer ID certificate either, at most ad-hoc signed, and an ad-hoc signature changes with
every build. macOS ties the audio-capture permission to the signature, so after a rebuild it may stop honoring the
permission you granted, and capture returns silence. Reset the permission as described under
[Audio-capture permission](#audio-capture-permission) and grant it again.
