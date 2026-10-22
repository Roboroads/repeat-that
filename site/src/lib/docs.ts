import { existsSync } from "node:fs";
import { posix, resolve } from "node:path";

export const REPO_URL = "https://github.com/Roboroads/repeat-that";
export const REPO_BLOB_URL = `${REPO_URL}/blob/main/`;
const REPO_TREE_URL = `${REPO_URL}/tree/main/`;

export interface DocEntry {
  /** The page, with a slash on both ends. */
  route: string;
  /** The Markdown file, relative to the repository root with forward slashes. */
  source: string;
  /** An optional doc may not exist yet. It is skipped with a build warning, and links to it go to GitHub. */
  optional: boolean;
}

/** The one list of docs the site renders. Everything else in the repository stays on GitHub. */
export const DOCS: readonly DocEntry[] = [
  { route: "/guide/", source: "docs/guide.md", optional: true },
  { route: "/install/windows/", source: "docs/install-windows.md", optional: false },
  { route: "/install/macos/", source: "docs/install-macos.md", optional: false },
  { route: "/install/linux/", source: "docs/install-linux.md", optional: true },
  { route: "/linux/", source: "docs/linux.md", optional: false },
  { route: "/macos/", source: "docs/macos.md", optional: false },
  { route: "/verify/", source: "docs/verify-downloads.md", optional: false },
  { route: "/privacy/", source: "PRIVACY.md", optional: false },
  { route: "/security/", source: "SECURITY.md", optional: false },
  { route: "/code-signing/", source: "docs/code-signing.md", optional: true },
];

/** `/install/windows/` becomes `install/windows`, the value of the `doc` parameter in `pages/[...doc].astro`. */
export function routeParam(route: string): string {
  return route.replace(/^\/|\/$/g, "");
}

export function routesBySource(docs: readonly DocEntry[]): Map<string, string> {
  return new Map(docs.map((d) => [d.source, d.route]));
}

/**
 * The docs whose source file exists. A missing optional doc is skipped and reported through `warn`, a missing required
 * one is a mistake in the repository and throws.
 */
export function listDocs(repoRoot: string, warn: (message: string) => void): DocEntry[] {
  const found: DocEntry[] = [];
  for (const doc of DOCS) {
    if (existsSync(resolve(repoRoot, doc.source))) {
      found.push(doc);
    } else if (doc.optional) {
      warn(`${doc.source} does not exist, so ${doc.route} is not built and links to it go to GitHub`);
    } else {
      throw new Error(`${doc.source} is in the route map for ${doc.route} but the file does not exist`);
    }
  }
  return found;
}

/**
 * The root of the repository: the parent of the site folder, which is where the build and the tests run. Astro bundles
 * the pages, so a path relative to this file wouldn't point at the source any more.
 */
export function repoRoot(): string {
  return resolve(process.cwd(), "..");
}

let cached: DocEntry[] | undefined;

/** `listDocs` for the build. The warnings print once. */
export function getDocs(): DocEntry[] {
  cached ??= listDocs(repoRoot(), (message) => console.warn(`[site] warning: ${message}`));
  return cached;
}

export function hasRoute(route: string): boolean {
  return getDocs().some((d) => d.route === route);
}

/** True for a link or image that names a file of the repository: not a full url, a protocol-relative url or an anchor. */
export function isRepoPath(href: string): boolean {
  return href !== "" && !href.startsWith("#") && !href.startsWith("//") && !/^[a-z][a-z0-9+.-]*:/i.test(href);
}

/**
 * Turns a link from a Markdown file into one that works on the site. `from` is the file the link is in, relative to
 * the repository root. A relative link to a doc on the site becomes its route, anchor kept. Any other relative link
 * (and a link that starts with a slash, which GitHub reads from the repository root) becomes the matching page on
 * GitHub, so a link never ends on a page this site doesn't have.
 */
export function rewriteHref(href: string, from: string, routes: ReadonlyMap<string, string>): string {
  if (!isRepoPath(href)) return href;

  // GitHub reads a link that starts with a slash from the root of the repository, so the site does too.
  const rooted = href.startsWith("/");
  const [path, ...rest] = href.split("#");
  const hash = rest.length > 0 ? `#${rest.join("#")}` : "";
  const file = path.split("?")[0].replace(/^\/+/, "");
  const target = posix.normalize(posix.join(rooted ? "" : posix.dirname(from), file));
  if (target === ".." || target.startsWith("../")) return href;

  const route = routes.get(target);
  if (route) return route + hash;
  if (target === "." || target === "./") return REPO_TREE_URL + hash;
  return (target.endsWith("/") ? REPO_TREE_URL : REPO_BLOB_URL) + target + hash;
}
