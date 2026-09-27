//! Pervue native host.
//!
//! The host speaks Chrome Native Messaging on stdin/stdout: `pervue_core::framing` reads
//! and writes bounded length-prefixed frames, [`protocol`] validates protocol-v1
//! requests and emits events, and [`host`] runs the request loop, recording its
//! lifecycle through [`diagnostics`]. [`limits`]
//! holds every bound on browser input, and [`manifest`] the caller-identity
//! checks and the Native Messaging registration. `pervue_core::process` starts
//! and supervises provider processes, and `pervue_core::stream` turns their
//! output into lines. Shared primitives are imported directly from the core.

pub mod conversation;
pub mod diagnostics;
pub mod host;
pub mod limits;
pub mod manifest;
pub mod protocol;
pub mod providers;
pub mod search;

/// Host version reported by `--version` and in the `host.ready` event.
pub const HOST_VERSION: &str = env!("CARGO_PKG_VERSION");
