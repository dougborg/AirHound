import { defineConfig } from "astro/config";
import starlight from "@astrojs/starlight";
import { satteri } from "@astrojs/markdown-satteri";
import importedHeadings from "./scripts/imported-headings.mjs";
export default defineConfig({
  markdown: { processor: satteri({ hastPlugins: [importedHeadings] }) },
  site: "https://airhound.tools",
  trailingSlash: "always",
  integrations: [
    starlight({
      title: "AirHound",
      description:
        "AirHound setup guides, receiver reference, and development documentation.",
      customCss: ["./src/styles/docs.css"],
      social: [
        {
          icon: "github",
          label: "GitHub",
          href: "https://github.com/dougborg/AirHound",
        },
      ],
      sidebar: [
        { label: "Project home", link: "/" },
        { label: "Getting started", link: "/start/" },
        { label: "Downloads", link: "/downloads/" },
        { label: "Hardware support", link: "/hardware/" },
        { label: "Signatures", link: "/signatures/" },
        {
          label: "Documentation",
          items: [{ autogenerate: { directory: "reference" } }],
        },
      ],
      head: [
        {
          tag: "meta",
          attrs: {
            name: "robots",
            content:
              process.env.SITE_PRODUCTION === "true"
                ? "index,follow"
                : "noindex,nofollow",
          },
        },
      ],
    }),
  ],
});
