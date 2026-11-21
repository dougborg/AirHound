import test from "node:test";
import assert from "node:assert/strict";
import { normalizeReleases, selectChannels } from "../scripts/releases.mjs";
function release(tag, preview, date, assets = []) {
  return {
    tag_name: tag,
    prerelease: preview,
    published_at: date,
    html_url: `https://github.com/dougborg/AirHound/releases/tag/${tag}`,
    assets,
  };
}
function asset(tag, name) {
  return {
    name,
    size: 10,
    browser_download_url: `https://github.com/dougborg/AirHound/releases/download/${tag}/${name}`,
  };
}
test("preview does not replace the stable channel", () => {
  const rs = normalizeReleases([
    release("v1", false, "2026-01-01"),
    release("v2", true, "2026-02-01"),
  ]);
  assert.equal(selectChannels(rs).stable.tag, "v1");
  assert.equal(selectChannels(rs).preview.tag, "v2");
});
test("draft and unpublished releases are never advertised", () => {
  assert.deepEqual(
    normalizeReleases([
      { ...release("v2", true, "2026-02-01"), draft: true },
      release("v3", false, null),
    ]),
    [],
  );
});
test("missing assets stay missing and preview-only is explicit", () => {
  const rs = normalizeReleases([release("v2", true, "2026-02-01")]);
  assert.equal(selectChannels(rs).stable, null);
  assert.deepEqual(rs[0].assets, []);
});
test("exact board runtime and architecture metadata remain distinct", () => {
  const rs = normalizeReleases([
    release("v2", true, "2026-02-01", [
      asset("v2", "airhound-m5stickc.bin"),
      asset("v2", "airhound-m5stickc-std.bin"),
      asset("v2", "unknown.zip"),
    ]),
  ]);
  assert.equal(rs[0].assets.length, 2);
  assert.notEqual(rs[0].assets[0].target, rs[0].assets[1].target);
});
test("rejects unexpected asset host or release URL", () => {
  assert.throws(
    () =>
      normalizeReleases([
        release("v2", true, "2026-02-01", [
          {
            ...asset("v2", "airhound-m5stickc.bin"),
            browser_download_url: "https://example.org/firmware",
          },
        ]),
      ]),
    /Unexpected asset/,
  );
  assert.throws(
    () =>
      normalizeReleases([
        { ...release("v2", true, "2026-02-01"), html_url: "javascript:bad" },
      ]),
    /identity/,
  );
});
