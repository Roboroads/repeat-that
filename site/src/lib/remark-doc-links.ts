import type { Root } from "mdast";
import type { Plugin } from "unified";
import { visit } from "unist-util-visit";
import { isRepoPath, rewriteHref } from "./docs";

export interface DocLinksOptions {
  /** The file the Markdown comes from, relative to the repository root. */
  from: string;
  /** Doc source file to route, see `routesBySource`. */
  routes: ReadonlyMap<string, string>;
}

/**
 * Rewrites the relative links of a doc, see `rewriteHref`. Inline and reference-style links are rewritten.
 *
 * Images that are files of the repository stop the build: the site serves none, and pointing them at GitHub would make
 * every visitor's browser contact another site. Images with a full url or a data url are left for `check-dist`.
 */
const remarkDocLinks: Plugin<[DocLinksOptions], Root> = ({ from, routes }) => {
  const refuse = (url: string): never => {
    throw new Error(
      `${from} has an image from the repository (${url}). The site serves no images from the docs yet, so leave it out or add support in src/lib/remark-doc-links.ts.`,
    );
  };

  return (tree) => {
    const imageReferences = new Set<string>();
    visit(tree, "imageReference", (node) => {
      imageReferences.add(node.identifier);
    });

    visit(tree, (node) => {
      if (node.type === "image") {
        if (isRepoPath(node.url)) refuse(node.url);
      } else if (node.type === "definition") {
        if (imageReferences.has(node.identifier)) {
          if (isRepoPath(node.url)) refuse(node.url);
        } else {
          node.url = rewriteHref(node.url, from, routes);
        }
      } else if (node.type === "link") {
        node.url = rewriteHref(node.url, from, routes);
      }
    });
  };
};

export default remarkDocLinks;
