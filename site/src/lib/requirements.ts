import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { toString } from "mdast-util-to-string";
import type { Heading, Paragraph, PhrasingContent, Root, RootContent, Table, TableCell } from "mdast";
import rehypeStringify from "rehype-stringify";
import remarkGfm from "remark-gfm";
import remarkParse from "remark-parse";
import remarkRehype from "remark-rehype";
import { unified } from "unified";
import { visit } from "unist-util-visit";
import remarkDocLinks from "./remark-doc-links";

export interface RequirementRow {
  label: string;
  /** The cell as HTML, inline code kept. */
  html: string;
  /** The cell as plain text. */
  text: string;
}

export interface PlatformRequirements {
  /** `windows`, `macos` or `linux`, from the heading. */
  id: string;
  title: string;
  rows: RequirementRow[];
  /** The paragraphs under the table, as HTML. Empty when there are none. */
  notesHtml: string;
}

export interface Requirements {
  platforms: PlatformRequirements[];
  /** The paragraph that points at the install guides, which belongs to the whole section. Empty when there is none. */
  footerHtml: string;
}

const SECTION = "system requirements";
// `docs/install-linux.md` before the links are rewritten, `/install/linux/` after.
const POINTS_AT_INSTALL = /install-[\w-]+\.md|\/install\/[\w-]+\//;

async function toHtml(children: RootContent[]): Promise<string> {
  const processor = unified().use(remarkRehype).use(rehypeStringify);
  const root: Root = { type: "root", children };
  const html = processor.stringify(await processor.run(root));
  return String(html).trim();
}

async function inlineHtml(cell: TableCell): Promise<string> {
  const paragraph: Paragraph = { type: "paragraph", children: cell.children as PhrasingContent[] };
  return (await toHtml([paragraph])).replace(/^<p>/, "").replace(/<\/p>$/, "");
}

function linksToInstall(node: RootContent): boolean {
  let found = false;
  visit(node, "link", (link) => {
    if (POINTS_AT_INSTALL.test(link.url)) found = true;
  });
  return found;
}

/**
 * Reads the "System requirements" section of the README: a `###` heading per platform, a two-column table under it
 * and optional paragraphs after the table. Returns null when the README has no such section.
 */
export async function parseRequirements(readme: string, routes: ReadonlyMap<string, string>): Promise<Requirements | null> {
  const tree = unified().use(remarkParse).use(remarkGfm).parse(readme) as Root;
  await unified().use(remarkDocLinks, { from: "README.md", routes }).run(tree);

  const nodes = tree.children;
  const start = nodes.findIndex((n) => n.type === "heading" && n.depth === 2 && toString(n).trim().toLowerCase() === SECTION);
  if (start === -1) return null;
  let end = nodes.findIndex((n, i) => i > start && n.type === "heading" && n.depth <= 2);
  if (end === -1) end = nodes.length;
  const section = nodes.slice(start + 1, end);

  const groups: { heading: Heading; body: RootContent[] }[] = [];
  for (const node of section) {
    if (node.type === "heading" && node.depth === 3) groups.push({ heading: node, body: [] });
    else groups.at(-1)?.body.push(node);
  }

  const footer: RootContent[] = [];
  const platforms: PlatformRequirements[] = [];
  for (const { heading, body } of groups) {
    const table = body.find((n): n is Table => n.type === "table");
    const rows: RequirementRow[] = [];
    for (const row of table?.children.slice(1) ?? []) {
      const [label, value] = row.children;
      if (label && value) rows.push({ label: toString(label), html: await inlineHtml(value), text: toString(value) });
    }
    const notes: RootContent[] = [];
    for (const node of body) {
      if (node.type === "table") continue;
      (linksToInstall(node) ? footer : notes).push(node);
    }
    const title = toString(heading).trim();
    platforms.push({ id: title.toLowerCase(), title, rows, notesHtml: notes.length ? await toHtml(notes) : "" });
  }

  return { platforms, footerHtml: footer.length ? await toHtml(footer) : "" };
}

/** "macOS 14.2 or newer. Apple Silicon (M1 or later). Intel Macs aren't supported." Empty without both rows. */
export function summaryLine(platform: PlatformRequirements): string {
  const text = (label: string) => platform.rows.find((r) => r.label === label)?.text.trim().replace(/\.$/, "");
  const parts = [text("Operating system"), text("Processor")].filter((p): p is string => Boolean(p));
  return parts.length ? `${parts.join(". ")}.` : "";
}

/**
 * Reads the requirements for the landing page from the README (`SITE_README` names another file, relative to the
 * working directory, for tests). There is no second copy of this text anywhere else. A README without the section
 * gives a warning and a landing page without the requirements.
 */
export async function loadRequirements(
  env: Record<string, string | undefined>,
  repoRoot: string,
  routes: ReadonlyMap<string, string>,
  warn: (message: string) => void,
): Promise<Requirements | null> {
  const path = env.SITE_README ? resolve(process.cwd(), env.SITE_README) : resolve(repoRoot, "README.md");
  const result = await parseRequirements(readFileSync(path, "utf8"), routes);
  if (!result) warn(`${path} has no "System requirements" section, so the landing page shows no requirements`);
  return result;
}
