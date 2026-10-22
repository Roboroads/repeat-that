# The Repeat That website

The site at https://repeat-that.roboroads.me, built with [Astro](https://astro.build) into plain static HTML. The
browser gets no framework, only one small inline script that marks the download card for the visitor's system. There
are no web fonts, analytics, cookies or requests to other sites.

The docs are not copied here. The site renders the Markdown files of the repository (see `src/lib/docs.ts` for the
list), the landing page takes its minimum system requirements from the "System requirements" section of the README, and
the download links come from the latest GitHub release.

## Commands

Run them from this folder with Node.js 22.19 or later.

```sh
npm ci
npm run dev          # http://localhost:4321/ with live reload
npm run build        # writes dist/
npm run preview      # serves dist/ on http://localhost:4321/
npm run check        # astro check: types in the pages, components and tests
npm test             # unit tests (vitest)
npm run check:dist   # looks for broken links and requests to other sites in dist/
npx playwright install --with-deps chromium   # once
npm run test:e2e     # browser tests (Playwright, Chromium)
```

`npm run test:e2e` builds the site three times with fixtures from `tests/fixtures/` (every platform with a placeholder
video, Windows only, no release) and serves each build on its own port. It doesn't touch `dist/`.

Set `ASTRO_TELEMETRY_DISABLED=1`, or run `npx astro telemetry disable` once, to stop Astro from sending usage data.

## What a build reads

| Input | Where it comes from |
|---|---|
| Release data | The file in `SITE_RELEASE_JSON` (relative to this folder), or `src/data/release.sample.json` (v0.1.0, Windows only). The deploy workflow writes the file with `.github/scripts/site_release.py`. The build checks it against the schema and fails when it doesn't match. A release of `null` shows a "No release yet" card. |
| Docs | The Markdown files named in `src/lib/docs.ts`. An optional doc that doesn't exist yet (the guide, the Linux install page, the code signing page) is skipped with a build warning, and links to it go to GitHub. |
| System requirements | `## System requirements` in the README: a `###` heading per platform, a table of two columns under it, optional paragraphs after the table. A paragraph that links to the install guides is shown under the whole section. `SITE_README` names another file, the browser tests use that. Without the section the build warns and the landing page leaves it out. |
| Demo video | `public/media/demo.mp4`, and optionally `demo-poster.jpg` and `demo.vtt` (captions). The folder is in `.gitignore`, the deploy workflow fills it from the `site-media` prerelease. `SITE_MEDIA_DIR` names another folder. Without `demo.mp4` there is no video section. |

Links in the docs are rewritten when they are rendered. A relative link to a doc on the site goes to its page, anything
else in the repository goes to `https://github.com/Roboroads/repeat-that/blob/main/...`. Anchors are kept.

## What a build writes

Besides the pages, `dist/.well-known/security.txt` is generated with an `Expires` date 180 days ahead, so the daily
rebuild keeps it valid. Everything you put in `public/.well-known/` is copied as it is, for example a verification file.
`public/.nojekyll` and `public/CNAME` are copied too. GitHub ignores `CNAME` when it deploys from Actions, the domain is
set under Settings > Pages.

`npm run check:dist` fails when `CNAME`, `.nojekyll` or `.well-known/security.txt` is missing, when an internal link,
image, script or stylesheet points at nothing (anchors included), and when a page loads a script, stylesheet, font,
image, iframe or media from another origin. Plain links to other sites are fine.

## The preview image

`public/og.png` (1280 x 640) is rendered from `src/assets/og.svg` and committed. After a change to the SVG:

```sh
npx playwright screenshot --viewport-size=1280,640 "file://$PWD/src/assets/og.svg" public/og.png
optipng -o7 -strip all public/og.png
```

The picture uses whatever sans-serif font the machine has. The icon in it is `app/src-tauri/icons/svg/app.svg`, see
`public/LICENSE.md`.
