# Pervue Native Host

This directory contains the native Rust companion/host.

The current host foundation includes bounded Chrome Native Messaging framing plus protocol-v1 JSON validation and routing.

## Layout

```text
native/
├── Cargo.toml       Cargo workspace: shared version, Rust 1.85+, `unsafe` forbidden
├── host/            pervue-host: the Native Messaging host (binary + library)
│   └── src/
│       ├── diagnostics.rs  structured lifecycle diagnostics (JSON lines on stderr)
│       ├── framing.rs   bounded length-prefixed frame reader/writer
│       ├── limits.rs    every bound on browser input (frame size, nesting, request IDs)
│       ├── manifest.rs  caller-origin checks and the Native Messaging manifest
│       ├── protocol/    strict request validation, routing, and event emission
│       ├── host.rs      request loop and fake-provider scaffold routes
│       └── main.rs      command-line entry point
├── test_provider/   pervue-fake-provider: deterministic fake provider for tests
└── fuzz/            cargo-fuzz targets and seed-corpus generators
```

## Requirements

- Rust 1.85 or newer (install with [rustup](https://rustup.rs))

## Development build

```bash
cd native
cargo build
cargo test --workspace
```

`cargo build` builds only the host (`target/debug/pervue-host`); the fake provider is a test fixture, so `cargo test --workspace` builds and tests it.

All project crates forbid `unsafe` code. The host crate's `clippy.toml` also bans `std::process::Command::new`, so the host can't start a process outside the future provider process manager (NAT-04; see `docs/security/trust-boundaries.md`). CI treats compiler and Clippy warnings as errors and checks formatting:

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
```

## Native Messaging framing

Pervue uses Chrome Native Messaging framing:

- 4-byte unsigned payload length in the platform's native byte order;
- followed by exactly that many payload bytes;
- zero-length payloads are valid;
- inbound and outbound frames are capped at `MAX_FRAME_SIZE` (1 MiB) from `host/src/limits.rs`, which tests keep equal to `docs/protocol/native-messaging-v1.json`, the copy the extension is tested against;
- oversized lengths are rejected before allocation;
- EOF before any prefix byte is clean end-of-stream;
- partial prefix/payload EOF is a truncated-frame error;
- short reads and writes are retried until the frame is complete or the stream fails.

NAT-02 validates and transports frames. NAT-03 validates protocol-v1 JSON envelopes/payloads, emits normalized protocol failures, and routes known methods. Provider process execution remains deferred to NAT-04/NAT-05.

A framing failure ends the host with a deterministic exit status, and the host's last diagnostics record (`host.stopped`, see [Diagnostics](#diagnostics)) names the reason and the exit status.

| Exit status | Meaning |
|---|---|
| 0 | Clean end of stream, or `--version` |
| 2 | I/O error while reading a request, writing an event, or printing the manifest |
| 3 | The stream ended inside a frame |
| 4 | A frame length exceeded the 1 MiB cap |
| 5 | A frame buffer could not be allocated |
| 64 | Missing or unexpected command-line arguments, including a caller origin or extension ID that isn't exact |

## Host behavior at this milestone

```bash
./target/debug/pervue-host --version
```

prints the host version and exits with status 0.

Chrome launches Native Messaging hosts with the caller origin as the first positional argument. Pervue accepts the production launch shape:

```bash
./target/debug/pervue-host chrome-extension://<extension-id>/
```

On Windows, Chrome also passes the calling window's handle as a second argument, `--parent-window=<decimal handle>` (0 when the caller is a service worker). The host accepts that shape and ignores the handle.

The caller origin is required and must be exactly `chrome-extension://<extension-id>/`, where the ID is 32 characters from `a` to `p`. With no arguments, any other origin, unknown flags, or extra positional arguments, the host exits with usage status 64 before reading a frame. To drive the host by hand, pass a well-formed origin such as `chrome-extension://aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa/`.

Normal execution first emits exactly one `host.ready` event, then reads bounded Native Messaging frames until EOF. Each frame must contain exactly one valid protocol-v1 JSON request object.

Malformed JSON, invalid envelopes/payloads, unsupported versions, and unknown methods produce normalized `response.failed` events. Malformed requests do not terminate an otherwise usable host stream.

No object in a request may repeat a member name, including members the host does not interpret; names are compared after decoding escapes, so `"a"` and `"\u0061"` collide. A repeat in the envelope is `INVALID_ENVELOPE` and anywhere inside a method payload is `INVALID_PAYLOAD`; syntax errors still take precedence.

Requests are validated by a strict pull reader (`host/src/protocol/json.rs`) rather than a serde deserializer, because protocol v1 depends on details serde hides: duplicate member names must be rejected, member order decides whether a request ID was recovered before a syntax error, request IDs are echoed byte-for-byte, and depth overflow is classified by where it happens. Outbound event payloads are serialized with serde_json.

For NAT-03, `provider_id: "fake"` is a deliberately local scaffold route that emits a deterministic conversation event sequence. Real provider discovery and execution begin in NAT-04/NAT-05. Rust's standard streams pass bytes through unchanged, including Windows pipes, so no binary-mode switch is needed before framing.

## Registering the host

Chrome starts the host only for the extensions its Native Messaging manifest lists in `allowed_origins`. The host prints that manifest for exact extension IDs:

```bash
./target/debug/pervue-host --print-manifest <extension-id> [<extension-id>...]
```

The output names `com.pervue.host`, points `path` at the absolute path the host was run from, and lists one `chrome-extension://<id>/` origin per ID. Anything that isn't a 32-character `a`–`p` ID, such as a wildcard or a full origin, is refused with status 64. The path isn't canonicalized: on macOS a symlink or `..` is kept as typed, while Linux reports the resolved path. That is deliberate, because a stable symlink can be the right path to register, where its versioned target would break on upgrade. Installers should run the host from the path they want registered (PKG-01). `extension/README.md` shows where to save the manifest for development; installers register it later (PKG-01).

## Diagnostics

While it serves a session, the host writes structured diagnostics to stderr, one JSON object per line (OBS-01). stdout carries only Native Messaging frames, so diagnostics can't corrupt them. If writing to stderr fails, for example because it's closed, the host ignores the failure and carries on. A write that blocks isn't skipped, though: a stderr pipe that nobody reads would eventually stall the host. Chrome passes a host's stderr through to its own, so that only happens when Chrome's own stderr is such a pipe, and starting Chrome with `--enable-logging=stderr` shows the records.

```text
{"ts":"2026-09-25T01:21:49.903Z","event":"host.started","host_version":"0.1.0-dev","pid":1764}
{"ts":"2026-09-25T01:21:49.903Z","event":"request.completed","request_id":"req_a","method":"conversation.send","provider_id":"fake","conversation_id":"conv_1","duration_ms":0}
{"ts":"2026-09-25T01:21:49.903Z","event":"request.failed","request_id":"req_b","method":"provider.status","provider_id":"codex","duration_ms":0,"error":{"code":"PROVIDER_NOT_FOUND","reason":"PROVIDER_NOT_INSTALLED"}}
{"ts":"2026-09-25T01:21:49.903Z","event":"request.rejected","error":{"code":"INVALID_REQUEST","reason":"MALFORMED_MESSAGE"}}
{"ts":"2026-09-25T01:21:49.903Z","event":"host.stopped","duration_ms":0,"reason":"end_of_input","exit_code":0,"requests":2,"rejected":1}
```

Every record has `ts` (RFC 3339 UTC, with milliseconds) and `event`. A field that doesn't apply is left out. Every request the host reads gets exactly one `request.*` record, including a request it was answering when it stopped. A request's `conversation_id` is the conversation it continued or, when it started one, the ID its `conversation.created` event announced, so the first request of a conversation correlates with the ones that follow.

| Event | Written when | Fields |
|---|---|---|
| `host.started` | Before `host.ready` | `host_version`, `pid` |
| `request.completed` | A handler finished the request successfully | `request_id`, `method`, `provider_id`, `conversation_id`, `target_request_id` (for `request.cancel`), `duration_ms` |
| `request.failed` | A handler ended the request with `response.failed` | The same fields, plus `error` (`code` and `reason`) |
| `request.aborted` | The host stopped while answering the request, such as when stdout closed, so the extension got no terminal event | The same fields as `request.completed`, plus `reason` (why the host stopped) |
| `request.rejected` | The request failed validation and never reached a handler | `request_id` if one was recovered, and `error` (`INVALID_REQUEST` and its reason) |
| `host.stopped` | The host is about to exit | `reason` (`end_of_input`, `io_error`, `frame_truncated`, `frame_too_large` or `allocation_failed`), `exit_code`, `duration_ms` (uptime), `requests` (requests that passed validation and reached a handler) and `rejected` (requests that failed validation) |

A record never contains request content: no prompt text, page context, other payload members, raw frame bytes, or error messages. Every identifier it records is cut to 128 characters, and JSON escaping keeps each record on one line whatever they contain. Redacting provider output and credentials comes with real providers (SEC-02).

Command-line errors, such as a usage error or an invalid `--print-manifest` ID, are plain text on stderr, because no session is running.

## Fuzz targets

With nightly Rust and [cargo-fuzz](https://github.com/rust-fuzz/cargo-fuzz) (`cargo install cargo-fuzz`):

```bash
cd native
python3 fuzz/create_corpus.py fuzz/corpus/frame_reader
python3 fuzz/create_protocol_corpus.py fuzz/corpus/protocol --fixtures ../docs/protocol/fixtures/v1-golden.json
cargo +nightly fuzz run frame_reader fuzz/corpus/frame_reader -- -runs=1000 -max_len="$(python3 fuzz/max_len.py frame_reader)"
cargo +nightly fuzz run protocol fuzz/corpus/protocol -- -runs=2000 -max_len="$(python3 fuzz/max_len.py protocol)"
```

cargo-fuzz builds the targets with AddressSanitizer. `frame_reader` reads frames from memory until the first non-frame result. `protocol` runs each input through the whole host as one request frame and fails if any emitted frame is not a JSON object.

The harnesses read directly from memory rather than creating a temporary file per input. The generated corpora seed empty, small valid, exact-maximum, oversized-prefix, truncated-prefix, and truncated-payload frames, plus requests derived from the golden protocol fixtures, so smoke runs start from structurally meaningful inputs.
