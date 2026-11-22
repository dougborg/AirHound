---
name: AirHound website
description: Blue schematic and field-atlas project reference
colors:
  paper: "#f2f5f7"
  ink: "#182e49"
  muted: "#4c617c"
  line: "#b9c9dd"
  accent: "#1552ce"
  accent-hover: "#103da0"
  wash: "#e7edf5"
  tag-bg: "#dce7f6"
  tag-text: "#24476e"
  experimental-bg: "#f2e3cc"
  experimental-text: "#704113"
  research-bg: "#e3e7ec"
  research-text: "#42536a"
  field-border: "#758ba6"
  docs-dark-bg: "#101a28"
  docs-dark-nav: "#142033"
  docs-dark-line: "#344c6d"
  docs-dark-accent-low: "#172d51"
  docs-dark-accent: "#6f9fff"
  docs-dark-accent-high: "#c4d8ff"
  docs-light-accent-high: "#123b83"
typography:
  display:
    fontFamily: "Manrope Variable, sans-serif"
    fontSize: "clamp(2rem, 3.6vw, 3.25rem)"
    fontWeight: 800
    lineHeight: 1.12
    letterSpacing: "-0.035em"
  headline:
    fontFamily: "Manrope Variable, sans-serif"
    fontSize: "1.55rem"
    fontWeight: 750
    lineHeight: 1.3
    letterSpacing: "-0.025em"
  title:
    fontFamily: "Manrope Variable, sans-serif"
    fontSize: "1rem"
    fontWeight: 750
    lineHeight: 1.5
  body:
    fontFamily: "Manrope Variable, sans-serif"
    fontSize: "1rem"
    fontWeight: 400
    lineHeight: 1.75
  navigation:
    fontFamily: "Manrope Variable, sans-serif"
    fontSize: "0.82rem"
    fontWeight: 700
  technical:
    fontFamily: "ui-monospace, monospace"
    fontSize: "0.8rem"
rounded:
  field: "2px"
spacing:
  gutter-desktop: "48px"
  gutter-tablet: "28px"
  gutter-mobile: "20px"
  row: "24px"
components:
  text-link:
    textColor: "{colors.accent}"
  text-link-hover:
    textColor: "{colors.accent-hover}"
  filter-field:
    backgroundColor: "{colors.paper}"
    textColor: "{colors.ink}"
    rounded: "{rounded.field}"
    padding: "11px 13px"
  status-tag:
    backgroundColor: "{colors.tag-bg}"
    textColor: "{colors.tag-text}"
    rounded: "{rounded.field}"
    padding: "4px 7px"
  notice:
    backgroundColor: "{colors.wash}"
    padding: "20px 24px"
---

# Design System: AirHound website

## Overview

**Creative North Star: "Blue schematic / field atlas"**

The approved blue schematic and field-atlas direction makes AirHound a readable project reference. Plain headings, ruled rows and direct links keep capabilities, setup choices and evidence easy to find. Schematic marks explain receiver context without implying live observations or transmitter positions.

Self-hosted Manrope and cool paper surfaces connect custom pages with imported Starlight documentation. Product constraints and content ownership are recorded in [PRODUCT.md](PRODUCT.md); canonical manuals remain in repository Markdown.

**Key Characteristics:**

- Blue schematic linework and restrained atlas imagery.
- Compact reference rows, readable prose and visible setup links.
- Plain headings and text links without decorative arrows.

## Colors

### Primary

Schematic blue (`accent`) identifies links, the homepage title, focus outlines and route marks. Its darker hover variant reinforces interaction. The documentation dark theme uses its brighter accent family for legibility.

### Neutral

Cool paper, deep blue ink, muted blue-gray text, fine rules and wash backgrounds establish the reading surface. Status tags add labeled experimental and research variants; color never stands alone as the support verdict. Dark documentation uses its own background, navigation and hairline tokens. Light documentation reuses the custom-page palette, with its own high accent token.

