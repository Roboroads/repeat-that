# Contributing to Repeat that?!

Thanks for wanting to help! Bug reports, feature ideas, testing on different hardware, and code are all welcome.

## Getting started

1. Install [Rust](https://rustup.rs) (stable), Node.js 20+, and the
   [Tauri prerequisites](https://tauri.app/start/prerequisites/) for your OS.
2. Run the tests: `cargo test --workspace`
3. Once the desktop app lands (milestone 4): `cd app && npm install && npm run tauri dev`

## Project layout

See [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md). In short:

- `crates/rt-core` — platform-independent buffering, voice activity detection, and playback logic.
- `crates/rt-capture` — per-OS application audio capture backends behind one trait.
- `app/` — the Tauri desktop app (Rust shell + Svelte UI).

## Guidelines

- Run `cargo fmt --all` and `cargo clippy --workspace --all-targets -- -D warnings` before opening a PR.
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
