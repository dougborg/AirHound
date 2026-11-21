import { defineConfig } from "astro/config";
import starlight from "@astrojs/starlight";
export default defineConfig({
  site: "https://airhound.tools",
  trailingSlash: "always",
  integrations: [
    starlight({
      title: "AirHound",
      description: "Observe wireless signals. Understand the evidence.",
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
        { label: "Get started", link: "/start/" },
        { label: "Downloads", link: "/downloads/" },
        { label: "Hardware & receivers", link: "/hardware/" },
        { label: "Signature evidence", link: "/signatures/" },
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
