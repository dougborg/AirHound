# AirHound build recipes — install just: cargo install just

xiao_target := "xtensa-esp32s3-none-elf"
m5_target   := "xtensa-esp32-none-elf"
dev_image   := "airhound-dev"
xiao_image  := "espressif/idf-rust:esp32s3_latest"
m5_image    := "espressif/idf-rust:esp32_latest"

# build-std is needed for xtensa targets (no pre-built sysroot).
# Kept here instead of .cargo/config.toml so `cargo test` works on host.
_build_std := "-Z build-std=core,alloc"

# Serial device for flashing (override: just device=/dev/ttyACM0 flash-xiao)
device := env_var_or_default("DEVICE", "/dev/ttyUSB0")

_volumes := "-v " + justfile_directory() + ":/home/esp/workspace -v airhound-cargo-registry:/home/esp/.cargo/registry -v airhound-cargo-git:/home/esp/.cargo/git"
_docker  := "docker run --rm " + _volumes + " -w /home/esp/workspace"
_esp_env := "bash -c '. /home/esp/export-esp.sh &&"

# List available recipes
[private]
default:
    @just --list

# ── Host ──────────────────────────────────────────────────

# Build firmware for both boards
[group('host')]
build: build-xiao build-m5stickc

# Build firmware for XIAO ESP32-S3
[group('host')]
build-xiao:
    cargo build --no-default-features --features xiao --release --target {{ xiao_target }} {{ _build_std }}

# Build firmware for M5StickC Plus2
[group('host')]
build-m5stickc:
    cargo build --no-default-features --features m5stickc --release --target {{ m5_target }} {{ _build_std }}

# Type-check both boards
[group('host')]
check: check-xiao check-m5stickc

# Type-check XIAO ESP32-S3
[group('host')]
check-xiao:
    cargo check --no-default-features --features xiao --release --target {{ xiao_target }} {{ _build_std }}

# Type-check M5StickC Plus2
[group('host')]
check-m5stickc:
    cargo check --no-default-features --features m5stickc --release --target {{ m5_target }} {{ _build_std }}

# Run library unit tests on host
[group('host')]
test:
    cargo test --lib --no-default-features

# Flash XIAO ESP32-S3 and open serial monitor
[group('host')]
flash-xiao:
    cargo run --no-default-features --features xiao --release --target {{ xiao_target }} {{ _build_std }}

# Flash M5StickC Plus2 and open serial monitor
[group('host')]
flash-m5stickc:
    cargo run --no-default-features --features m5stickc --release --target {{ m5_target }} {{ _build_std }}

# Flash pre-built XIAO binary (espflash auto-detects port; override: just device=/dev/cu.xxx flash-xiao-host)
[group('host')]
flash-xiao-host:
    espflash flash --monitor --chip esp32s3 target/{{ xiao_target }}/release/airhound

# Flash pre-built M5StickC binary (espflash auto-detects port; override: just device=/dev/cu.xxx flash-m5stickc-host)
[group('host')]
flash-m5stickc-host:
    espflash flash --monitor --chip esp32 target/{{ m5_target }}/release/airhound

# Remove build artifacts
[group('host')]
clean:
    cargo clean

