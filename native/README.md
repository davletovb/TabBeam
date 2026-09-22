# Pervue Native Host

This directory contains the native C companion/host.

NAT-01 establishes only the portable build and process skeleton. Native Messaging framing is intentionally deferred to NAT-02.

## Requirements

- CMake 3.20+
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

## Host behavior at this milestone

```bash
./build/dev/pervue-host --version
```

prints the host version.

Normal execution reads stdin until EOF and exits cleanly without interpreting or emitting protocol data. NAT-02 replaces that foundation behavior with bounded Chrome Native Messaging framing.
