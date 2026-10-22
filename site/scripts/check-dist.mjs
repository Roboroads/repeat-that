// @ts-check
// Checks the built site (`npm run check:dist [folder]`). It fails when
// - CNAME, .nojekyll or .well-known/security.txt is missing,
// - an internal link, image, script or stylesheet points at nothing, or at an anchor no heading has,
// - a page loads a script, stylesheet, font, image, iframe or media from another origin. Plain <a> links to other sites
//   are fine, so are canonical links and meta tags.
import { existsSync, readdirSync, readFileSync, statSync } from "node:fs";
import { join, relative, resolve, sep } from "node:path";
import { pathToFileURL } from "node:url";

const REQUIRED = ["CNAME", ".nojekyll", ".well-known/security.txt"];
// `link` tags that make the browser fetch something. canonical, alternate and the like only name a page.
const LOADING_RELS = /(^|\s)(stylesheet|preload|prefetch|modulepreload|preconnect|dns-prefetch|prerender|icon|apple-touch-icon|mask-icon|manifest)(\s|$)/i;
const LOADING_ATTRIBUTES = {
  script: ["src"],
  iframe: ["src"],
  embed: ["src"],
  object: ["data"],
  img: ["src", "srcset"],
  source: ["src", "srcset"],
  video: ["src", "poster"],
  audio: ["src"],
  track: ["src"],
  input: ["src"],
};
const LINK_ATTRIBUTES = { a: ["href"], area: ["href"], link: ["href"], ...LOADING_ATTRIBUTES };

const ORIGIN = "http://dist.invalid";

/** @param {string} dir @returns {string[]} */
function listFiles(dir) {
  /** @type {string[]} */
  const files = [];
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    const path = join(dir, entry.name);
    if (entry.isDirectory()) files.push(...listFiles(path));
    else files.push(path);
  }
  return files;
}

