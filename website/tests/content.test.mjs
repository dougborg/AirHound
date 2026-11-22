import test from "node:test";
import assert from "node:assert/strict";
import { rewriteUrl, routeFor } from "../scripts/prepare-content.mjs";
const sources = new Set(["docs/firmware.md", "hostd/README.md"]);
test("canonical relative docs map to site routes and retain fragments", () => {
  assert.equal(
    rewriteUrl(
      "../hostd/README.md#permissions",
      "docs/firmware.md",
      sources,
      "abc",
    ),
    "/reference/hostd/#permissions",
  );
  assert.equal(routeFor("docs/firmware.md"), "/reference/docs/firmware/");
});
test("source files use immutable revision links", () =>
  assert.equal(
    rewriteUrl("../src/main.rs", "docs/firmware.md", sources, "abc"),
    "https://github.com/dougborg/AirHound/blob/abc/src/main.rs",
  ));
test("external links and same-page anchors stay unchanged", () => {
  assert.equal(
    rewriteUrl("#board", "docs/firmware.md", sources, "abc"),
    "#board",
  );
  assert.equal(
    rewriteUrl("https://example.org/a", "docs/firmware.md", sources, "abc"),
    "https://example.org/a",
  );
});
test("links cannot escape the repository", () =>
  assert.throws(
    () => rewriteUrl("../../secret", "README.md", sources, "abc"),
    /escapes/,
  ));

test("imported titles retain anchors without duplicate visible headings", async () => {
  const { importedHeadingVisitor } =
    await import("../scripts/imported-headings.mjs");
  const {
    element: { visit },
  } = importedHeadingVisitor();
  const ctx = { textContent: (node) => node.children[0].value };
  const heading = (tagName, value) => ({
    type: "element",
    tagName,
    properties: {},
    children: [{ type: "text", value }],
  });
  const title = visit(heading("h1", "AirHound docs"), ctx);
  assert.equal(title.tagName, "span");
  assert.equal(title.properties.id, "airhound-docs");
  assert.deepEqual(title.children, []);
  assert.equal(
    visit(heading("h2", "AirHound docs"), ctx).properties.id,
    "airhound-docs-1",
  );
  assert.equal(
    visit(heading("h2", "Permissions"), ctx).properties.id,
    "permissions",
  );
  assert.equal(visit(heading("h1", "Other title"), ctx).tagName, "h2");
});
