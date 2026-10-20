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

You need Apple's command-line tools, Rust and Node.js 20 or later.

```sh
xcode-select --install
curl https://sh.rustup.rs -sSf | sh
git clone https://github.com/Roboroads/repeat-that && cd repeat-that
# once the desktop app lands (milestone 4):
cd app && npm install && npm run tauri build
```

The built app ends up in `target/release/bundle/macos/`. An app you build yourself isn't quarantined, so Gatekeeper
doesn't check it.
