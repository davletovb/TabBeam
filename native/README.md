# Pervue Native Host

This directory contains the native C companion/host.

The current host foundation includes bounded Chrome Native Messaging framing. JSON validation/routing is intentionally deferred to NAT-03.

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

The sanitizer preset fails during configuration when the active compiler is not configured for ASan+UBSan, rather than silently producing an unsanitized build.

## Native Messaging framing

Pervue uses Chrome Native Messaging framing:

- 4-byte unsigned little-endian payload length;
- followed by exactly that many payload bytes;
- zero-length payloads are valid;
- inbound and outbound frames are capped at `PERVUE_NATIVE_MAX_FRAME_SIZE` (currently 1 MiB);
- oversized lengths are rejected before allocation;
- EOF before any prefix byte is clean end-of-stream;
- partial prefix/payload EOF is a truncated-frame error.

NAT-02 only validates and transports opaque payload bytes. NAT-03 owns JSON parsing, envelope validation, routing, and protocol responses.

## Host behavior at this milestone

```bash
./build/dev/pervue-host --version
```

prints the host version and exits with status 0.

Unsupported command-line arguments print usage information to stderr and exit with status 64.

Normal execution reads bounded Native Messaging frames from stdin until EOF. Valid payloads are currently consumed without interpretation or response. On Windows, stdin/stdout are switched to binary mode before framing so bytes are not transformed by the CRT.

## Frame fuzz target

With Clang/libFuzzer available:

```bash
cd native
cmake -S . -B build/fuzz -DPERVUE_BUILD_FUZZERS=ON -DCMAKE_C_COMPILER=clang
cmake --build build/fuzz --target pervue-frame-fuzz
./build/fuzz/pervue-frame-fuzz
```

The fuzz harness feeds arbitrary byte streams into the same bounded frame reader used by the host.
