import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { DOCS, routesBySource } from "../../src/lib/docs";
import { loadRequirements, parseRequirements, summaryLine, type Requirements } from "../../src/lib/requirements";

const routes = routesBySource(DOCS);
const fixture = readFileSync(new URL("../fixtures/readme-requirements.md", import.meta.url), "utf8");

describe("parseRequirements", () => {
  let result: Requirements;
  const platform = (id: string) => result.platforms.find((p) => p.id === id)!;

  beforeEach(async () => {
    result = (await parseRequirements(fixture, routes))!;
  });

  it("returns one entry per platform heading, in README order", () => {
    expect(result.platforms.map((p) => [p.id, p.title])).toEqual([
      ["windows", "Windows"],
      ["macos", "macOS"],
      ["linux", "Linux"],
    ]);
  });

  it("reads the rows of each table, the header row left out", () => {
    expect(platform("windows").rows.map((r) => r.label)).toEqual(["Operating system", "Processor", "WebView2 runtime"]);
    expect(platform("linux").rows.map((r) => r.label)).toEqual(["Operating system", "Processor", "Audio", "Hotkeys"]);
    expect(platform("macos").rows[1].text).toBe("Apple Silicon (M1 or later). Intel Macs aren't supported");
  });

  it("keeps inline code in a row as code", () => {
    const hotkeys = platform("linux").rows.find((r) => r.label === "Hotkeys")!;
    expect(hotkeys.html).toContain("<code>repeat-that --replay</code>");
  });

  it("keeps the paragraphs after a table as notes of that platform", () => {
    expect(platform("macos").notesHtml).toContain("hasn't been tested on a real Mac yet");
    expect(platform("linux").notesHtml).toContain("Other distributions can use the AppImage");
    expect(platform("windows").notesHtml).toBe("");
  });

  it("moves the paragraph that points at the install guides under the section, with links for the site", () => {
    expect(platform("linux").notesHtml).not.toContain("steps to install");
    expect(result.footerHtml).toContain("The steps to install are in");
    expect(result.footerHtml).toContain('href="/install/windows/"');
    expect(result.footerHtml).toContain('href="/install/macos/"');
    expect(result.footerHtml).toContain('href="/install/linux/"');
    expect(result.footerHtml).toContain('href="/linux/"');
  });

  it("stops at the next section of the README", () => {
    const everything = JSON.stringify(result);
    expect(everything).not.toContain("must not end up");
  });

  it("ignores the logo and the badges above the section, so the site never loads them", async () => {
    const top = [
      '<div align="center">',
      '<img src="app/src-tauri/icons/svg/app.svg" width="112" alt="The logo">',
      "[![CI](https://img.shields.io/github/actions/workflow/status/Roboroads/repeat-that/ci.yml?branch=main)](https://example.org/ci)",
      "![Logo](app/src-tauri/icons/svg/app.svg)",
      "</div>",
    ].join("\n\n");
    const withBadges = await parseRequirements(`# Repeat That\n\n${top}\n\n${fixture.split("# Repeat That\n")[1]}`, routes);
    expect(withBadges).toEqual(result);
    const everything = JSON.stringify(withBadges);
    expect(everything).not.toContain("<img");
    expect(everything).not.toContain("shields.io");
  });

  it("returns null when the README has no System requirements section", async () => {
    expect(await parseRequirements("# Repeat That\n\n## Install (Windows)\n\nText\n", routes)).toBeNull();
  });
});

describe("summaryLine", () => {
  it("joins the operating system and processor rows into one sentence", async () => {
    const result = (await parseRequirements(fixture, routes))!;
    const macos = result.platforms.find((p) => p.id === "macos")!;
    expect(summaryLine(macos)).toBe("macOS 14.2 or newer. Apple Silicon (M1 or later). Intel Macs aren't supported.");
  });

  it("is empty when the table has neither row", () => {
    expect(summaryLine({ id: "x", title: "X", rows: [{ label: "Audio", html: "a", text: "a" }], notesHtml: "" })).toBe("");
  });
});

describe("loadRequirements", () => {
  const scratch = join(process.cwd(), ".tmp");
  let dir: string;

  beforeEach(() => {
    mkdirSync(scratch, { recursive: true });
    dir = mkdtempSync(join(scratch, "readme-"));
  });
  afterEach(() => rmSync(dir, { recursive: true, force: true }));

  it("reads README.md in the repository root", async () => {
    writeFileSync(join(dir, "README.md"), fixture);
    expect((await loadRequirements({}, dir, routes, () => {}))?.platforms).toHaveLength(3);
  });

  it("reads the file named by SITE_README, relative to the working directory", async () => {
    const result = await loadRequirements({ SITE_README: "tests/fixtures/readme-requirements.md" }, dir, routes, () => {});
    expect(result?.platforms).toHaveLength(3);
  });

  it("warns and returns null when the section is missing, so the site builds without it", async () => {
    writeFileSync(join(dir, "README.md"), "# Repeat That\n");
    const warnings: string[] = [];
    expect(await loadRequirements({}, dir, routes, (m) => warnings.push(m))).toBeNull();
    expect(warnings.join("\n")).toMatch(/System requirements/);
  });
});

// Guards the format the site relies on. It runs as soon as the README has the section.
const realReadme = resolve(process.cwd(), "../README.md");
const realHasSection = existsSync(realReadme) && /^## System requirements$/m.test(readFileSync(realReadme, "utf8"));

describe.skipIf(!realHasSection)("the README of this repository", () => {
  it("has a table with an operating system and a processor row for Windows, macOS and Linux", async () => {
    const result = (await parseRequirements(readFileSync(realReadme, "utf8"), routes))!;
    expect(result.platforms.map((p) => p.id)).toEqual(["windows", "macos", "linux"]);
    for (const p of result.platforms) {
      expect(p.rows.map((r) => r.label)).toEqual(expect.arrayContaining(["Operating system", "Processor"]));
      expect(summaryLine(p)).not.toBe("");
    }
  });

  it("puts nothing from other sites on the landing page, whatever badges the README has", async () => {
    const result = (await parseRequirements(readFileSync(realReadme, "utf8"), routes))!;
    const everything = JSON.stringify(result);
    expect(everything).not.toContain("<img");
    expect(everything).not.toContain("shields.io");
  });
});
