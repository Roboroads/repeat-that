# Contributing to Repeat That

Thanks for wanting to help! Bug reports, feature ideas, testing on different hardware, and code are all welcome.

## Getting started

1. Install [Rust](https://rustup.rs) (stable), Node.js 22.19 or later, and the
   [Tauri prerequisites](https://tauri.app/start/prerequisites/) for your OS. On Debian/Ubuntu:
   `sudo apt install pkg-config libwebkit2gtk-4.1-dev libgtk-3-dev libayatana-appindicator3-dev librsvg2-dev libasound2-dev libxdo-dev`
2. Run the Rust tests: `cargo test --workspace`
3. Run the app: `cd app && npm install && npm run tauri dev`. It starts in the tray; the first launch also opens the
   settings window.
4. Frontend checks, in `app/`: `npm run check` (svelte-check, and a type check of the end-to-end test) and `npm test`
   (vitest).
5. End-to-end test, on Linux: see [Running the end-to-end test](#running-the-end-to-end-test).

`tauri::generate_context!` embeds `app/dist`, the built frontend. A fresh debug build works without it (the build
script puts a red "FRONTEND NOT BUILT" placeholder page there and warns); a release build refuses to go on. Run
`npm run build` in `app/` to build the real one; `tauri dev` and `tauri build` do that for you.

## Running the end-to-end test

`app/e2e` holds a [WebdriverIO](https://webdriver.io) test that drives the real desktop app through
[tauri-driver](https://crates.io/crates/tauri-driver), the way a person would: it picks the fake app, waits for speech
to be buffered, replays, pauses, changes a setting and records a hotkey. It runs on Linux only, because a Linux debug
build is the one that needs no capture backend: it plays the speech fixture as an app called `speech_8s`, and
`REPEAT_THAT_NULL_OUTPUT=1` (set by the test) plays replays into nothing, so no audio device is needed either.

It needs Node.js 22.19 or later, which is what WebdriverIO asks for (`engines` in `app/package.json`). One-time setup on
Debian/Ubuntu:

```sh
sudo apt install xvfb webkit2gtk-driver dbus-daemon   # plus the packages from step 1 above
cargo install tauri-driver --locked --version 2.1.0
```

Then, in `app/`:

```sh
xvfb-run -a npm run test:e2e    # on a desktop session, `npm run test:e2e` is enough
```

`npm run test:e2e` builds the debug app first (`tauri build --debug --no-bundle`), so the first run takes a few
minutes. The specs themselves take about half a minute. Each spec file starts its own app, its own tauri-driver (on a
free port) and its own D-Bus session, with the app's home, temp and settings folders in one scratch folder. The specs
don't depend on each other, and they never touch your real settings or a copy of Repeat That that is already running.
Specs are in `app/e2e/specs/*.e2e.ts`, the setup is in `app/e2e/wdio.conf.ts`. The `permission` spec gets an app started
with `REPEAT_THAT_MOCK_PERMISSION=denied`, which refuses to capture, as macOS does without the permission, until "Ask
again" is clicked. To see that state yourself, run `REPEAT_THAT_MOCK_PERMISSION=denied npm run tauri dev` in `app/`
(debug builds only).

CI runs the same thing as the required check `E2E (Linux)`. Like every other test, it must not write captured audio to
disk, and the `privacy` spec checks that.

## Project layout

See [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md). In short:

- `crates/rt-core` — platform-independent buffering, voice activity detection, and playback logic.
- `crates/rt-capture` — per-OS application audio capture backends behind one trait.
- `crates/rt-engine` — the audio engine: capture, speech buffer, playback and ducking on their own threads.
- `app/` — the Tauri desktop app: the Svelte UI in `app/src`, the Rust shell (tray, windows, plugins) in `app/src-tauri`.

## Guidelines

- Run `cargo fmt --all` and `cargo clippy --workspace --all-targets -- -D warnings` before opening a PR, and
  `npm run check && npm test` in `app/` if you touched the UI, plus the end-to-end test if you changed what the window
  or the tray does.
- App icons are in-repo SVGs under the project license; see `app/src-tauri/icons/LICENSE.md` before adding assets.
- Keep platform-specific code behind `#[cfg(target_os = "...")]` inside `rt-capture` (or clearly isolated in `app/`).
- **Never write captured audio to disk** — not even in tests or debug builds. Use the WAV fixtures in
  `crates/rt-core/tests/fixtures` (synthetic or freely licensed recordings only) for testing.
- PR titles must follow [Conventional Commits](https://www.conventionalcommits.org) (`feat: ...`, `fix: ...`,
  `docs: ...`). PRs are squash-merged and the title becomes the release note. Mark breaking changes with `!`
  (`feat!: ...`).
- Small, focused PRs are easier to review. For larger changes, open an issue first to discuss the approach.

## Testing on real hardware

Capture backends can only be fully tested on the matching OS. If you test on a platform/version combination, please
mention it in your PR or issue (e.g. "Windows 11 23H2, Discord desktop").

## License

By contributing, you agree that your contributions are licensed under the
[GNU General Public License v3.0 or later](LICENSE).
