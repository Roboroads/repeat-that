import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { pathToFileURL } from "node:url";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import wellKnown, { securityTxt } from "../../integrations/well-known.mjs";

describe("securityTxt", () => {
  const now = new Date("2026-10-22T10:00:00Z");

  it("sets Expires 180 days after the build, as an RFC 3339 date in UTC", () => {
    expect(securityTxt(now)).toContain("Expires: 2027-04-20T10:00:00Z\n");
  });

  it("has the contact, policy, language and canonical fields", () => {
    const text = securityTxt(now);
    expect(text).toContain("Contact: https://github.com/Roboroads/repeat-that/security/advisories/new\n");
    expect(text).toContain("Policy: https://github.com/Roboroads/repeat-that/security/policy\n");
    expect(text).toContain("Preferred-Languages: en\n");
    expect(text).toContain("Canonical: https://repeat-that.roboroads.me/.well-known/security.txt\n");
  });

  it("is one field per line and ends with a newline", () => {
    const lines = securityTxt(now).trimEnd().split("\n");
    expect(lines).toHaveLength(5);
    for (const line of lines) expect(line).toMatch(/^[A-Za-z-]+: \S/);
    expect(securityTxt(now).endsWith("\n")).toBe(true);
  });
});

describe("the astro:build:done hook", () => {
  const scratch = join(process.cwd(), ".tmp");
  let dist: string;

  beforeEach(() => {
    mkdirSync(scratch, { recursive: true });
    dist = mkdtempSync(join(scratch, "wk-"));
  });
  afterEach(() => rmSync(dist, { recursive: true, force: true }));

  const run = async () => {
    const hook = wellKnown().hooks["astro:build:done"] as (options: unknown) => Promise<void>;
    await hook({ dir: pathToFileURL(`${dist}/`), logger: { info() {}, warn() {} } });
  };

  it("writes .well-known/security.txt into the build, with an Expires in the future", async () => {
    await run();
    const text = readFileSync(join(dist, ".well-known/security.txt"), "utf8");
    const expires = /^Expires: (.+)$/m.exec(text)?.[1];
    expect(new Date(expires!).getTime()).toBeGreaterThan(Date.now() + 170 * 24 * 3600 * 1000);
  });

  it("drops the .gitkeep that keeps the folder in git", async () => {
    mkdirSync(join(dist, ".well-known"));
    writeFileSync(join(dist, ".well-known/.gitkeep"), "");
    await run();
    expect(existsSync(join(dist, ".well-known/.gitkeep"))).toBe(false);
  });

  it("leaves the files already in .well-known alone", async () => {
    mkdirSync(join(dist, ".well-known"));
    writeFileSync(join(dist, ".well-known/token.txt"), "abc");
    await run();
    expect(readFileSync(join(dist, ".well-known/token.txt"), "utf8")).toBe("abc");
  });
});
