//! AirHound library — portable surveillance detection engine.
//!
//! One of three portable layers in the AirHound toolkit (alongside the
//! [signature schema](../schemas/signatures.v1.schema.json) and
//! [companion event protocol schema](../schemas/device-message.v1.schema.json)).
//! This crate contains all scanning, filtering, and protocol logic with no
//! platform dependencies, testable on any host with `cargo test`. Platform
//! binaries (ESP32 firmware, Linux daemon, Kismet companion) are thin consumers
//! that provide radio access and output sinks.
//!
//! The library is organized in two code layers:
//! - **Layer 1** (implemented): `scanner`, `filter`, `defaults`, `protocol`,
//!   `comm`, `board` — `no_std`, no allocator, no external dependencies.
//! - **Layer 2** (planned): `gps`, `tracker`, `channel`, `export`, `wids` —
//!   behind feature gates, progressively requiring `alloc` or `std`.

#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![cfg_attr(not(test), no_std)]

pub mod ble_data;
#[cfg(any(feature = "firmware", test))]
pub mod ble_scan_control;
pub mod board;
pub mod buttons;
pub mod comm;
pub mod defaults;
pub mod filter;
#[cfg(any(feature = "firmware", test))]
pub mod health_output;
pub mod mac_index;
pub mod observations;
pub mod odid;
pub mod protocol;
pub mod proximity;
pub mod rules;
pub mod scanner;
pub mod serial_output;
pub mod watchlist;

pub mod evidence;

pub mod wifi_frequency;
