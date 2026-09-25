# Pervue Native Host

This directory contains the native Rust companion/host.

The current host foundation includes bounded Chrome Native Messaging framing plus protocol-v1 JSON validation and routing.

## Layout

```text
native/
├── Cargo.toml       Cargo workspace: shared version, Rust 1.85+, `unsafe` forbidden
├── host/            pervue-host: the Native Messaging host (binary + library)
│   └── src/
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

At this milestone framing failures are exposed as deterministic exit statuses. Structured stderr/lifecycle diagnostics are intentionally deferred to **OBS-01** so NAT-02 does not create an ad-hoc diagnostics format that later observability work must replace.

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

The output names `com.pervue.host`, points `path` at the absolute path of the binary that printed it, and lists one `chrome-extension://<id>/` origin per ID. Anything that isn't a 32-character `a`–`p` ID, such as a wildcard or a full origin, is refused with status 64. `extension/README.md` shows where to save the manifest for development; installers register it later (PKG-01).

## Fuzz targets

With nightly Rust and [cargo-fuzz](https://github.com/rust-fuzz/cargo-fuzz) (`cargo install cargo-fuzz`):

```bash
cd native
python3 fuzz/create_corpus.py fuzz/corpus/frame_reader
python3 fuzz/create_protocol_corpus.py fuzz/corpus/protocol --fixtures ../docs/protocol/fixtures/v1-golden.json
cargo +nightly fuzz run frame_reader fuzz/corpus/frame_reader -- -runs=1000 -max_len=1048580
cargo +nightly fuzz run protocol fuzz/corpus/protocol -- -runs=2000 -max_len=1048576
```

cargo-fuzz builds the targets with AddressSanitizer. `frame_reader` reads frames from memory until the first non-frame result. `protocol` runs each input through the whole host as one request frame and fails if any emitted frame is not a JSON object.

The harnesses read directly from memory rather than creating a temporary file per input. The generated corpora seed empty, small valid, exact-maximum, oversized-prefix, truncated-prefix, and truncated-payload frames, plus requests derived from the golden protocol fixtures, so smoke runs start from structurally meaningful inputs.
