import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import {
  formatDate,
  formatSize,
  kindLabel,
  loadRelease,
  parseRelease,
  primaryFile,
  type ReleaseData,
} from "../../src/lib/release";

const fixture = (name: string): unknown => JSON.parse(readFileSync(new URL(`../fixtures/${name}`, import.meta.url), "utf8"));
const sample = (): any => structuredClone(fixture("release-windows.json"));

describe("parseRelease", () => {
  it("accepts the Windows-only release", () => {
    const data = parseRelease(fixture("release-windows.json"));
    expect(data.release?.tag).toBe("v0.1.0");
    expect(data.release?.platforms.map((p) => p.name)).toEqual(["windows"]);
  });

  it("keeps the platforms in the order the data has them", () => {
    const data = parseRelease(fixture("release-all.json"));
    expect(data.release?.platforms.map((p) => p.name)).toEqual(["windows", "linux", "flatpak", "macos"]);
    expect(data.release?.platforms[1].files.map((f) => f.kind)).toEqual(["appimage", "deb", "rpm"]);
  });

  it("accepts a release of null", () => {
    expect(parseRelease(fixture("release-none.json"))).toEqual({ schema: 1, release: null });
  });

  it("rejects schema 2", () => {
    const raw = sample();
    raw.schema = 2;
    expect(() => parseRelease(raw)).toThrow(/schema/);
  });

  it("rejects a file without sha256", () => {
    const raw = sample();
    delete raw.release.platforms[0].files[0].sha256;
    expect(() => parseRelease(raw)).toThrow(/sha256/);
  });

  it("rejects a sha256 that is not 64 hex digits", () => {
    const raw = sample();
    raw.release.platforms[0].files[0].sha256 = "fdc2...29d";
    expect(() => parseRelease(raw)).toThrow(/sha256/);
  });

  it("rejects an unknown kind", () => {
    const raw = sample();
    raw.release.platforms[0].files[0].kind = "msi";
    expect(() => parseRelease(raw)).toThrow(/kind/);
  });

  it("rejects a missing release key, a bad size and a url that is not https", () => {
    expect(() => parseRelease({ schema: 1 })).toThrow(/release/);
    const size = sample();
    size.release.platforms[0].files[0].size = -1;
    expect(() => parseRelease(size)).toThrow(/size/);
    const url = sample();
    url.release.platforms[0].files[0].url = "http://example.com/a.exe";
    expect(() => parseRelease(url)).toThrow(/url/);
  });

  it("rejects something that is not an object", () => {
    expect(() => parseRelease(null)).toThrow();
    expect(() => parseRelease([])).toThrow();
  });

  it("names where the problem is", () => {
    const raw = sample();
    raw.release.platforms[0].files[0].kind = "msi";
    expect(() => parseRelease(raw)).toThrow(/release\.platforms\[0\]\.files\[0\]\.kind/);
  });
});

describe("primaryFile", () => {
  const data = parseRelease(fixture("release-all.json")) as ReleaseData;
  const byName = (name: string) => data.release!.platforms.find((p) => p.name === name)!;

  it("is the installer on Windows and the disk image on macOS", () => {
    expect(primaryFile(byName("windows")).kind).toBe("setup-exe");
    expect(primaryFile(byName("macos")).kind).toBe("dmg");
  });

  it("is the AppImage on Linux, which runs on every distribution", () => {
    expect(primaryFile(byName("linux")).kind).toBe("appimage");
  });

  it("is the Flatpak bundle for Flatpak", () => {
    expect(primaryFile(byName("flatpak")).kind).toBe("flatpak");
  });

  it("falls back to the first file when the preferred kind is missing", () => {
    const linux = structuredClone(byName("linux"));
    linux.files = linux.files.filter((f) => f.kind !== "appimage");
    expect(primaryFile(linux).kind).toBe("deb");
  });
});

describe("formatSize", () => {
  it("shows megabytes with one decimal", () => {
    expect(formatSize(9500925)).toBe("9.5 MB");
    expect(formatSize(87114752)).toBe("87.1 MB");
  });

  it("shows kilobytes below one megabyte and bytes below one kilobyte", () => {
    expect(formatSize(98_000)).toBe("98 kB");
    expect(formatSize(512)).toBe("512 B");
  });
});

describe("formatDate", () => {
  it("writes the day, the month name and the year, in UTC", () => {
    expect(formatDate("2026-10-21T13:50:34Z")).toBe("21 October 2026");
    expect(formatDate("2027-01-05T23:59:59Z")).toBe("5 January 2027");
  });

  it("gives the text back when it is not a date", () => {
    expect(formatDate("soon")).toBe("soon");
  });
});

describe("kindLabel", () => {
  it("describes every kind", () => {
    expect(kindLabel("setup-exe")).toMatch(/installer/i);
    expect(kindLabel("appimage")).toMatch(/AppImage/);
    expect(kindLabel("deb")).toMatch(/Debian/);
    expect(kindLabel("rpm")).toMatch(/Fedora/);
    expect(kindLabel("dmg")).toMatch(/disk image/i);
    expect(kindLabel("flatpak")).toMatch(/Flatpak/);
  });
});

describe("loadRelease", () => {
  it("reads the file named by SITE_RELEASE_JSON, relative to the working directory", () => {
    const data = loadRelease({ SITE_RELEASE_JSON: "tests/fixtures/release-all.json" }, process.cwd());
    expect(data.release?.version).toBe("0.2.0");
  });

  it("falls back to the committed sample", () => {
    const data = loadRelease({}, process.cwd());
    expect(data.release?.tag).toBe("v0.1.0");
  });

  it("fails the build when the file is missing or wrong", () => {
    expect(() => loadRelease({ SITE_RELEASE_JSON: "tests/fixtures/nope.json" }, process.cwd())).toThrow(/SITE_RELEASE_JSON/);
    expect(() => loadRelease({ SITE_RELEASE_JSON: "package.json" }, process.cwd())).toThrow(/schema/);
  });
});
