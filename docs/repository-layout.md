# Repository layout and build roots

AirHound has several independently built components. The root Rust package still
contains both the portable library and the no_std firmware executable. Desktop,
ESP-IDF firmware, the GPL receiver, fuzzing and the website have separate dependency
resolution or tooling. Start from the component you intend to change.

## Find the right component

Commands below run from the repository root unless a working directory is shown.
Install the platform prerequisites from the linked component guide first.

| Component | Source and entry point | Toolchain / lockfile | Build or validation command |
| --- | --- | --- | --- |
| Portable detection core | [src/](../src/README.md); root `Cargo.toml` library | Explicit stable Rust; root `Cargo.lock` | `cargo +stable test --locked --lib --no-default-features` |
| no_std firmware | `src/main.rs`, hardware modules and root `Cargo.toml` binary; [firmware guide](firmware.md) | Root `esp` toolchain and `Cargo.lock` | `just build-xiao` or `just build-m5stickc`; container alternatives in `just --list` |
| ESP-IDF firmware | [firmware-std/](../firmware-std/README.md) | Local `esp` toolchain, Cargo config, SDK config and `Cargo.lock` | Follow the board-specific build commands in the component README |
| macOS/Linux desktop CLI | [hostd/](../hostd/README.md) | Local stable Rust; `hostd/Cargo.lock` | `just build-hostd`, `just test-hostd`, `just clippy-hostd` |
| Shared control API and Rust client | `hostd/crates/airhound-api`, `hostd/crates/airhound-client`; [contract](shared-backend-api.md) | Desktop workspace and `hostd/Cargo.lock` | `cargo +stable test --manifest-path hostd/Cargo.toml --workspace --locked` |
| Terminal client | [clients/tui/](../clients/tui/README.md) | Independent stable Rust workspace and lockfile | `cargo +stable test --manifest-path clients/tui/Cargo.toml --locked` |
| MCP adapter | [clients/mcp/](../clients/mcp/README.md) | Independent stable Rust workspace and lockfile | `cargo +stable test --manifest-path clients/mcp/Cargo.toml --locked` |
| Native macOS GUI | [apps/macos/](../apps/macos/README.md) | Swift 5.9+, macOS 14+; no third-party Swift dependencies | In `apps/macos`: `swift test`, `sh bundle.sh` |
| ALFA MT7610U userspace receiver | [drivers/mt7610u/](../drivers/mt7610u/README.md) | Independent stable Rust package/workspace and lockfile | In `drivers/mt7610u`: `cargo test --locked` |
| Flipper companion prototype | [companions/flipper/](../companions/flipper/README.md) | Independent stable Rust package/lockfile; API87.1 FAP target | In `companions/flipper`: `cargo test --locked`; explicit ARM build in its guide |
| Fuzz harnesses | `fuzz/`; [testing guide](testing.md) | Independent Cargo workspace/lockfile; explicitly select nightly for cargo-fuzz | `cargo +nightly fuzz run commands -- -max_total_time=30` |
| Canonical signatures | [signatures/](../signatures/README.md) | JSON data; Python compiler/tests | `python3 scripts/compile-signatures.py --check` |
| Public schemas/contracts | [schemas/](../schemas/README.md) | JSON Schema and examples | `just check-schemas` and `just check-examples` |
| Build, packaging and capture tools | [scripts/](../scripts/README.md) | Python; requirements vary by tool | `python3 -B -m unittest discover -s scripts/tests -v` |
| Project website | [website/](../website/README.md) | Node/npm; `website/package-lock.json` | In `website`: `npm ci`, `npm run validate`, `npm run check` |
| Documentation and automation | [docs/](README.md), `.github/`, `.github/scripts/`, `justfile` | Workflow-pinned tools and existing component toolchains | [Testing and CI reference](testing.md) |

Running bare `cargo` at the repository root selects the ESP toolchain and default
firmware features. Use the explicit stable/no-default-features command for portable
core development. Run desktop and receiver commands inside their own build roots
so local toolchain selection is honored. Independent lockfiles are intentional;
there is not yet one Cargo workspace for every component.

## Ownership and licensing

The main AirHound code is MIT licensed. The MT7610U receiver is a separate
GPL-2.0-only project and communicates with AirHound over a versioned process
protocol. Keep its notices and corresponding-source packaging together. The
Sniffle helper in `scripts/` has its own GPL-3.0-only license; that exception is
documented in the scripts guide. Hardware firmware, dependencies and website
fonts have their own terms. Moving a file does not change its license.

Canonical signature rules live in `signatures/builtin.v2.json`; do not hand-edit
`src/generated_signatures.rs`. Canonical guides live in component READMEs and
`docs/`. The website imports those guides at build time; generated pages are not a
second editable manual. Its release catalog is a reviewed snapshot updated after
publication, not a live browser API query.

## Outputs and private evidence

Rust build outputs remain in each component's `target/`; coverage reports use
`coverage/`, ESP-IDF also uses `.embuild`, and the macOS bundle is
`hostd/AirHound.app`. Website output is `website/dist`, imported reference pages
are generated under `website/src/content/docs/reference`, and browser-review
artifacts are under `website/.impeccable/review` and `website/test-results`.
These generated locations are ignored; named fuzz seed fixtures are deliberately
tracked while generated findings are not.

Keep raw radio captures, GPS trails, device identifiers, credentials and full flash
backups outside tracked files. `secrets/` directories are ignored, but verify
`git check-ignore` and `git status` before committing new evidence. Release source
archives must contain only intended tracked source, never local recovery files.

## Planned organization changes

[#200](https://github.com/dougborg/AirHound/issues/200) coordinates the repository
layout; [#33](https://github.com/dougborg/AirHound/issues/33) owns extraction of the
portable library into `crates/airhound-core/` and no_std executable into
`firmware/`. Those directories are proposals, not current build roots. Existing
`hostd/`, `firmware-std/`, `drivers/`, `schemas/`, `signatures/` and `website/`
paths stay in place initially.

Extraction must update Cargo dependencies, toolchain/config discovery, CI path
filters/caches, signature generation, release-please version updates, firmware
artifacts, packaging and website source links together. Preserve independent
runtime lockfiles, public schema/documentation URLs, release asset names and GPL
boundaries. Keep mechanical moves separate from behavioral or dependency changes;
validate all four firmware builds and both desktop platforms before merging.
