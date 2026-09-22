# Pervue Native Host

This directory contains the native C companion/host.

The current host foundation includes bounded Chrome Native Messaging framing plus protocol-v1 JSON validation and routing.

## Requirements

- CMake 3.21+
- a C11 compiler

## Development build

```bash
cd native
cmake --preset dev
cmake --build --preset dev
ctest --preset dev
```

The project treats compiler warnings as errors for Pervue C code.

## Sanitizer build

On GCC/Clang platforms with AddressSanitizer and UndefinedBehaviorSanitizer support:

```bash
cd native
cmake --preset dev-sanitize
cmake --build --preset dev-sanitize
ctest --preset dev-sanitize
```

The sanitizer preset performs a compile-and-link capability probe and fails during configuration when the active compiler/runtime cannot actually provide ASan+UBSan.

## Native Messaging framing

Pervue uses Chrome Native Messaging framing:

- 4-byte unsigned payload length in the platform's native byte order;
- followed by exactly that many payload bytes;
- zero-length payloads are valid;
- inbound and outbound frames are capped at `PERVUE_NATIVE_MAX_FRAME_SIZE` (currently 1 MiB);
- oversized lengths are rejected before allocation;
- EOF before any prefix byte is clean end-of-stream;
- partial prefix/payload EOF is a truncated-frame error;
- short reads and writes are retried until the frame is complete or the stream fails.

NAT-02 validates and transports frames. NAT-03 validates protocol-v1 JSON envelopes/payloads, emits normalized protocol failures, and routes known methods. Provider process execution remains deferred to NAT-04/NAT-05.

At this milestone framing failures are exposed as deterministic host return/exit statuses. Structured stderr/lifecycle diagnostics are intentionally deferred to **OBS-01** so NAT-02 does not create an ad-hoc diagnostics format that later observability work must replace.

## Host behavior at this milestone

```bash
./build/dev/pervue-host --version
```

prints the host version and exits with status 0.

Chrome launches Native Messaging hosts with the caller origin as the first positional argument. Pervue accepts the production launch shape:

```bash
./build/dev/pervue-host chrome-extension://<extension-id>/
```

The caller-origin value is not yet used for application routing; host registration/allowed-origin policy and packaged identity checks are hardened by later security/packaging work. Unknown flags or unrelated positional arguments are rejected with usage status 64.

Normal execution first emits exactly one `host.ready` event, then reads bounded Native Messaging frames until EOF. Each frame must contain exactly one valid protocol-v1 JSON request object.

Malformed JSON, invalid envelopes/payloads, unsupported versions, and unknown methods produce normalized `response.failed` events. Malformed requests do not terminate an otherwise usable host stream.

For NAT-03, `provider_id: "fake"` is a deliberately local scaffold route that emits a deterministic conversation event sequence. Real provider discovery and execution begin in NAT-04/NAT-05. On Windows, stdin/stdout are switched to binary mode before framing so bytes are not transformed by the CRT.

## Frame fuzz target

With Clang/libFuzzer and sanitizer runtimes available:

```bash
cd native
cmake -S . -B build/fuzz -DPERVUE_BUILD_FUZZERS=ON -DCMAKE_C_COMPILER=clang
cmake --build build/fuzz --target pervue-frame-fuzz
python3 fuzz/create_corpus.py build/fuzz-corpus
./build/fuzz/pervue-frame-fuzz build/fuzz-corpus -runs=1000 -max_len=1048580
```

Configuration performs a compile-and-link probe and fails early with a clear message when libFuzzer/sanitizer runtimes are missing.

The fuzz harness reads directly from memory rather than creating a temporary file per input. The generated corpus seeds empty, small valid, exact-maximum, oversized-prefix, truncated-prefix, and truncated-payload cases so the smoke run starts from structurally meaningful Native Messaging frames.
