import { readFile, writeFile, mkdir, readdir, rm } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { execFileSync } from "node:child_process";
import { unified } from "unified";
import remarkParse from "remark-parse";
import remarkStringify from "remark-stringify";
import { visit } from "unist-util-visit";
export const root = fileURLToPath(new URL("../../", import.meta.url));
export const routeFor = (file) =>
  "/reference/" +
  file
    .replace(/\.md$/i, "")
    .toLowerCase()
    .replace(/\/readme$/, "")
    .replace(/^readme$/, "project") +
  "/";
export function rewriteUrl(url, source, sources, revision) {
  if (/^(?:[a-z][a-z0-9+.-]*:|#|\/)/i.test(url)) return url;
  const [relative, fragment] = url.split("#");
  const resolved = path.posix.normalize(
    path.posix.join(path.posix.dirname(source), relative),
  );
  if (resolved.startsWith("../"))
    throw new Error(`Link escapes repository: ${source}: ${url}`);
  const target = sources.has(resolved)
    ? routeFor(resolved)
    : `https://github.com/dougborg/AirHound/blob/${revision}/${resolved}`;
  return target + (fragment ? "#" + fragment : "");
}
export async function prepare() {
  const docs = (await readdir(path.join(root, "docs")))
    .filter((f) => f.endsWith(".md"))
    .map((f) => "docs/" + f);
  const files = [
    "README.md",
    "CONTRIBUTING.md",
    "CHANGELOG.md",
    ...docs,
    "src/README.md",
    "firmware-std/README.md",
    "hostd/README.md",
    "companions/flipper/README.md",
    "drivers/README.md",
    "drivers/mt7610u/README.md",
    "drivers/mt7610u/TESTING.md",
    "signatures/README.md",
    "schemas/README.md",
    "scripts/README.md",
  ];
  const sources = new Set(files);
  const revision = execFileSync("git", ["rev-parse", "HEAD"], {
    cwd: root,
    encoding: "utf8",
  }).trim();
  const output = path.join(root, "website/src/content/docs/reference");
  await rm(output, { recursive: true, force: true });
  for (const source of files) {
    const original = await readFile(path.join(root, source), "utf8");
    const title = original.match(/^# (.+)$/m)?.[1] ?? source;
    const parser = unified()
      .use(remarkParse)
      .use(remarkStringify, { fences: true });
    const tree = parser.parse(original);
    for (const kind of ["link", "image", "definition"])
      visit(tree, kind, (node) => {
        node.url = rewriteUrl(node.url, source, sources, revision);
      });
    const content = parser.stringify(tree);
    const route = routeFor(source);
    const file = path.join(
      root,
      "website/src/content/docs",
      route.replace(/^\//, "") + "index.md",
    );
    await mkdir(path.dirname(file), { recursive: true });
    await writeFile(
      file,
      `---\ntitle: ${JSON.stringify(title)}\neditUrl: ${JSON.stringify("https://github.com/dougborg/AirHound/edit/main/" + source)}\n---\n\n> **Development documentation** · Built from [${source} at ${revision.slice(0, 7)}](https://github.com/dougborg/AirHound/blob/${revision}/${source}). This may include changes newer than a release. [Choose release-specific downloads and source](/downloads/).\n\n${content}`,
    );
  }
  console.log(
    `Imported ${files.length} canonical documents at ${revision.slice(0, 7)}`,
  );
}
if (process.argv[1] === fileURLToPath(import.meta.url)) await prepare();
