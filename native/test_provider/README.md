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
```

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

The fake provider is its own workspace crate outside the default build, so `cargo build` in `native/` produces only the host; `cargo test --workspace` builds and tests the fake provider.
