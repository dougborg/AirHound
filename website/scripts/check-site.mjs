import { readdir, readFile } from "node:fs/promises";
import { load } from "cheerio";
import { gzipSync } from "node:zlib";
const dist = new URL("../dist/", import.meta.url);
async function walk(dir) {
  let out = [];
  for (const e of await readdir(dir, { withFileTypes: true })) {
    const p = new URL(e.name + (e.isDirectory() ? "/" : ""), dir);
    out.push(...(e.isDirectory() ? await walk(p) : [p]));
  }
  return out;
}
const files = await walk(dist);
const pages = new Map();
const errors = [];
for (const file of files.filter((f) => f.pathname.endsWith(".html"))) {
  const html = await readFile(file, "utf8");
  const relative = file.pathname.slice(dist.pathname.length);
  const route =
    relative === "index.html"
      ? "/"
      : "/" + relative.replace(/index\.html$/, "");
  pages.set(route, { html, $: load(html) });
}
for (const [route, { $, html }] of pages) {
  if (!$("title").text()) errors.push(`${route}: missing title`);
  if (!$("html").attr("lang")) errors.push(`${route}: missing language`);
  if ($("h1").length !== 1) errors.push(`${route}: expected one H1`);
  $("a[href]").each((_, a) => {
    const href = $(a).attr("href");
    if (!href || (!href.startsWith("/") && !href.startsWith("#"))) return;
    const url = new URL(href, "https://airhound.tools" + route);
    const target = pages.get(url.pathname);
    if (!target) {
      if (
        !files.some((f) => f.pathname === dist.pathname + url.pathname.slice(1))
      )
        errors.push(`${route}: missing ${href}`);
      return;
    }
    if (url.hash) {
      const id = decodeURIComponent(url.hash.slice(1));
      if (
        !target
          .$("[id]")
          .toArray()
          .some((e) => target.$(e).attr("id") === id)
      )
        errors.push(`${route}: missing anchor ${href}`);
    }
  });
  if (route === "/") {
    let js = 0;
    for (const s of $("script[src]").toArray()) {
      const src = $(s).attr("src");
      if (src.startsWith("/"))
        js += gzipSync(await readFile(new URL("." + src, dist))).length;
    }
    if (js > 100 * 1024) errors.push(`Homepage JS ${js} exceeds 100 KiB gzip`);
    console.log(
      `Homepage HTML: ${gzipSync(html).length} bytes gzip; referenced JS: ${js} bytes gzip`,
    );
  }
}
if (errors.length) {
  console.error(errors.join("\n"));
  process.exitCode = 1;
} else console.log(`Validated ${pages.size} pages, internal links and anchors`);
