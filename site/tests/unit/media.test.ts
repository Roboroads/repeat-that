import { existsSync, mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { copyMedia } from "../../integrations/media.mjs";
import { findMedia, mediaDir } from "../../src/lib/media";

const scratch = join(process.cwd(), ".tmp");
let dir: string;

beforeEach(() => {
  mkdirSync(scratch, { recursive: true });
  dir = mkdtempSync(join(scratch, "media-"));
});
afterEach(() => rmSync(dir, { recursive: true, force: true }));

describe("findMedia", () => {
  it("is null without demo.mp4, even when a poster is there", () => {
    writeFileSync(join(dir, "demo-poster.jpg"), "x");
    expect(findMedia(dir)).toBeNull();
  });

  it("is null when the folder does not exist", () => {
    expect(findMedia(join(dir, "nope"))).toBeNull();
  });

  it("finds the video alone", () => {
    writeFileSync(join(dir, "demo.mp4"), "x");
    expect(findMedia(dir)).toEqual({ video: "/media/demo.mp4" });
  });

  it("finds the poster and the captions when they are there", () => {
    for (const name of ["demo.mp4", "demo-poster.jpg", "demo.vtt"]) writeFileSync(join(dir, name), "x");
    expect(findMedia(dir)).toEqual({
      video: "/media/demo.mp4",
      poster: "/media/demo-poster.jpg",
      captions: "/media/demo.vtt",
    });
  });
});

describe("mediaDir", () => {
  it("is public/media unless SITE_MEDIA_DIR says otherwise", () => {
    expect(mediaDir({}, "/site")).toBe("/site/public/media");
    expect(mediaDir({ SITE_MEDIA_DIR: ".e2e/media" }, "/site")).toBe("/site/.e2e/media");
  });
});

describe("copyMedia", () => {
  it("copies the known files into media/ of the build and nothing else", () => {
    const out = join(dir, "dist");
    const src = join(dir, "src");
    mkdirSync(src);
    for (const name of ["demo.mp4", "demo.vtt", "notes.txt"]) writeFileSync(join(src, name), "x");
    copyMedia(src, out);
    expect(existsSync(join(out, "media/demo.mp4"))).toBe(true);
    expect(existsSync(join(out, "media/demo.vtt"))).toBe(true);
    expect(existsSync(join(out, "media/notes.txt"))).toBe(false);
  });

  it("does nothing when the folder does not exist", () => {
    copyMedia(join(dir, "nope"), join(dir, "dist"));
    expect(existsSync(join(dir, "dist/media"))).toBe(false);
  });
});
