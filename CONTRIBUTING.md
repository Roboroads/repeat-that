# Contributing to Repeat That

Thanks for wanting to help! Bug reports, feature ideas, testing on different hardware, and code are all welcome.

## Getting started

1. Install [Rust](https://rustup.rs) (stable), Node.js 22 LTS or later, and the
   [Tauri prerequisites](https://tauri.app/start/prerequisites/) for your OS. On Debian/Ubuntu:
   `sudo apt install pkg-config libwebkit2gtk-4.1-dev libgtk-3-dev libayatana-appindicator3-dev librsvg2-dev libasound2-dev libxdo-dev`
2. Run the Rust tests: `cargo test --workspace`
3. Run the app: `cd app && npm install && npm run tauri dev`. It starts in the tray; the first launch also opens the
   settings window.
4. Frontend checks, in `app/`: `npm run check` (svelte-check) and `npm test` (vitest).

`tauri::generate_context!` embeds `app/dist`, the built frontend. A fresh debug build works without it (the build
script puts a red "FRONTEND NOT BUILT" placeholder page there and warns); a release build refuses to go on. Run
`npm run build` in `app/` to build the real one; `tauri dev` and `tauri build` do that for you.

## Project layout

See [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md). In short:

- `crates/rt-core` — platform-independent buffering, voice activity detection, and playback logic.
- `crates/rt-capture` — per-OS application audio capture backends behind one trait.
- `crates/rt-engine` — the audio engine: capture, speech buffer, playback and ducking on their own threads.
- `app/` — the Tauri desktop app: the Svelte UI in `app/src`, the Rust shell (tray, windows, plugins) in `app/src-tauri`.

## Guidelines

- Run `cargo fmt --all` and `cargo clippy --workspace --all-targets -- -D warnings` before opening a PR, and
  `npm run check && npm test` in `app/` if you touched the UI.
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
