import type { Link, Root } from "mdast";
import remarkParse from "remark-parse";
import { unified } from "unified";
import { visit } from "unist-util-visit";
import { describe, expect, it } from "vitest";
import { DOCS, REPO_BLOB_URL, routesBySource, rewriteHref } from "../../src/lib/docs";
import remarkDocLinks from "../../src/lib/remark-doc-links";

const routes = routesBySource(DOCS);
const BLOB = REPO_BLOB_URL;

describe("rewriteHref from a file in docs/", () => {
  const rewrite = (href: string) => rewriteHref(href, "docs/linux.md", routes);

  it("sends a sibling doc in the route map to its route", () => {
    expect(rewrite("verify-downloads.md")).toBe("/verify/");
    expect(rewrite("./install-windows.md")).toBe("/install/windows/");
  });

  it("sends a doc one folder up in the route map to its route", () => {
    expect(rewrite("../PRIVACY.md")).toBe("/privacy/");
    expect(rewrite("../SECURITY.md")).toBe("/security/");
  });

  it("keeps the anchor on a route", () => {
    expect(rewrite("macos.md#build-from-source")).toBe("/macos/#build-from-source");
  });

  it("sends a doc that is not in the route map to GitHub, anchor kept", () => {
    expect(rewrite("../README.md#using-it")).toBe(`${BLOB}README.md#using-it`);
    expect(rewrite("ARCHITECTURE.md")).toBe(`${BLOB}docs/ARCHITECTURE.md`);
    expect(rewrite("ARCHITECTURE.md#macos")).toBe(`${BLOB}docs/ARCHITECTURE.md#macos`);
    expect(rewrite("../CONTRIBUTING.md#running-the-end-to-end-test")).toBe(
      `${BLOB}CONTRIBUTING.md#running-the-end-to-end-test`,
    );
  });

  it("sends other files in the repository to GitHub too", () => {
    expect(rewrite("../LICENSE")).toBe(`${BLOB}LICENSE`);
    expect(rewrite("../app/src-tauri/tauri.conf.json")).toBe(`${BLOB}app/src-tauri/tauri.conf.json`);
  });

  it("sends a folder to its tree page on GitHub", () => {
    expect(rewrite("../crates/")).toBe("https://github.com/Roboroads/repeat-that/tree/main/crates/");
  });

  it("leaves external links, mail links, protocol-relative links and plain anchors alone", () => {
    expect(rewrite("https://signpath.org")).toBe("https://signpath.org");
    expect(rewrite("https://github.com/Roboroads/repeat-that/issues/new?template=macos.yml")).toBe(
      "https://github.com/Roboroads/repeat-that/issues/new?template=macos.yml",
    );
    expect(rewrite("mailto:someone@example.org")).toBe("mailto:someone@example.org");
    expect(rewrite("//example.org/x")).toBe("//example.org/x");
    expect(rewrite("#audio-capture-permission")).toBe("#audio-capture-permission");
  });

  it("reads a link that starts with a slash from the repository root, as GitHub does", () => {
    expect(rewrite("/docs/linux.md")).toBe("/linux/");
    expect(rewrite("/docs/macos.md#build-from-source")).toBe("/macos/#build-from-source");
    expect(rewrite("/PRIVACY.md")).toBe("/privacy/");
    expect(rewrite("/LICENSE")).toBe(`${BLOB}LICENSE`);
    expect(rewrite("/crates/")).toBe("https://github.com/Roboroads/repeat-that/tree/main/crates/");
    expect(rewrite("/")).toBe("https://github.com/Roboroads/repeat-that/tree/main/");
    expect(rewrite("../")).toBe("https://github.com/Roboroads/repeat-that/tree/main/");
  });

  it("leaves a link that climbs out of the repository alone", () => {
    expect(rewrite("../../elsewhere.md")).toBe("../../elsewhere.md");
  });
});

describe("rewriteHref from the README", () => {
  it("resolves paths from the repository root", () => {
    expect(rewriteHref("docs/install-linux.md", "README.md", routes)).toBe("/install/linux/");
    expect(rewriteHref("PRIVACY.md", "README.md", routes)).toBe("/privacy/");
    expect(rewriteHref("ROADMAP.md", "README.md", routes)).toBe(`${BLOB}ROADMAP.md`);
  });
});

describe("rewriteHref when a doc is not on the site", () => {
  it("sends the link to GitHub instead of to a page that does not exist", () => {
    const without = routesBySource(DOCS.filter((d) => d.source !== "docs/install-linux.md"));
    expect(rewriteHref("install-linux.md", "docs/linux.md", without)).toBe(`${BLOB}docs/install-linux.md`);
  });
});

async function linksIn(markdown: string, from: string): Promise<string[]> {
  const tree = unified().use(remarkParse).parse(markdown) as Root;
  await unified().use(remarkDocLinks, { from, routes }).run(tree);
  const urls: string[] = [];
  visit(tree, (node) => {
    if (node.type === "link" || node.type === "definition") urls.push((node as Link).url);
  });
  return urls;
}

describe("the remark plugin", () => {
  it("rewrites inline links", async () => {
    const urls = await linksIn(
      "[a](verify-downloads.md) [b](../PRIVACY.md) [c](../README.md#using-it) [d](ARCHITECTURE.md) [e](https://example.org/x.md)",
      "docs/linux.md",
    );
    expect(urls).toEqual([
      "/verify/",
      "/privacy/",
      `${BLOB}README.md#using-it`,
      `${BLOB}docs/ARCHITECTURE.md`,
      "https://example.org/x.md",
    ]);
  });

  it("rewrites reference-style links", async () => {
    const urls = await linksIn("[a][doc]\n\n[doc]: install-macos.md#permission\n", "docs/macos.md");
    expect(urls).toEqual(["/install/macos/#permission"]);
  });

  it("stops the build on an image that is a file in the repository, because the site serves none", async () => {
    await expect(linksIn("![The tray](images/tray.png)", "docs/linux.md")).rejects.toThrow(/docs\/linux\.md.*images\/tray\.png/);
    await expect(linksIn("![The tray](/docs/images/tray.png)", "docs/linux.md")).rejects.toThrow(/tray\.png/);
    await expect(linksIn("![The tray][pic]\n\n[pic]: ../app/tray.png\n", "docs/linux.md")).rejects.toThrow(/tray\.png/);
  });

  it("leaves images with a full url or a data url to check-dist", async () => {
    const tree = unified().use(remarkParse).parse("![a](https://example.org/a.png) ![b](data:image/png;base64,AAAA)") as Root;
    await unified().use(remarkDocLinks, { from: "docs/linux.md", routes }).run(tree);
    const urls: string[] = [];
    visit(tree, "image", (node) => {
      urls.push(node.url);
    });
    expect(urls).toEqual(["https://example.org/a.png", "data:image/png;base64,AAAA"]);
  });

  it("does not touch links inside code", async () => {
    const urls = await linksIn("`[a](verify-downloads.md)`\n\n```\n[a](verify-downloads.md)\n```\n", "docs/linux.md");
    expect(urls).toEqual([]);
  });
});
