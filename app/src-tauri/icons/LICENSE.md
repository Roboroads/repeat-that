# Icon licenses

Every icon in this directory is original artwork made for Repeat That and is licensed
under the [GNU General Public License v3.0 or later](../../../LICENSE) like the rest of the repository. No third-party
artwork is bundled.

- `svg/`: the sources. `app.svg` is the app icon; `tray-idle.svg`, `tray-capturing.svg` and `tray-paused.svg` are the
  tray icon variants.
- `32x32.png`, `128x128.png`, `128x128@2x.png`, `icon.png`, `icon.ico`, `icon.icns`: the app icon rendered by
  `npx tauri icon src-tauri/icons/svg/app.svg --output <dir>` from `app/` (copy the files you need into this directory).
- `tray/*.png`: the tray variants rendered at 64 x 64 px with `npx tauri icon <svg> --png 64 --output <dir>`.
- The settings window and its favicon use `svg/app.svg` itself (imported by `app/src/App.svelte` and linked from
  `app/index.html`), so there is no second copy. The SVG carries an SPDX license tag, which stays with the file when
  Vite bundles it into the app.

When you change a source SVG, render it again and commit the PNGs with it.
