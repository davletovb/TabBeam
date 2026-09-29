# Shared runtime core

`runtime-core` is the browser-neutral foundation of the shared provider runtime described by ADR-0002. During Stage 1 it lives in the Pervue workspace, but its public API does not depend on the extension, Chrome Native Messaging, Pervue request IDs, browser context, provider CLI formats, diagnostics, or user-facing copy.

| Public module | Owns | Kept outside `runtime-core` |
|---|---|---|
| `process` | Absolute-path spawn, isolated child environment, piped I/O, bounded output queue, termination and reap | Executable choice, argv, trusted environment allowlist, provider parsing |
| `stream` | Bounded UTF-8 lines, process-output streaming, terminal/error/stopped states, stderr tail, outgoing text chunks | Interpretation of provider lines and provider-specific messages |
| `discovery` | Absolute directory search and platform executable detection | Pervue's provider-path override and provider-specific path policy |
| `protocol` | Neutral failure (with the runtime's own `#[non_exhaustive]` error codes), capability (including `tool_isolation`), source, provider-status, and model-option values | Protocol-v1 error categories, page-context and attachment capabilities, browser wire envelopes, and user-facing failure wording |
| `exchange` | Deadline-driven `Exchange`, lifecycle `Update` values (`Launched`, `Session`, `SessionLost`, `Started`, `Delta`, …), `Timeouts`, scripted exchanges | Provider registry, conversation IDs and routing, Native Messaging serialization |
| `turn` | Neutral messages, tool policy (`None`, `NativeWebSearch`, `ProviderDefault`), session policy, cleanup group, usage, sign-in classification, namespace (a safe path component, and never a Windows device name such as `con`) and argv-bound identifier validation | Pervue browser-context policy and conversation-to-session mappings |
| `prompt` | How every adapter renders a turn: the system prompt and its introduction, the earlier messages, and the search instructions | Framing of browser context, or of anything else an application attaches to its own message |
| `search` | Bounded, plain-text normalization of provider-native search results into sources | Whether a request asks for search at all |

The scheduler and threaded service are sibling crates, `provider-runtime-scheduler` and `provider-runtime-service`. A service turn's queue holds at most one unread `Update::Activity` (progress with nothing to show, which the scheduler has already counted), so a provider that floods progress cannot fill memory behind a slow reader. The platform layer (`runtime-platform`: the trusted provider-environment allowlist, private files and workspaces, executable discovery policy, and the layout that names an application's directories) and the adapters (`runtime-providers`: the `Provider` trait and the Codex, Claude, Gemini and Grok adapters) are crates of their own beside it, and none of them depends on Pervue. Native Messaging framing is intentionally **not** part of the shared runtime; it lives in `native/host/src/framing.rs` because it is Pervue's browser transport boundary.

Only the public modules and types above are source API. Process-tree control, bounded-I/O implementation details, platform detection internals, provider command lines, cleanup record layouts, and provider-native transcript formats are implementation details. Windows process-tree termination still requires a Job Object and is not promised by the current API.

## Ownership, errors, and cleanup

`Process::spawn(&ProcessSpec)` borrows its specification only during spawn; the returned `Process` owns the child and all three pipes. `Process::write` copies bytes into its bounded writer path, `Process::next_event` returns owned byte vectors, and `LineStream` owns its `Process` and returns owned lines. Dropping a process or stream kills and reaps the child. Explicit terminate/kill operations are safe after completion.

An `Exchange` is owned by the scheduler for the accepted turn. The scheduler is responsible for start/idle/absolute limits, cancellation, stop grace and final destruction. Dropping an owned exchange is therefore also a cleanup boundary: provider-controlled drops are isolated by the supervisor. A service `Turn` handle cancels its provider turn when dropped before `Ended`; dropping the whole runtime cancels all remaining turns and waits through stop handling.

Runtime failures are Rust-owned `Failure { code, reason, retryable }` values. They deliberately carry no Pervue prose and no raw provider error string. Pervue translates them to protocol-v1 `ErrorBody` values in `host/src/protocol/messages.rs`. Provider stdout/stderr, prompts, account identifiers and credentials are never part of a runtime failure.

Provider-native session handles are opaque strings. Only `Persistent` turns may report or continue one. Persistent Claude/Codex turns emit `Session` before `Started`, and emit it again if the provider changes the handle. A turn that resumes a session it can't use sends `SessionLost` before its own terminal failure, `Confirmed` when the provider said the session is gone and `Suspected` when the run merely ended before the turn began; the application decides whether to start over. Pervue owns the mapping from its conversation ID to that opaque handle (`pervue-host`'s `conversations` layer, the only place a conversation exists). When Pervue forks a fresh search session, every superseded handle is recorded durably before best-effort transcript deletion so `conversation.forget` can retry cleanup. A turn's `cleanup_group` groups its per-turn cleanup records the same way, and is opaque to the runtime.

`Ephemeral` means provider state must not outlive the turn. Modes with a native switch use it (`claude --no-session-persistence`, `codex exec --ephemeral`); stateless Gemini/Grok execution refuses `Persistent`. A mode that cannot meet its stated session policy must fail instead of silently weakening it.

## Compatibility and extraction

There is **no C ABI, Node addon, or shared-library binary interface** in Stage 1. The crates are statically linked. Rust source API changes are reviewed against every workspace consumer; while the runtime remains 0.x, source compatibility is not promised across arbitrary revisions.

The browser compatibility boundary is separate. Packaged Pervue hosts speak the versioned Native Messaging protocol documented under `docs/protocol/`; changing a Rust crate version does not change that wire contract, and changing a runtime type does not implicitly permit a protocol-v1 change. Framing itself is host-owned and fuzzed through the host's public framing API.

The extraction rule is the framework's two-consumer rule: a primitive or adapter is promoted only after two real consumers, applications, or provider modes demonstrate the same semantics. Codex and Claude established the process/stream/session contract; Pervue and Conclave justify the shared runtime. Product-only behavior stays with the application.

Stage 2 moves the reusable runtime into its own repository. Pervue switches to the extracted revision first; a shared adapter is fixed there rather than patched locally. A future non-Rust consumer uses a versioned stdio sidecar around the service API rather than an ABI façade. The adapters and everything they use (`native/providers/` and `native/platform/`) depend on no part of `pervue-host`: conversations, browser context and protocol v1 sit above them, and the fake provider, the contract and hostile-matrix tests and the `stream_lines` fuzz target already sit beside them (`native/fake-provider/`, `native/runtime-tests/`, `native/runtime-fuzz/`), which is what makes the move a change of location.

## Verification

Run the neutral crate independently with:

```bash
cd native
cargo test -p runtime-core
```

The real-child process/stream integration suites live in `runtime-tests` because that package builds the fake provider executable they need (`runtime-fake-provider` holds its behaviour and harness). The `stream_lines` fuzz target is the runtime's own, in `runtime-fuzz`, and compiles against `runtime-core::stream` alone; the Native Messaging `frame_reader` fuzz target belongs to Pervue and intentionally compiles against `pervue_host::framing`, matching the host-owned transport boundary. Workspace CI also exercises scheduler/service panic and lifecycle tests, the provider contract and hostile-provider tests at the runtime's level, protocol fixtures, and platform packaging; a separate job checks that no runtime crate depends on a `pervue*` crate (`scripts/check-runtime-independence.mjs`).
