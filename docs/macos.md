# Repeat that?! on macOS

macOS support is planned for milestone 6 and needs macOS 14.2 or later. Releases will be signed with a Developer ID
certificate and notarized by Apple, so the app opens like any other once it is in Applications.

## Audio-capture permission

Repeat that?! needs permission to capture other apps' audio. The first time you start capturing, macOS asks for it. If
you clicked "Don't Allow", open System Settings → Privacy & Security and turn the switch on for Repeat that?! in the
pane that covers audio capture. On macOS 15 and later that pane is **Screen & System Audio Recording**. On macOS 14 it
is called **Screen Recording** or **Screen & System Audio Recording**, depending on the release. Then start capturing
again.

If Repeat that?! isn't listed there, or capture still returns silence with the switch on, reset the permission from
Terminal and start capturing again, so macOS asks once more:

```sh
tccutil reset AudioCapture me.roboroads.repeatthat
```

`AudioCapture` is the service name that Apple's
[tccutil documentation](https://developer.apple.com/documentation/xcode/resetting-access-to-protected-resources-in-macos)
lists for capturing system audio, and `me.roboroads.repeatthat` is the app's bundle identifier.

## Build from source

You need Apple's command-line tools, Rust and Node.js 22 LTS or later.

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
permission you granted, and capture returns silence. Reset it and grant it again:

```sh
tccutil reset AudioCapture me.roboroads.repeatthat
```
