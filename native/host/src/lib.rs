//! Pervue native host.
//!
//! The host speaks Chrome Native Messaging on stdin/stdout: [`framing`] reads
//! and writes bounded length-prefixed frames, [`protocol`] validates protocol-v1
//! requests and emits events, and [`host`] runs the request loop.

pub mod framing;
pub mod host;
pub mod protocol;

/// Host version reported by `--version` and in the `host.ready` event.
pub const HOST_VERSION: &str = env!("CARGO_PKG_VERSION");
