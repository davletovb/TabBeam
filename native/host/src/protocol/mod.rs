//! Protocol v1: request validation, routing, and event emission.
//!
//! The normative contract is `docs/protocol/v1.md`, with the error and
//! capability vocabulary in `docs/protocol/errors-and-capabilities-v1.md`.

pub mod events;
pub mod json;
pub mod request;
pub mod router;

/// The protocol version this host speaks.
pub const PROTOCOL_VERSION: i64 = 1;
