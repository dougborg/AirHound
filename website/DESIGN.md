# AirHound website design

The public project site uses an editorial field-guide composition: an oversized serif thesis, compact navigation, sequential setup paths and evidence tables. Documentation is a reading surface; downloads and catalogs are task surfaces. Product truth is recorded in PRODUCT.md.

## Tokens
- Paper `#f7f4ed`; ink `#24342d`; secondary text `#526257`; rule `#c8cec2`.
- Rust accent `#a53e1e`; pale field `#e9ede2`; dark signal diagram `#203c31`.
- Self-hosted Newsreader Variable for display/section headings; Manrope Variable for reading/navigation. Monospace only for filenames, rule IDs and JSON.
- Desktop content width 1280px; 48px outer margins; mobile 18px. Reading text max 70ch.
- Primary action has ink fill, paper text, 3px radius. Status labels use text plus color. Tables retain horizontal scrolling on narrow screens.
- Default surface is light for daytime field/reference reading; imported Starlight docs retain an accessible light/dark control.

## Surface contract
Homepage: explain observations → matching → reviewed evidence and send a visitor to their setup. No fake captures, claims or metrics. The diagram shows process, not live state. The temporary geometric monogram is not the final logo (#130).
Documentation: canonical Markdown imports, immutable source revision, development context, local search.
Downloads: published metadata only, preview/stable distinction, explicit platform/board/runtime.
Hardware: setup and evidence per source; research cannot look supported.
Signatures: canonical rule extraction, validation/limitations visible, no runtime installation claim.

## Responsive and interaction rules
Main navigation wraps visibly on mobile; no hidden mobile menu is needed. Essential content works without scripts. Filters progressively enhance tables/catalogs. Main-site focus rings use a 3px rust outline with 5px offset; Starlight documentation uses #bd5128 with 4px offset. Mobile download tables show an explicit horizontal-scroll hint. Reduced motion gets static content. No analytics, remote fonts or third-party embeds.
