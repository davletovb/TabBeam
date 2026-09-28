# Runtime core

`runtime-core` is the reusable foundation of the shared provider runtime described by ADR-0002. During Stage 1 it still lives in Pervue, but browser-specific policy stays outside it.

Current public modules are `process`, `stream`, `discovery`, `exchange`, and `protocol`. The latter two are transitional during LIB-06 and will be replaced by neutral turn, failure, usage, session-policy, and timeout types.

Chrome Native Messaging framing lives in `native/host/src/framing.rs`. Browser wire envelopes, origin checks, request IDs, browser-context policy, conversation/session mappings, diagnostics, and user-facing failure wording remain Pervue concerns.

There is no C ABI and no Node addon. Rust consumers use the runtime in-process; a future non-Rust consumer gets a versioned stdio sidecar around the service API.

Run the crate independently with `cargo test -p runtime-core`.
