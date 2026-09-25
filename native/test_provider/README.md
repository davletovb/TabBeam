# Deterministic fake provider

`pervue-fake-provider` is a test-only executable used to exercise provider process supervision without coupling tests to Codex, Claude, or any other real provider.

It emits a deliberately small line-oriented JSON test stream on stdout. This is **not** the public Pervue Native Messaging protocol and is not a provider API contract.

## Modes

```text
normal        stream three deterministic JSON lines and exit 0
slow          stream the same lines with 700 ms pauses between chunks
stderr        write a deterministic stderr message, then stream normally
exit-nonzero  write a diagnostic and exit 42
hang          emit a ready line, then remain alive
ignore-cancel emit ready, ignore SIGTERM and an input "cancel" command, then remain alive
malformed     emit a deliberately malformed JSON line and exit 0
large         emit exactly 2 MiB of deterministic stdout bytes and exit 0
crash         abort (SIGABRT on POSIX) without leaving a core file
echo          copy stdin to stdout until end of input, then exit 0
tree          start a `hang` descendant that shares stdout, emit a ready line, then remain alive
orphan        start a `hang` descendant that shares stdout, then exit 0 at once
escape        start a `detached` descendant that shares stdout, then exit 0 at once
detached      leave the process group (a new session on POSIX), emit its pid, then remain alive
```

`tree`, `orphan`, and `escape` leave processes running on purpose, to exercise process-tree cleanup. Run them only under a supervisor that stops the whole process group: in a shell pipeline, the descendant keeps the pipe open after the fake provider exits.

The `large` mode intentionally emits one 2 MiB byte stream **without a newline**. It exists to force later supervision/streaming code to chunk provider output by bounded byte size rather than assume one provider line maps to one protocol frame.

Invalid arguments print usage and exit 64.

Example:

```bash
cd native
cargo build -p pervue-fake-provider
./target/debug/pervue-fake-provider --mode normal
```

The tests place strict timeouts around the non-terminating modes so CI never relies on manual cleanup.

## What the tests pin

`tests/modes.rs` verifies both completed-output and mid-stream behavior. Slow mode is run to completion, is separately required to still be running at one second, and is killed at 0.5 seconds to prove its first line was already flushed. On POSIX, `tests/signals.rs` sends SIGTERM directly: `hang` must terminate on SIGTERM, while `ignore-cancel` must survive SIGTERM until the test escalates to SIGKILL and reaps it. `ignore-cancel` blocks SIGTERM before it prints its ready line, so a supervisor that signals right after readiness always hits the ignored state.

`tests/process_manager.rs` and `tests/process_stress.rs` test the host's provider process manager (NAT-04) against this binary, including the descendant modes; see "Provider processes" in `native/README.md`.

The fake provider is its own workspace crate outside the default build, so `cargo build` in `native/` produces only the host; `cargo test --workspace` builds and tests the fake provider.