/** @param {string} value */
const decode = (value) => value.replace(/&amp;/g, "&").replace(/&quot;/g, '"').replace(/&#39;|&#x27;/g, "'");

/** @param {string} value */
const isExternal = (value) => /^(https?:)?\/\//i.test(value.trim());

/** @param {string} css @returns {string[]} */
function cssUrls(css) {
  /** @type {Set<string>} */
  const urls = new Set();
  for (const match of css.matchAll(/url\(\s*(["']?)([^)"']*)\1\s*\)/gi)) urls.add(match[2].trim());
  for (const match of css.matchAll(/@import\s+(?:url\(\s*)?["']?([^"')\s;]+)/gi)) urls.add(match[1].trim());
  return [...urls];
}

/** @param {string} srcset */
const srcsetUrls = (srcset) => srcset.split(",").map((candidate) => candidate.trim().split(/\s+/)[0]).filter(Boolean);

/**
 * @param {string} html
 * @returns {{ tags: { name: string, attributes: Record<string, string> }[], styles: string[] }}
 */
function readHtml(html) {
  /** @type {string[]} */
  const styles = [];
  const stripped = html
    .replace(/<!--[\s\S]*?-->/g, "")
    .replace(/<style\b([^>]*)>([\s\S]*?)<\/style>/gi, (_, attributes, body) => {
      styles.push(body);
      return `<style${attributes}></style>`;
    })
    .replace(/<script\b([^>]*)>[\s\S]*?<\/script>/gi, (_, attributes) => `<script${attributes}></script>`);

  const tagPattern = /<([a-zA-Z][\w:-]*)((?:\s+[^\s"'>/=]+(?:\s*=\s*(?:"[^"]*"|'[^']*'|[^\s"'=<>`]+))?)*)\s*\/?>/g;
  const attributePattern = /([^\s"'>/=]+)(?:\s*=\s*(?:"([^"]*)"|'([^']*)'|([^\s"'=<>`]+)))?/g;
  const tags = [];
  for (const tag of stripped.matchAll(tagPattern)) {
    /** @type {Record<string, string>} */
    const attributes = {};
    for (const attribute of tag[2].matchAll(attributePattern)) {
      attributes[attribute[1].toLowerCase()] = decode(attribute[2] ?? attribute[3] ?? attribute[4] ?? "");
    }
    tags.push({ name: tag[1].toLowerCase(), attributes });
  }
  return { tags, styles };
}

/** The URL path a page is served at. @param {string} file relative to the build folder */
function pageUrl(file) {
  const path = file.split(sep).join("/");
  if (path === "index.html") return "/";
  return path.endsWith("/index.html") ? `/${path.slice(0, -"index.html".length)}` : `/${path}`;
}

/**
 * Looks for problems in a built site.
 * @param {string} distDir
 * @returns {string[]} one message per problem, empty when the build is fine
 */
export function checkDist(distDir) {
  const root = resolve(distDir);
  /** @type {string[]} */
  const errors = [];
  if (!existsSync(root) || !statSync(root).isDirectory()) return [`${distDir} does not exist, build the site first`];

  for (const required of REQUIRED) {
    if (!existsSync(join(root, required))) errors.push(`${required} is missing from the build`);
  }

  const files = listFiles(root).map((path) => relative(root, path));
  const pages = files.filter((file) => file.endsWith(".html"));
  /** @type {Map<string, Set<string>>} */
  const anchorCache = new Map();

  /** @param {string} file */
  const anchorsOf = (file) => {
    let anchors = anchorCache.get(file);
    if (!anchors) {
      anchors = new Set();
      // A link can land on any element with an id, and on an <a name>. Other names (meta, input) are not targets.
      for (const { name, attributes } of readHtml(readFileSync(join(root, file), "utf8")).tags) {
        if (attributes.id) anchors.add(attributes.id);
        if (name === "a" && attributes.name) anchors.add(attributes.name);
      }
      anchorCache.set(file, anchors);
    }
    return anchors;
  };

  /** The file a path in the build is served from, or undefined. @param {string} pathname */
  const fileFor = (pathname) => {
    const clean = decodeURIComponent(pathname).replace(/^\//, "");
    const candidates = clean === "" || clean.endsWith("/") ? [`${clean}index.html`] : [clean, `${clean}/index.html`];
    return candidates.find((candidate) => existsSync(join(root, candidate)) && statSync(join(root, candidate)).isFile());
  };

  for (const page of pages) {
    const base = pageUrl(page);
    const { tags, styles } = readHtml(readFileSync(join(root, page), "utf8"));

    for (const css of styles) {
      for (const url of cssUrls(css)) {
        if (isExternal(url)) errors.push(`${page}: a style loads ${url} from another origin`);
      }
    }

    for (const { name, attributes } of tags) {
      const loads =
        name === "link" ? (LOADING_RELS.test(attributes.rel ?? "") ? ["href"] : []) : (LOADING_ATTRIBUTES[/** @type {keyof typeof LOADING_ATTRIBUTES} */ (name)] ?? []);
      for (const attribute of loads) {
        const value = attributes[attribute];
        if (value === undefined) continue;
        for (const url of attribute === "srcset" ? srcsetUrls(value) : [value]) {
          if (isExternal(url)) errors.push(`${page}: <${name} ${attribute}> loads ${url} from another origin`);
        }
      }

      const linked = LINK_ATTRIBUTES[/** @type {keyof typeof LINK_ATTRIBUTES} */ (name)] ?? [];
      for (const attribute of linked) {
        const value = attributes[attribute];
        if (value === undefined) continue;
        for (const href of attribute === "srcset" ? srcsetUrls(value) : [value]) {
          if (!href || isExternal(href) || /^[a-z][a-z0-9+.-]*:/i.test(href)) continue;
          const url = new URL(href, ORIGIN + base);
          if (url.origin !== ORIGIN) continue;
          const target = fileFor(url.pathname);
          if (!target) {
            errors.push(`${page}: <${name} ${attribute}="${href}"> points at nothing (${url.pathname})`);
            continue;
          }
          const fragment = decodeURIComponent(url.hash.slice(1));
          if (fragment && fragment !== "top" && target.endsWith(".html") && !anchorsOf(target).has(fragment)) {
            errors.push(`${page}: <${name} ${attribute}="${href}"> points at #${fragment}, which ${target} does not have`);
          }
        }
      }
    }
  }

  for (const file of files.filter((f) => f.endsWith(".css"))) {
    for (const url of cssUrls(readFileSync(join(root, file), "utf8"))) {
      if (isExternal(url)) errors.push(`${file}: loads ${url} from another origin`);
    }
  }

  return errors;
}

function main() {
  const folder = process.argv[2] ?? process.env.SITE_OUT_DIR ?? "dist";
  const errors = checkDist(folder);
  if (errors.length > 0) {
    for (const error of errors) console.error(`::error::${error}`);
    console.error(`${errors.length} problem${errors.length === 1 ? "" : "s"} in ${folder}`);
    process.exit(1);
  }
  console.log(`${folder} is fine`);
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) main();
