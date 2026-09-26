//! Native primitives shared by provider adapters and the browser host.
//!
//! This is a Rust source API, not a C ABI. The executable retains product
//! policy, browser request validation, and Native Messaging event encoding.
//!
//! Process handles own and reap their children, including on drop. Streams
//! borrow no external buffers; all returned events own their data. Framing
//! rejects oversized payloads before allocation.

pub mod discovery;
pub mod exchange;
pub mod framing;
pub mod process;
pub mod protocol;
pub mod stream;
