# Pervue Native Host

This directory contains the native C companion/host.

NAT-01 establishes only the portable build and process skeleton. Native Messaging framing is intentionally deferred to NAT-02.

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

## Host behavior at this milestone

```bash
./build/dev/pervue-host --version
```

prints the host version and exits with status 0.

Unsupported command-line arguments print usage information to stderr and exit with status 64.

Normal execution reads stdin until EOF and exits cleanly without interpreting or emitting protocol data. On Windows, stdin/stdout are switched to binary mode before normal host execution so future length-prefixed Native Messaging bytes are not transformed by the CRT. NAT-02 replaces the foundation read loop with bounded Chrome Native Messaging framing.
