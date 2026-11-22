import GithubSlugger from "github-slugger";

// Starlight supplies the page H1. Reserve its source slug before removing it,
// so subsequent duplicate headings keep their original anchor suffixes.
export function importedHeadingVisitor() {
  const slugger = new GithubSlugger();
  let removedTitle = false;
  return {
    name: "airhound-imported-titles",
    element: {
      filter: ["h1", "h2", "h3", "h4", "h5", "h6"],
      visit(node, ctx) {
        const id = slugger.slug(ctx.textContent(node));
        if (node.tagName === "h1" && !removedTitle) {
          removedTitle = true;
          return {
            type: "element",
            tagName: "span",
            properties: { id },
            children: [],
          };
        }
        return {
          ...node,
          tagName: node.tagName === "h1" ? "h2" : node.tagName,
          properties: { ...node.properties, id },
        };
      },
    },
  };
}

export default function importedHeadings({ fileURL }) {
  if (fileURL?.pathname.includes("/content/docs/reference/")) {
    return importedHeadingVisitor();
  }
}