## Typography

Manrope Variable is self-hosted for headings, body and navigation. The frontmatter hierarchy describes custom pages; Starlight retains its own responsive type scale, with the same font and heading weights (800 for H1; 750 for H2/H3). Custom H1 tracking is tighter than the documentation heading tracking. Technical filenames, rule IDs and JSON use system monospace.

Body paragraphs are normally capped at 70ch; page leads and reference summaries use wider measures. Smaller catalog text supports dense evidence reading. Headings describe the section directly, without slogans.

## Layout

Custom pages use a centered container capped at 1280px with 48px outer gutters. At 1100px and below, gutters become 28px and the main navigation wraps onto its own row. At 700px and below, gutters become 20px, two-column content stacks and navigation remains visible. These are custom-page breakpoints; Starlight owns its documentation shell and responsive navigation.

The homepage pairs capability rows with a 285px setup/reference sidebar (255px at the intermediate breakpoint). Rows use rules rather than floating cards. The receiver illustration follows the reference content in a two-column section that stacks on mobile. General editorial pages use a 1.2:1 grid; signature entries use a 240px label column and flexible detail column before stacking.

Tables scroll horizontally; mobile tables retain a 650px minimum width and show a scroll hint. Filters wrap and become full width on mobile. Main navigation and content links remain usable without JavaScript; catalog filtering progressively enhances the content.

## Elevation & Depth

Custom pages use flat tonal panels and thin rules, with no card shadows. Imported documentation keeps Starlight component behavior; pagination shadows are explicitly removed. The pale route grid and blue path create schematic hierarchy without a simulated dashboard.

Color and background transitions last 0.15s on custom-page links and buttons only when reduced motion is not requested. No content animation is required.

## Shapes

Most reference surfaces are square. Fields and status labels use a small (2px) radius. Circular observation nodes belong to the schematic illustration. The geometric AirHound monogram remains temporary pending the logo work in #130.

## Components

- **Text links:** blue with an offset underline; hover darkens blue and thickens the underline. Custom pages have no filled primary-action button pattern. Do not infer one from the palette.
- **Navigation:** compact bold text; hover and the current page use blue and underlines. The custom shell wraps visibly on mobile. Starlight maintains its documentation navigation and light/dark selector.
- **Filter fields:** paper surface, blue-gray border, small corner radius and inherited font; labels remain visible. Global custom-page focus is a blue 3px outline offset by 4px. Documentation uses its theme's text accent for the same outline dimensions.
- **Status labels:** compact text with separate default, experimental and research treatments. Labels describe validation state explicitly.
- **Notices and reference panels:** flat wash with rules or borders; notices use 20px by 24px padding, reduced to 18px on mobile.
- **Tables and signature rows:** dense, ruled reference structures. Tables use an ink header with paper text; filenames and IDs wrap, and JSON scrolls within its block.
- **Imported manuals:** canonical Markdown is imported with source revision and development context. The page title appears once; source heading anchors remain available. Edit source manuals rather than generated copies, as described in [README.md](README.md#content-ownership).

The implementation sources are [site.css](src/styles/site.css), [docs.css](src/styles/docs.css), the [custom shell](src/layouts/Site.astro) and [Starlight configuration](astro.config.mjs). Framework defaults not overridden there remain framework-owned, rather than new AirHound tokens.

## Do's and Don'ts

- Do use plain headings and underline links with meaningful labels.
- Do retain explicit status text alongside status colors.
- Do preserve visible focus, narrow-screen table scrolling and reduced-motion behavior.
- Do label synthetic receiver illustrations and preserve canonical documentation links.
- Don't add decorative arrows to ordinary text links.
- Don't turn the reference into a marketing landing page or scanner dashboard.
- Don't present receiver coordinates as transmitter locations or invent observed data.
- Don't treat the temporary monogram as the approved final logo (#130).
