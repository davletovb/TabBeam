//! Pervue native host.
//!
//! The host owns Chrome Native Messaging framing and protocol-v1 encoding.
//! It validates browser requests, applies product policy, and routes provider
//! work through the reusable `runtime-core` primitives. Native Messaging is
//! deliberately host-owned under ADR-0002; it is not part of the shared
//! provider runtime boundary.

pub mod conversation;
pub mod diagnostics;
pub mod framing;
pub mod host;
pub mod limits;
pub mod manifest;
pub mod protocol;
pub mod providers;
pub mod search;

/// Host version reported by `--version` and in the `host.ready` event.
pub const HOST_VERSION: &str = env!("CARGO_PKG_VERSION");
