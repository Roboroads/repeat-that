import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import type { Server } from "node:http";
import type { AddressInfo } from "node:net";
import { join } from "node:path";
import { afterAll, beforeAll, describe, expect, it } from "vitest";
import { createStaticServer } from "../e2e/serve.mjs";

// The static server of the browser tests has to behave like GitHub Pages, or the tests prove little.
const scratch = join(process.cwd(), ".tmp");
let dir: string;
let server: Server;
let base: string;

beforeAll(async () => {
  mkdirSync(scratch, { recursive: true });
  dir = mkdtempSync(join(scratch, "serve-"));
  mkdirSync(join(dir, "verify"));
  mkdirSync(join(dir, ".well-known"));
  mkdirSync(join(dir, "empty"));
  writeFileSync(join(dir, "index.html"), "<p>home</p>");
  writeFileSync(join(dir, "verify/index.html"), "<p>verify</p>");
  writeFileSync(join(dir, ".well-known/security.txt"), "Contact: x\n");
  server = createStaticServer(dir);
  await new Promise<void>((resolve) => server.listen(0, "127.0.0.1", resolve));
  base = `http://127.0.0.1:${(server.address() as AddressInfo).port}`;
});

afterAll(async () => {
  await new Promise((resolve) => server.close(resolve));
  rmSync(dir, { recursive: true, force: true });
});

describe("createStaticServer", () => {
  it("serves index.html for a folder, and redirects a folder without a slash", async () => {
    expect(await (await fetch(`${base}/verify/`)).text()).toBe("<p>verify</p>");
    const redirect = await fetch(`${base}/verify`, { redirect: "manual" });
    expect(redirect.status).toBe(301);
    expect(redirect.headers.get("location")).toBe("/verify/");
  });

  it("serves files in dot folders", async () => {
    const response = await fetch(`${base}/.well-known/security.txt`);
    expect(response.status).toBe(200);
    expect(response.headers.get("content-type")).toMatch(/^text\/plain/);
  });

  it("answers 404 for a missing file and a folder without an index", async () => {
    expect((await fetch(`${base}/nope.html`)).status).toBe(404);
    expect((await fetch(`${base}/empty/`)).status).toBe(404);
  });

  it("answers 400, and stays up, for a stray percent sign", async () => {
    expect((await fetch(`${base}/100%`)).status).toBe(400);
    expect((await fetch(`${base}/%E0%A4%A`)).status).toBe(400);
    expect(await (await fetch(`${base}/`)).text()).toBe("<p>home</p>");
  });

  it("doesn't leave the folder", async () => {
    expect((await fetch(`${base}/..%2f..%2f..%2fetc/passwd`)).status).toBe(403);
  });
});
