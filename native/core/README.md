# Reusable native core

`pervue-core` is a Rust library crate consumed by `pervue-host`. It contains
primitives used by both Codex and Claude adapters. It does not depend on the
host, extension, browser APIs, provider CLI formats, or a running companion.

| Public module | Owns | Kept in `pervue-host` |
|---|---|---|
| `process` | Absolute-path spawn, isolated child environment, piped I/O, bounded output queue, termination and reap | Choice of executable, args, environment allowlist, timeout values, provider parsing |
| `framing` | Native Messaging byte framing and a fixed 1 MiB payload limit | Request JSON validation and event serialization |
| `stream` | Public `LineSplitter` for bounded UTF-8 lines, `LineStream` for process output and final/error/stopped states, stderr tail, text chunks | Interpretation of provider lines and messages |
| `protocol` | Normalized protocol-v1 error, capability, and status value types | Browser wire envelope, request IDs, product-specific validation |
| `exchange` | Protocol-v1 `Update`, deadline-driven `Exchange`, `Timeouts`, `Scripted` | `SendRequest`, provider registry and `Provider` trait, conversation and session policy |
| `discovery` | Absolute directory search and platform executable detection | The `PERVUE_PROVIDER_PATH` override name and any provider-specific path choices |

Only those modules are public. `LineSplitter` and its `push`, `finish`, and
`pending_len` operations are part of the Rust source API: CRLF is removed,
lone CR remains data, invalid UTF-8 and over-limit lines fail, and unfinished
lines stay bounded across chunk boundaries. Process-tree control, buffered I/O
internals, and platform detection remain private within the modules.
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
Native Messaging framing is an explicit product boundary shared with the host;
its single production consumer is the host, and its standalone fuzz target
checks the same wire format. `Update` and normalized errors intentionally use
Pervue's protocol-v1 vocabulary, so a change to those event or error meanings
is a Rust core API change as well as a browser protocol change. Browser request
data structures and provider-specific session details remain in the host.

Run the library independently with `cargo test -p pervue-core` (including
`tests/public_api.rs`). The fuzz workspace targets `frame_reader` and
`stream_lines` directly against this crate. Workspace consumers import the
shared modules directly from `pervue-core`; the unpublished host crate does
not promise source compatibility for its former glob reexport paths. The host
does retain an explicit `limits::MAX_FRAME_SIZE` binding at its browser trust
boundary and reexports the normalized event vocabulary it writes on the wire.
