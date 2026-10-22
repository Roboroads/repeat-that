import type { Root } from "mdast";
import type { Plugin } from "unified";
import { visit } from "unist-util-visit";
import { rewriteHref } from "./docs";

export interface DocLinksOptions {
  /** The file the Markdown comes from, relative to the repository root. */
  from: string;
  /** Doc source file to route, see `routesBySource`. */
  routes: ReadonlyMap<string, string>;
}

/** Rewrites the relative links of a doc, see `rewriteHref`. Inline and reference-style links, nothing else. */
const remarkDocLinks: Plugin<[DocLinksOptions], Root> = ({ from, routes }) => {
  return (tree) => {
    visit(tree, (node) => {
      if (node.type === "link" || node.type === "definition") {
        node.url = rewriteHref(node.url, from, routes);
      }
    });
  };
};

export default remarkDocLinks;
