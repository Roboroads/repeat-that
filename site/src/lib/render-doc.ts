import { unified } from "@astrojs/markdown-remark";
import remarkDocLinks from "./remark-doc-links";

export interface RenderedDoc {
  html: string;
  /** The text of the first top-level heading, or undefined when the doc has none. */
  title: string | undefined;
  /** The first paragraph as plain text, at most 160 characters, for the page description. Undefined without a paragraph. */
  summary: string | undefined;
}

const ENTITIES: Record<string, string> = { "&amp;": "&", "&lt;": "<", "&gt;": ">", "&quot;": '"', "&#39;": "'", "&#x26;": "&" };

function summarize(html: string): string | undefined {
  const paragraph = /<p>([\s\S]*?)<\/p>/.exec(html)?.[1];
  if (!paragraph) return undefined;
  const text = paragraph
    .replace(/<[^>]+>/g, "")
    .replace(/&(?:amp|lt|gt|quot|#39|#x26);/g, (entity) => ENTITIES[entity])
    .replace(/\s+/g, " ")
    .trim();
  if (!text) return undefined;
  return text.length <= 160 ? text : `${text.slice(0, 157).trimEnd()}...`;
}

/**
 * Renders one doc with the pipeline Astro uses for its own Markdown (GitHub flavoured, heading ids made the way GitHub
 * makes them, so anchors keep working), with relative links rewritten for the site. Code stays a plain `<pre>`.
 */
export async function renderDoc(markdown: string, from: string, routes: ReadonlyMap<string, string>): Promise<RenderedDoc> {
  const processor = unified({ remarkPlugins: [[remarkDocLinks, { from, routes }]], smartypants: false });
  const renderer = await processor.createRenderer({ syntaxHighlight: false });
  const { code, metadata } = await renderer.render(markdown);
  return { html: code, title: metadata.headings.find((h) => h.depth === 1)?.text, summary: summarize(code) };
}
