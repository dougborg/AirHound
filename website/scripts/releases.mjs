export const REPO = "dougborg/AirHound";
export const expectedAssets = {
  "airhound-m5stickc.bin": {
    component: "Firmware",
    platform: "Embedded",
    target: "M5StickC Plus2 · no_std",
  },
  "airhound-xiao.bin": {
    component: "Firmware",
    platform: "Embedded",
    target: "XIAO ESP32-S3 · no_std",
  },
  "airhound-m5stickc-std.bin": {
    component: "Firmware",
    platform: "Embedded",
    target: "M5StickC Plus2 · ESP-IDF",
  },
  "airhound-xiao-std.bin": {
    component: "Firmware",
    platform: "Embedded",
    target: "XIAO ESP32-S3 · ESP-IDF",
  },
  "airhound-desktop-macos-arm64.tar.gz": {
    component: "Desktop",
    platform: "macOS",
    target: "macOS 15+ · Apple Silicon",
  },
  "airhound-desktop-linux-x86_64.tar.gz": {
    component: "Desktop",
    platform: "Linux",
    target: "Linux x86-64 · glibc 2.35+",
  },
  "mt7610u-lab-macos-arm64.tar.gz": {
    component: "ALFA receiver",
    platform: "macOS",
    target: "macOS arm64 · experimental",
  },
  "mt7610u-lab-linux-x86_64.tar.gz": {
    component: "ALFA receiver",
    platform: "Linux",
    target: "Linux x86-64 · experimental",
  },
  "mt7610u-lab-macos-arm64-source.tar.gz": {
    component: "Corresponding source",
    platform: "macOS",
    target: "Exact vendored receiver source",
  },
  "mt7610u-lab-linux-x86_64-source.tar.gz": {
    component: "Corresponding source",
    platform: "Linux",
    target: "Exact vendored receiver source",
  },
  "airhound-source.tar.gz": {
    component: "Source & tools",
    platform: "All",
    target: "Tracked source, scripts, schemas and docs",
  },
  SHA256SUMS: {
    component: "Checksums",
    platform: "All",
    target: "Release artifact integrity",
  },
};
export function normalizeReleases(items) {
  if (!Array.isArray(items))
    throw new Error("Release response must be an array");
  return items
    .filter((r) => !r.draft && r.published_at)
    .map((r) => {
      if (
        typeof r.tag_name !== "string" ||
        !/^https:\/\/github\.com\/dougborg\/AirHound\/releases\/tag\//.test(
          r.html_url,
        )
      )
        throw new Error("Unexpected release identity");
      const prefix = `https://github.com/${REPO}/releases/download/${encodeURIComponent(r.tag_name)}/`;
      const assets = (r.assets ?? [])
        .filter((a) => expectedAssets[a.name])
        .map((a) => {
          if (a.browser_download_url !== prefix + encodeURIComponent(a.name))
            throw new Error(`Unexpected asset URL: ${a.name}`);
          return {
            name: a.name,
            url: a.browser_download_url,
            size: a.size,
            ...expectedAssets[a.name],
          };
        });
      const names = new Set(assets.map((a) => a.name));
      if (names.size !== assets.length)
        throw new Error("Duplicate release asset");
      return {
        tag: r.tag_name,
        name: r.name || r.tag_name,
        preview: !!r.prerelease,
        publishedAt: r.published_at,
        url: r.html_url,
        assets,
      };
    })
    .sort((a, b) => b.publishedAt.localeCompare(a.publishedAt));
}
export function selectChannels(releases) {
  return {
    stable: releases.find((r) => !r.preview) ?? null,
    preview: releases.find((r) => r.preview) ?? null,
  };
}
