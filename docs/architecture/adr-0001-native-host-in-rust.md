# ADR-0001: Write the native host in Rust

**Status:** Accepted  
**Date:** 2026-09-24  
**Decided by:** Project owner  
**Supersedes:** the C requirement in Framework v0.1 (§1, §3.6, §6.1, §8, §9, §14.3, §16, §24, §25) and the C-specific wording of NAT-01, TST-01, the LIB items, and DOC-03 in Tracker v0.1  
**Implemented by:** the Rust port of NAT-01, NAT-02, NAT-03, TST-01, and TST-02

## Context

Framework v0.1 specified a native C host and, later, a reusable C library. When this decision was made, the Foundation items and three Milestone A items (NAT-02 framing, NAT-03 request validation and routing, TST-02 fake provider) were implemented in C. None was `VERIFIED`, and 71 of 81 tracker items were still in the backlog.

The remaining native work is:

- **Concurrent.** Protocol v1 allows several in-flight requests plus `request.cancel`, so the process and stream managers (NAT-04, NAT-05) must read Chrome's stdin while streaming stdout and stderr from provider processes, with timeouts and kill escalation.
- **Cross-platform.** macOS comes first, then Windows. Windows cannot poll child pipes the way POSIX polls file descriptors, and `CreateProcessW` takes a single command-line string that npm-installed `.cmd` shims hand to `cmd.exe` for re-parsing.
- **Security-sensitive.** The host parses untrusted input at the browser/native boundary: page content, Native Messaging payloads, and provider output.
- **JSON-heavy.** Provider adapters consume nested JSONL event streams whose formats change between CLI releases.

Performance was not a deciding factor: the framework notes that provider and network latency dominate (§16).

## Decision

The native host and all native tooling are written in Rust.

- A Cargo workspace in `native/` replaces the CMake build. The minimum supported Rust version is 1.85 (edition 2024).
- Project crates forbid `unsafe` code. CI treats compiler and Clippy warnings as errors on Linux, macOS, and Windows.
- cargo-fuzz targets, which run under AddressSanitizer, replace the ASan/UBSan C job.
- Request validation stays a strict hand-written pull reader, because protocol v1 depends on details a serde deserializer hides (duplicate names, request-ID recovery order, byte-for-byte request-ID echo, and where a depth overflow happens). Outbound events are serialized with serde_json.
- The reusable library planned for Milestone F becomes a Rust library crate. If a non-Rust application needs it, it can be exposed through a C ABI (`extern "C"` with generated headers) without changing the Rust API.

## Alternatives considered

**Keep C.** This was viable, and C remains the natural choice when the product itself is a C library for C consumers. Staying in C meant writing the process and stream managers twice (POSIX and Windows) or adopting libuv, adding a JSON library for provider output, and keeping memory safety dependent on sanitizers and fuzzing. The reusable library is deferred until real reuse exists (framework §3.7, §9.6), so it did not outweigh the cost of doing the remaining work in C.

## Consequences

- One implementation of concurrent process and stream handling can cover all three platforms, for example with tokio.
- The compiler rules out memory-safety bugs in project code, instead of relying on sanitizers to catch them.
- Contributors need a Rust toolchain. The host binary is somewhat larger than the C build, which does not matter for a desktop companion.
- Some per-OS code remains, as it would in C: terminating process trees (process groups on POSIX, Job Objects on Windows) and registering the Native Messaging host.
- A C ABI for the reusable library is extra work, done only if a non-Rust consumer appears.

## Verification of the port

- Before the C sources were removed, the Rust host matched the C host byte-for-byte, including exit statuses, on about 2.3 million differential inputs. The original CMake mode harness and C signal test passed against the Rust fake provider.
- The protocol conformance harness (`scripts/validate-host-protocol.mjs`) and the golden fixtures carried over unchanged.
- Review then made two deliberate behavior changes on top of the port. The host accepts Chrome's Windows launch shape (`--parent-window=<handle>` after the origin), which the C host rejected. Duplicate member names are now rejected in every object of a method payload, as protocol v1 §1 rule 9 requires; the C host only rejected duplicates of the members it interpreted.
