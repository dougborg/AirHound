//! AirHound library — pure-logic modules for WiFi/BLE surveillance detection.
//!
//! This crate contains the parsing, filtering, and protocol logic that can be
//! tested on the host without ESP hardware dependencies. Hardware-specific code
//! (embassy tasks, BLE GATT server, WiFi sniffer callbacks) lives in the
//! firmware binary (`main.rs`).

#![cfg_attr(not(test), no_std)]

pub mod board;
pub mod comm;
pub mod defaults;
pub mod filter;
pub mod protocol;
pub mod scanner;
