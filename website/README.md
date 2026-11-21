# AirHound project website

Repo-owned Astro + Starlight site for airhound.tools. This is public project documentation, not a detector dashboard. Scope and follow-up: [#190](https://github.com/dougborg/AirHound/issues/190).

## Develop and validate

Use Node 24 LTS and npm. From `website/`:

```sh
npm ci
npm run dev
npm run validate
npm run check
npx playwright install chromium
npx playwright test
```

`npm run build` produces `dist/`; `npm run preview` serves it locally. The CI workflow builds on pull requests and main, tests navigation/filtering/local search/no-JavaScript content, runs axe accessibility checks, and uploads `website-preview` plus screenshot evidence. It **does not deploy** and requires no deployment secrets.

## Content ownership

- `scripts/prepare-content.mjs` imports the repository README, component READMEs and docs into ignored generated Starlight pages. Edit canonical source Markdown, never generated copies. Internal document links are rewritten using a Markdown AST; other source links pin the build's git revision. In-page anchors stay intact. Imported docs are clearly labeled development documentation.
- `src/pages/` owns onboarding and presentation, not duplicate component manuals.
- `src/data/hardware.json` is the website summary with evidence links; update alongside receiver/release acceptance docs. Do not infer support from enumeration or parser tests.
- The signature catalog reads `../signatures/builtin.v2.json` directly; legacy-unvalidated entries remain visibly unvalidated. It does not install runtime packs.
- `src/data/releases.json` is a reviewed snapshot of published release metadata. Run `npm run sync-releases` after publication and inspect the diff in a PR. The sync fails without overwriting on API/validation failure; ordinary builds are offline/deterministic. It reads up to the latest 100 releases (sufficient for the current history); add pagination before history exceeds that. The page shows the check date and links the upstream release list. No browser token or live API dependency.
- Match exact platform/board/runtime asset names in `scripts/releases.mjs`. Unknown/missing assets are not fabricated; the original release remains linked. Tagged source/doc links give immutable release context; main docs are not mislabeled release snapshots.

## Deployment runbook — GitHub Pages

1. Review the static artifact and acceptance results. Resolve hosting/domain ownership with the maintainer. GitHub Pages is the selected host for `https://airhound.tools`.
2. Identify the domain administrator, renewal responsibility and existing DNS records. Verify domain ownership before attaching the repository, choose apex/www canonical redirects, and preserve unrelated records. Never infer DNS authorization from a code change.
3. `.github/workflows/website-deploy.yml` validates and deploys on relevant pushes to `main`, or manually with `gh workflow run website-deploy.yml --ref main`. Other branches and forks cannot deploy. Set repository Settings → Pages → Source to GitHub Actions and restrict the `github-pages` environment to `main`. Only the deployment job receives `pages: write` and `id-token: write`; all actions are commit-pinned. PRs only build through the separate Website workflow.
4. The deployment workflow builds with `SITE_PRODUCTION=true` for indexing. All normal review builds emit `noindex,nofollow`. `site` is already configured to `https://airhound.tools`; the custom domain is bound in Pages settings (Actions deployments do not require a CNAME file). Verify HTTPS, redirects, certificate issuance, mixed content, sitemap and canonical URLs after launch.
5. Update the privacy page to name the actual hosting provider and its logs/privacy policy; verify the repository security-reporting route. No application analytics/cookies, external fonts or tracking scripts are included.
6. Record the deployment artifact/commit, owner and rollback command. Roll back by rebuilding/redeploying the last reviewed commit; maintain compatible content/asset links. Preview artifact retention is 14 days. Unbind custom domains safely before deleting a host to avoid abandoned-domain takeover.

Maintainer responsibilities: domain renewal and access, trusted deployment approvals, dependency/security reviews (weekly Dependabot), release snapshot updates after publication, and receiver-status review for every release. These are responsibilities to assign before launch, not claims about existing hosting.

## Quality and remaining launch gates

The homepage uses self-hosted Manrope and Newsreader (SIL OFL; licenses are distributed with their npm packages). Essential content and links work without JavaScript; catalog filters are progressive enhancements. The docs search is local Pagefind. Internal page/anchor validation and a 100 KiB gzip homepage-JS budget are enforced by `check-site.mjs`.

Automated axe checks target WCAG A/AA rules but do not establish conformance. Before production launch, finish manual keyboard/screen-reader checks, review mobile/desktop screenshots, repeat the recorded Lighthouse mobile measurement when content changes, and resolve any hardware/release wording discrepancies. Full archived versioned-doc snapshots and hosted PR preview URLs are later enhancements, not implemented claims. No website changes close hardware acceptance gates.


## Initial review evidence (2026-11-21)

- 46 generated pages; all internal links/anchors checked, 9 focused data/URL tests, Astro type checks, 7 Chromium browser tests (including mobile/desktop axe, local search, filters and no-JavaScript essentials) passed locally.
- Homepage Lighthouse: performance 100, accessibility 100, best practices 100; ~94,461 bytes initial transfer, simulated LCP 0.75 s, CLS 0. Local HTTP preview, Chromium 159, Lighthouse mobile 412×823, simulated 150 ms RTT / 1.6 Mbps / 4× CPU slowdown. This is a local lab result, not real-user production performance or full accessibility conformance.
- Homepage HTML ~2.2 KiB gzip and referenced JS ~1.1 KiB gzip. No third-party runtime network requests are required.
- Desktop 1440×1000 and mobile 390×844 full-page screenshots are generated in the ignored `.impeccable/review/` directory and uploaded by CI. No production deployment occurred.
