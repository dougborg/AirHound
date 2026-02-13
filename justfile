# AirHound build recipes — install just: cargo install just

xiao_target := "xtensa-esp32s3-none-elf"
m5_target   := "xtensa-esp32-none-elf"
image       := "airhound-dev"

# Serial device for flashing (override: just device=/dev/ttyACM0 flash-xiao)
device := env_var_or_default("DEVICE", "/dev/ttyUSB0")

_volumes := "-v " + justfile_directory() + ":/home/esp/workspace -v airhound-cargo-registry:/home/esp/.cargo/registry -v airhound-cargo-git:/home/esp/.cargo/git"
_docker  := "docker run --rm " + _volumes + " -w /home/esp/workspace"

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
    cargo build --no-default-features --features xiao --release --target {{ xiao_target }}

# Build firmware for M5StickC Plus2
[group('host')]
build-m5stickc:
    cargo build --no-default-features --features m5stickc --release --target {{ m5_target }}

# Type-check both boards
[group('host')]
check: check-xiao check-m5stickc

# Type-check XIAO ESP32-S3
[group('host')]
check-xiao:
    cargo check --no-default-features --features xiao --release --target {{ xiao_target }}

# Type-check M5StickC Plus2
[group('host')]
check-m5stickc:
    cargo check --no-default-features --features m5stickc --release --target {{ m5_target }}

# Flash XIAO ESP32-S3 and open serial monitor
[group('host')]
flash-xiao:
    cargo run --no-default-features --features xiao --release --target {{ xiao_target }}

# Flash M5StickC Plus2 and open serial monitor
[group('host')]
flash-m5stickc:
    cargo run --no-default-features --features m5stickc --release --target {{ m5_target }}

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

# ── Docker ────────────────────────────────────────────────

# Build the dev container image
[group('docker')]
docker-image:
    docker build -t {{ image }} .devcontainer/

# Build firmware for both boards (in container)
[group('docker')]
docker-build: docker-build-xiao docker-build-m5stickc

# Build XIAO firmware (in container)
[group('docker')]
docker-build-xiao: docker-image
    {{ _docker }} {{ image }} cargo build --no-default-features --features xiao --release --target {{ xiao_target }}

# Build M5StickC firmware (in container)
[group('docker')]
docker-build-m5stickc: docker-image
    {{ _docker }} {{ image }} cargo build --no-default-features --features m5stickc --release --target {{ m5_target }}

# Type-check both boards (in container)
[group('docker')]
docker-check: docker-check-xiao docker-check-m5stickc

# Type-check XIAO (in container)
[group('docker')]
docker-check-xiao: docker-image
    {{ _docker }} {{ image }} cargo check --no-default-features --features xiao --release --target {{ xiao_target }}

# Type-check M5StickC (in container)
[group('docker')]
docker-check-m5stickc: docker-image
    {{ _docker }} {{ image }} cargo check --no-default-features --features m5stickc --release --target {{ m5_target }}

# Flash XIAO via container (Linux only — requires USB passthrough)
[group('docker')]
docker-flash-xiao: docker-image
    {{ _docker }} --device={{ device }} {{ image }} cargo run --no-default-features --features xiao --release --target {{ xiao_target }}

# Flash M5StickC via container (Linux only — requires USB passthrough)
[group('docker')]
docker-flash-m5stickc: docker-image
    {{ _docker }} --device={{ device }} {{ image }} cargo run --no-default-features --features m5stickc --release --target {{ m5_target }}

# Remove build artifacts (in container)
[group('docker')]
docker-clean: docker-image
    {{ _docker }} {{ image }} cargo clean

# Open an interactive shell in the dev container
[group('docker')]
docker-shell: docker-image
    {{ _docker }} -it {{ image }}
