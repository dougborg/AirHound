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
