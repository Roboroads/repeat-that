// @ts-check
// A small static server for the browser tests: `node tests/e2e/serve.mjs <folder> <port>`.
// It serves the way GitHub Pages does (a folder gives its index.html, a folder without a trailing slash redirects, dot
// folders such as .well-known are served, no directory listings), and unlike `astro preview` it always stays in the
// foreground, which Playwright's webServer needs.
import { createReadStream, existsSync, statSync } from "node:fs";
import { createServer } from "node:http";
import { extname, join, resolve, sep } from "node:path";
import { pathToFileURL } from "node:url";

/** @type {Record<string, string>} */
const TYPES = {
  ".html": "text/html; charset=utf-8",
  ".css": "text/css; charset=utf-8",
  ".js": "text/javascript; charset=utf-8",
  ".json": "application/json",
  ".svg": "image/svg+xml",
  ".png": "image/png",
  ".jpg": "image/jpeg",
  ".mp4": "video/mp4",
  ".vtt": "text/vtt; charset=utf-8",
  ".txt": "text/plain; charset=utf-8",
  ".md": "text/markdown; charset=utf-8",
};

/**
 * A server for the files in `folder`. The caller calls `listen`.
 * @param {string} folder
 */
export function createStaticServer(folder) {
  const root = resolve(folder);

  return createServer((request, response) => {
    const url = new URL(request.url ?? "/", "http://localhost");
    /** @param {number} status @param {string} body @param {Record<string, string>} [headers] */
    const send = (status, body, headers = {}) => {
      response.writeHead(status, { "content-type": "text/plain", ...headers });
      response.end(body);
    };

    let pathname;
    try {
      pathname = decodeURIComponent(url.pathname);
    } catch {
      return send(400, "Bad request");
    }
    const path = resolve(join(root, pathname));
    if (path !== root && !path.startsWith(root + sep)) return send(403, "Forbidden");
    if (!existsSync(path)) return send(404, "Not found");

    if (statSync(path).isDirectory()) {
      if (!url.pathname.endsWith("/")) return send(301, "", { location: `${url.pathname}/${url.search}` });
      const index = join(path, "index.html");
      if (!existsSync(index)) return send(404, "Not found");
      response.writeHead(200, { "content-type": TYPES[".html"] });
      return createReadStream(index).pipe(response);
    }

    response.writeHead(200, { "content-type": TYPES[extname(path)] ?? "application/octet-stream" });
    createReadStream(path).pipe(response);
  });
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  const [folder, port] = process.argv.slice(2);
  if (!folder || !port) {
    console.error("usage: node tests/e2e/serve.mjs <folder> <port>");
    process.exit(2);
  }
  createStaticServer(folder).listen(Number(port), "127.0.0.1", () => console.log(`serving ${resolve(folder)} on http://127.0.0.1:${port}/`));
}