# Configure git hooks for this repository
[group('host')]
setup-hooks:
    git config core.hooksPath .githooks
    chmod +x .githooks/*
    @echo "Git hooks configured."

# ── Schemas ──────────────────────────────────────────────

# Check JSON files are well-formed and consistently formatted (2-space indent)
[group('schemas')]
check-json:
    #!/usr/bin/env bash
    set -euo pipefail
    fail=0
    while IFS= read -r -d '' f; do
        if ! python3 -c "
    import json, sys
    with open('$f') as fh:
        obj = json.load(fh)
    formatted = json.dumps(obj, indent=2, ensure_ascii=False) + '\n'
    with open('$f') as fh:
        actual = fh.read()
    if actual != formatted:
        print('$f: not formatted (run just fmt-json)', file=sys.stderr)
        sys.exit(1)
    "; then
            fail=1
        fi
    done < <(find schemas -name '*.json' -print0 2>/dev/null)
    if [ "$fail" -eq 0 ]; then echo "All JSON files formatted."; fi
    exit $fail

# Auto-format all JSON files in schemas/ (2-space indent)
[group('schemas')]
fmt-json:
    #!/usr/bin/env bash
    set -euo pipefail
    while IFS= read -r -d '' f; do
        python3 -c "
    import json
    with open('$f') as fh:
        obj = json.load(fh)
    with open('$f', 'w') as fh:
        json.dump(obj, fh, indent=2, ensure_ascii=False)
        fh.write('\n')
    "
        echo "Formatted $f"
    done < <(find schemas -name '*.json' -print0 2>/dev/null)

# Validate JSON Schema files are valid draft 2020-12 schemas (requires: pip install check-jsonschema)
[group('schemas')]
check-schemas:
    #!/usr/bin/env bash
    set -euo pipefail
    if ! command -v check-jsonschema &>/dev/null; then
        echo "ERROR: check-jsonschema not found. Install with: pip install check-jsonschema"
        exit 1
    fi
    fail=0
    shopt -s nullglob
    files=(schemas/*.schema.json)
    if [ ${#files[@]} -eq 0 ]; then
        echo "No schema files found."
        exit 0
    fi
    for f in "${files[@]}"; do
        echo "Validating schema: $f"
        if ! check-jsonschema --check-metaschema "$f"; then
            fail=1
        fi
    done
    exit $fail

# Validate example files against their schemas (requires: pip install check-jsonschema)
[group('schemas')]
check-examples:
    #!/usr/bin/env bash
    set -euo pipefail
    if ! command -v check-jsonschema &>/dev/null; then
        echo "ERROR: check-jsonschema not found. Install with: pip install check-jsonschema"
        exit 1
    fi
    fail=0
    for f in schemas/examples/*.json; do
        [ -f "$f" ] || continue
        # Extract schema reference from $schema field
        schema=$(python3 -c "import json; print(json.load(open('$f')).get('\$schema',''))")
        if [ -z "$schema" ]; then
            echo "SKIP $f (no \$schema field)"
            continue
        fi
        # Convert raw.githubusercontent URL to local path
        local_schema=$(echo "$schema" | sed 's|https://raw.githubusercontent.com/dougborg/AirHound/main/||')
        if [ -f "$local_schema" ]; then
            echo "Validating $f against $local_schema"
            if ! check-jsonschema --schemafile "$local_schema" "$f"; then
                fail=1
            fi
        else
            echo "SKIP $f (schema not found locally: $local_schema)"
        fi
    done
    exit $fail

# ── Docker ────────────────────────────────────────────────

# Build the dev container image (interactive use / Codespaces)
[group('docker')]
docker-image:
    docker build -t {{ dev_image }} .devcontainer/

# Build firmware for both boards (in container)
[group('docker')]
docker-build: docker-build-xiao docker-build-m5stickc

# Build XIAO firmware (in container, chip-specific image)
[group('docker')]
docker-build-xiao:
    {{ _docker }} {{ xiao_image }} {{ _esp_env }} cargo build --no-default-features --features xiao --release --target {{ xiao_target }} {{ _build_std }}'

# Build M5StickC firmware (in container, chip-specific image)
[group('docker')]
docker-build-m5stickc:
    {{ _docker }} {{ m5_image }} {{ _esp_env }} cargo build --no-default-features --features m5stickc --release --target {{ m5_target }} {{ _build_std }}'

# Type-check both boards (in container)
[group('docker')]
docker-check: docker-check-xiao docker-check-m5stickc

# Type-check XIAO (in container, chip-specific image)
[group('docker')]
docker-check-xiao:
    {{ _docker }} {{ xiao_image }} {{ _esp_env }} cargo check --no-default-features --features xiao --release --target {{ xiao_target }} {{ _build_std }}'

# Type-check M5StickC (in container, chip-specific image)
[group('docker')]
docker-check-m5stickc:
    {{ _docker }} {{ m5_image }} {{ _esp_env }} cargo check --no-default-features --features m5stickc --release --target {{ m5_target }} {{ _build_std }}'

# Run library unit tests (in container)
[group('docker')]
docker-test:
    {{ _docker }} {{ xiao_image }} {{ _esp_env }} cargo test --lib --no-default-features'

# Flash XIAO via container (Linux only — requires USB passthrough)
[group('docker')]
docker-flash-xiao:
    {{ _docker }} --device={{ device }} {{ xiao_image }} {{ _esp_env }} cargo run --no-default-features --features xiao --release --target {{ xiao_target }} {{ _build_std }}'

# Flash M5StickC via container (Linux only — requires USB passthrough)
[group('docker')]
docker-flash-m5stickc:
    {{ _docker }} --device={{ device }} {{ m5_image }} {{ _esp_env }} cargo run --no-default-features --features m5stickc --release --target {{ m5_target }} {{ _build_std }}'

# Remove build artifacts (uses either chip image)
[group('docker')]
docker-clean:
    {{ _docker }} {{ xiao_image }} {{ _esp_env }} cargo clean'

# Open an interactive shell in the dev container (full toolchain)
[group('docker')]
docker-shell: docker-image
    {{ _docker }} -it {{ dev_image }}
