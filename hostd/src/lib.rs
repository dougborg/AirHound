//! MIT-licensed desktop source validation and offline import; no driver linkage.
#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
pub mod external_identity;
pub mod external_wifi;

pub mod flipper_bridge;
pub mod flipper_rpc_session;
pub mod flipper_rpc_transport;
pub mod marauder;
pub mod marauder_stream;

pub mod session;

pub mod build_info;
pub mod discovery;
pub mod managed_helpers;
pub mod profiles;
pub mod serial_lease;
pub mod service;

pub mod device_stream;
pub mod storage;
