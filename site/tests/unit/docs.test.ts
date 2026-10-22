import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { DOCS, listDocs, routeParam } from "../../src/lib/docs";

const scratchRoot = join(process.cwd(), ".tmp");
let repo: string;

beforeEach(() => {
  mkdirSync(scratchRoot, { recursive: true });
  repo = mkdtempSync(join(scratchRoot, "repo-"));
  mkdirSync(join(repo, "docs"));
});

afterEach(() => {
  rmSync(repo, { recursive: true, force: true });
});

function write(source: string): void {
  writeFileSync(join(repo, source), `# ${source}\n`);
}

describe("the route map", () => {
  it("has the routes of the plan, each with its own source", () => {
    expect(DOCS.map((d) => [d.route, d.source])).toEqual([
      ["/guide/", "docs/guide.md"],
      ["/install/windows/", "docs/install-windows.md"],
      ["/install/macos/", "docs/install-macos.md"],
      ["/install/linux/", "docs/install-linux.md"],
      ["/linux/", "docs/linux.md"],
      ["/macos/", "docs/macos.md"],
      ["/verify/", "docs/verify-downloads.md"],
      ["/privacy/", "PRIVACY.md"],
      ["/security/", "SECURITY.md"],
      ["/code-signing/", "docs/code-signing.md"],
    ]);
  });

  it("uses every route and source once, and every route starts and ends with a slash", () => {
    expect(new Set(DOCS.map((d) => d.route)).size).toBe(DOCS.length);
    expect(new Set(DOCS.map((d) => d.source)).size).toBe(DOCS.length);
    for (const doc of DOCS) expect(doc.route).toMatch(/^\/.+\/$/);
  });
});

describe("routeParam", () => {
  it("turns a route into the rest parameter of [...doc].astro", () => {
    expect(routeParam("/guide/")).toBe("guide");
    expect(routeParam("/install/windows/")).toBe("install/windows");
  });
});

describe("listDocs", () => {
  const everyRequired = ["docs/install-windows.md", "docs/install-macos.md", "docs/linux.md", "docs/macos.md", "docs/verify-downloads.md", "PRIVACY.md", "SECURITY.md"];

  it("returns the docs that exist", () => {
    for (const source of everyRequired) write(source);
    write("docs/guide.md");
    const warnings: string[] = [];
    const docs = listDocs(repo, (message) => warnings.push(message));
    expect(docs.map((d) => d.route)).toContain("/guide/");
    expect(docs).toHaveLength(everyRequired.length + 1);
  });

  it("skips a missing optional doc with a warning that names the file and the route", () => {
    for (const source of everyRequired) write(source);
    const warnings: string[] = [];
    const docs = listDocs(repo, (message) => warnings.push(message));
    expect(docs.map((d) => d.route)).not.toContain("/guide/");
    expect(docs.map((d) => d.route)).not.toContain("/install/linux/");
    expect(docs.map((d) => d.route)).not.toContain("/code-signing/");
    expect(warnings).toHaveLength(3);
    expect(warnings.join("\n")).toMatch(/docs\/guide\.md.*\/guide\//);
  });

  it("throws when a doc that has to exist is missing", () => {
    for (const source of everyRequired.filter((s) => s !== "PRIVACY.md")) write(source);
    expect(() => listDocs(repo, () => {})).toThrow(/PRIVACY\.md/);
  });
});
