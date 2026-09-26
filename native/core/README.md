# Reusable native core

`pervue-core` is a Rust library crate consumed by `pervue-host`. It contains
primitives used by both Codex and Claude adapters. It does not depend on the
host, extension, browser APIs, provider CLI formats, or a running companion.

| Public module | Owns | Kept in `pervue-host` |
|---|---|---|
| `process` | Absolute-path spawn, isolated child environment, piped I/O, bounded output queue, termination and reap | Choice of executable, args, environment allowlist, timeout values, provider parsing |
| `framing` | Native Messaging byte framing and a fixed 1 MiB payload limit | Request JSON validation and event serialization |
| `stream` | Bounded UTF-8 lines, stderr tail, text chunks, final/error/stopped output | Interpretation of provider lines and messages |
| `protocol` | Normalized error, capability, and status value types | Browser wire envelope, request IDs, product-specific validation |
| `exchange` | `Update`, deadline-driven `Exchange`, `Timeouts`, `Scripted` | `SendRequest`, provider registry and `Provider` trait, conversation and session policy |
| `discovery` | Absolute directory search and platform executable detection | The `PERVUE_PROVIDER_PATH` override name and any provider-specific path choices |

Only those modules are public. Process-tree control, buffered I/O internals,
platform detection, and stream state machines remain private within them.
Pervue's diagnostics records, request ID rules, credential allowlist, browser
context handling, and provider session maps stay in the host: those are product
policy, rather than a reusable logging or configuration API. `discovery` uses
platform-specific internals with a portable search API; Windows process-tree
termination still requires a Job Object and is not promised by the current API.

## Ownership, errors, and cleanup

`Process::spawn(&ProcessSpec)` borrows the specification only during spawn;
the returned handle owns the child and its pipes. `Process::write` copies the
bytes into the writer queue. `Process::next_event` returns owned byte vectors;
`LineStream` owns a `Process` and returns owned lines. A dropped process or
stream kills and reaps its child. `terminate` and `kill` are idempotent after
completion. Callers must continue polling exchanges up to their chosen
deadline and explicitly cancel work that exceeds it.

Framing reads allocate only after checking the declared length. A clean EOF
returns `Ok(None)`; truncated frames, oversize payloads, I/O, and allocation
failures return `FrameError`. The writer rejects oversize payloads without
writing; an I/O or flush failure may occur after a partial frame was written,
so the host should close that connection. Streams return exactly one terminal
`Final`, `Error`, or `Stopped` state. Provider adapters map process and stream
failures to normalized `ErrorBody`; browser request validation and user-facing
messages belong to the host. Returned data and errors are Rust-owned values,
with no manual free or borrowed error pointers.

## Compatibility and extraction

There is **no C ABI or shared-library binary interface** in this release.
The crate is statically linked into the host, has no `extern "C"` functions,
and is not `cdylib`. Rust source API changes are reviewed with all workspace
consumers and a corresponding host release. Packaged hosts communicate with
the extension through the versioned Native Messaging protocol, whose
compatibility rules are in `docs/protocol/v1.md`; crate version equality does
not imply protocol compatibility. A stable external ABI would require a
separate versioned C façade with opaque handles, explicit allocations and
destructors, and a cross-version dynamic-link test before advertising one.

The extraction rule is to add a public primitive only after two real
consumers or two real providers demonstrate identical semantics. Codex and
Claude establish the process, stream, and status/exchange contracts here.
The framing code has an independent consumer in the host and its standalone
fuzz target. Browser-specific data structures and provider-specific session
details remain opaque to the core.

Run the library independently with `cargo test -p pervue-core` (including
`tests/public_api.rs`). The fuzz workspace targets `frame_reader` and
`stream_lines` directly against this crate. The host still reexports prior
module paths for existing workspace consumers; these paths do not transfer
ownership of the primitives back to the host.
