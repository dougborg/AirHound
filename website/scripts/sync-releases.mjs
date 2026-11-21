import { writeFile } from "node:fs/promises";
import { normalizeReleases, REPO } from "./releases.mjs";
const response = await fetch(
  `https://api.github.com/repos/${REPO}/releases?per_page=100`,
  {
    headers: {
      Accept: "application/vnd.github+json",
      "User-Agent": "AirHound-website-release-catalog",
      ...(process.env.GITHUB_TOKEN
        ? { Authorization: `Bearer ${process.env.GITHUB_TOKEN}` }
        : {}),
    },
  },
);
if (!response.ok)
  throw new Error(
    `GitHub releases: ${response.status}; existing catalog is unchanged`,
  );
const releases = normalizeReleases(await response.json());
await writeFile(
  new URL("../src/data/releases.json", import.meta.url),
  JSON.stringify({ checkedAt: new Date().toISOString(), releases }, null, 2) +
    "\n",
);
console.log(
  `Catalog refreshed: ${releases.length} published releases; review the diff before committing`,
);
