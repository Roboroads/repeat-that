import AxeBuilder from "@axe-core/playwright";
import { expect, type APIRequestContext, type Page } from "@playwright/test";

export const UA = {
  windows:
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0.0.0 Safari/537.36",
  macos:
    "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.0 Safari/605.1.15",
  linux: "Mozilla/5.0 (X11; Linux x86_64; rv:131.0) Gecko/20100101 Firefox/131.0",
  android:
    "Mozilla/5.0 (Linux; Android 14; Pixel 8) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0.0.0 Mobile Safari/537.36",
} as const;

/** The pages found by following every link on the site that stays on it, starting at the home page. */
export interface Crawl {
  /** Paths of the pages that were opened in the browser, like `/guide/`. */
  pages: string[];
  /** Everything else the pages link to on the same site (files, for example), already fetched. */
  files: string[];
}

/**
 * Opens every page of the site that can be reached by links and calls `onPage` for each. Fails when a page or a linked
 * file doesn't answer 200, and when a request leaves the site.
 */
export async function crawl(
  page: Page,
  request: APIRequestContext,
  baseURL: string,
  onPage?: (path: string) => Promise<void>,
): Promise<Crawl> {
  const origin = new URL(baseURL).origin;
  const foreign: string[] = [];
  const failed: string[] = [];
  page.on("request", (r) => {
    const url = r.url();
    if (!url.startsWith(origin) && !url.startsWith("data:") && !url.startsWith("blob:")) foreign.push(url);
  });
  page.on("response", (r) => {
    if (r.url().startsWith(origin) && r.status() >= 400) failed.push(`${r.status()} ${r.url()}`);
  });

  const queue = ["/"];
  const seen = new Set<string>(queue);
  const pages: string[] = [];
  const files: string[] = [];

  while (queue.length > 0) {
    const path = queue.shift()!;
    const isPage = !/\.[a-z0-9]+$/i.test(path) || path.endsWith(".html");
    if (!isPage) {
      const response = await request.get(path);
      expect(response.status(), `GET ${path}`).toBe(200);
      files.push(path);
      continue;
    }
    const response = await page.goto(path);
    expect(response?.status(), `GET ${path}`).toBe(200);
    pages.push(path);
    await onPage?.(path);

    const hrefs = await page.$$eval("a[href]", (links) => links.map((a) => (a as HTMLAnchorElement).href));
    for (const href of hrefs) {
      const url = new URL(href);
      if (url.origin !== origin) continue;
      const next = url.pathname;
      if (!seen.has(next)) {
        seen.add(next);
        queue.push(next);
      }
    }
  }

  expect(foreign, "requests to other origins").toEqual([]);
  expect(failed, "responses with an error status").toEqual([]);
  return { pages, files };
}

/** Axe finds nothing worse than a moderate issue. */
export async function expectNoSeriousA11yIssues(page: Page): Promise<void> {
  const { violations } = await new AxeBuilder({ page }).analyze();
  const serious = violations
    .filter((v) => v.impact === "serious" || v.impact === "critical")
    .map((v) => `${v.id} (${v.impact}): ${v.nodes.map((n) => n.target.join(" ")).join(", ")}`);
  expect(serious).toEqual([]);
}

/** A page of the docs that exists in every build, for checks that need one. */
export async function aDocPage(request: APIRequestContext): Promise<string> {
  // The guide when step B has landed, the Windows install page before that.
  return (await request.get("/guide/")).ok() ? "/guide/" : "/install/windows/";
}
