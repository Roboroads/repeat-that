# Repeat that?! on macOS

macOS support is planned for milestone 6 and needs macOS 14.2 or later. Releases will be signed with a Developer ID
certificate and notarized by Apple, so the app opens like any other once it is in Applications.

## Audio-capture permission

Repeat that?! needs permission to capture other apps' audio. The first time you start capturing, macOS asks for it. If
you clicked "Don't Allow", open System Settings → Privacy & Security, go to **Screen & System Audio Recording** on recent
versions, and turn the switch on for Repeat that?!. Then start capturing again.

If the app is listed there and capture still returns silence, remove its entry with the minus button and start capturing
again, so macOS asks once more.

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
