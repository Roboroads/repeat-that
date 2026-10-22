import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { checkDist } from "../../scripts/check-dist.mjs";

const scratch = join(process.cwd(), ".tmp");
let dist: string;

const GOOD: Record<string, string> = {
  CNAME: "repeat-that.roboroads.me\n",
  ".nojekyll": "",
  ".well-known/security.txt": "Contact: https://example.org\n",
  "og.png": "png",
  "_astro/site.css": "body { background: #fff; }",
  "index.html": `<!doctype html><html><head>
    <link rel="canonical" href="https://repeat-that.roboroads.me/">
    <link rel="stylesheet" href="/_astro/site.css">
    <meta property="og:image" content="https://repeat-that.roboroads.me/og.png">
    <script>const link = '<a href="/does-not-exist/">';</script>
  </head><body>
    <h2 id="downloads">Downloads</h2>
    <a href="#downloads">Down</a>
    <a href="/verify/">Verify</a>
    <a href="/verify/#windows">Verify on Windows</a>
    <a href="guide/">Relative</a>
    <a href="https://github.com/Roboroads/repeat-that/releases">Releases</a>
    <a href="mailto:someone@example.org">Mail</a>
    <img src="/og.png" alt="">
  </body></html>`,
  "verify/index.html": `<!doctype html><h2 id="windows">Windows</h2><a href="../">Home</a>`,
  "guide/index.html": `<!doctype html><p>Guide</p>`,
};

function build(files: Record<string, string | null>): string {
  const all = { ...GOOD, ...files };
  for (const [path, content] of Object.entries(all)) {
    if (content === null) continue;
    mkdirSync(dirname(join(dist, path)), { recursive: true });
    writeFileSync(join(dist, path), content);
  }
  return dist;
}

beforeEach(() => {
  mkdirSync(scratch, { recursive: true });
  dist = mkdtempSync(join(scratch, "dist-"));
});
afterEach(() => rmSync(dist, { recursive: true, force: true }));

describe("checkDist", () => {
  it("finds nothing wrong with a good site", () => {
    expect(checkDist(build({}))).toEqual([]);
  });

  it("catches a missing CNAME", () => {
    rmSync(join(build({}), "CNAME"));
    expect(checkDist(dist).join("\n")).toMatch(/CNAME/);
  });

  it("catches a missing .nojekyll", () => {
    rmSync(join(build({}), ".nojekyll"));
    expect(checkDist(dist).join("\n")).toMatch(/\.nojekyll/);
  });

  it("catches a missing security.txt", () => {
    rmSync(join(build({}), ".well-known/security.txt"));
    expect(checkDist(dist).join("\n")).toMatch(/security\.txt/);
  });

  it("catches an internal link to a page that does not exist, and names the page", () => {
    const errors = checkDist(build({ "verify/index.html": `<h2 id="windows"></h2><a href="/nope/">x</a>` }));
    expect(errors).toHaveLength(1);
    expect(errors[0]).toMatch(/verify\/index\.html/);
    expect(errors[0]).toMatch(/\/nope\//);
  });

  it("catches a broken relative link and a missing file", () => {
    const errors = checkDist(build({ "verify/index.html": `<h2 id="windows"></h2><a href="../missing/">x</a><img src="/nope.png" alt="">` }));
    expect(errors).toHaveLength(2);
  });

  it("catches an anchor that no heading has, on another page and on the same page", () => {
    const other = checkDist(build({ "index.html": `<a href="/verify/#linux">x</a>` }));
    expect(other.join("\n")).toMatch(/#linux/);
    const same = checkDist(build({ "index.html": `<a href="#nowhere">x</a>` }));
    expect(same.join("\n")).toMatch(/#nowhere/);
  });

  it("does not take the name of a meta tag or a form field for an anchor", () => {
    const html = `<meta name="description" content="x"><input name="query"><a href="#description">x</a><a href="#query">y</a>`;
    const errors = checkDist(build({ "index.html": html }));
    expect(errors).toHaveLength(2);
    expect(errors.join("\n")).toMatch(/#description/);
    expect(errors.join("\n")).toMatch(/#query/);
  });

  it("takes an id on any element and the name of an a element for an anchor", () => {
    const html = `<span id="one"></span><a name="two"></a><a href="#one">x</a><a href="#two">y</a>`;
    expect(checkDist(build({ "index.html": html }))).toEqual([]);
  });

  it("does not take an id inside an inline script for an anchor", () => {
    const html = `<script>const html = '<div id="fake"></div>';</script><a href="#fake">x</a>`;
    expect(checkDist(build({ "index.html": html }))).toHaveLength(1);
  });

  it("reports a link with a stray percent sign instead of crashing on it", () => {
    const errors = checkDist(build({ "index.html": `<a href="/100%/">x</a><a href="/verify/#100%">y</a>` }));
    expect(errors).toHaveLength(2);
    expect(errors.join("\n")).toMatch(/100%/);
  });

  it("reads percent escapes in links and anchors", () => {
    expect(checkDist(build({ "index.html": `<a href="/verify/#win%64ows">x</a><a href="/gu%69de/">y</a>` }))).toEqual([]);
  });

  it("catches a script, a stylesheet, an image, an iframe and a video from another origin", () => {
    const html = [
      `<script src="https://cdn.example.com/a.js"></script>`,
      `<script src="//cdn.example.com/b.js"></script>`,
      `<link rel="stylesheet" href="https://fonts.example.com/c.css">`,
      `<link rel="preload" as="font" href="https://fonts.example.com/d.woff2">`,
      `<img src="https://img.example.com/e.png" alt="">`,
      `<img srcset="/og.png 1x, https://img.example.com/f.png 2x" alt="">`,
      `<iframe src="https://www.youtube.com/embed/x"></iframe>`,
      `<video poster="https://img.example.com/g.jpg"><source src="https://media.example.com/h.mp4"></video>`,
    ].join("\n");
    const errors = checkDist(build({ "index.html": html }));
    expect(errors).toHaveLength(9);
    expect(errors.every((e) => /another origin/.test(e))).toBe(true);
  });

  it("allows plain links, canonical links and meta tags that name another origin", () => {
    const html = `<link rel="canonical" href="https://elsewhere.example/"><meta property="og:image" content="https://elsewhere.example/og.png"><a href="https://github.com/Roboroads/repeat-that">x</a>`;
    expect(checkDist(build({ "index.html": html }))).toEqual([]);
  });

  it("catches a font or image loaded from another origin by a stylesheet", () => {
    const css = `@font-face { font-family: x; src: url("https://fonts.example.com/x.woff2"); } @import url(//cdn.example.com/y.css);`;
    const errors = checkDist(build({ "_astro/site.css": css }));
    expect(errors).toHaveLength(2);
  });

  it("allows data: urls and same-origin urls in a stylesheet", () => {
    const css = `a { background: url("data:image/svg+xml,%3Csvg%3E%3C/svg%3E"); } b { background: url(/og.png); }`;
    expect(checkDist(build({ "_astro/site.css": css }))).toEqual([]);
  });

  it("catches an external url in an inline style", () => {
    const errors = checkDist(build({ "index.html": `<style>@import url("https://fonts.example.com/z.css");</style>` }));
    expect(errors).toHaveLength(1);
  });
});
