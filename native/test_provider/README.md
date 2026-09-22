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

Example:

```bash
./build/dev/pervue-fake-provider --mode normal
```

The test harness places strict timeouts around the non-terminating modes so CI never relies on manual cleanup.

## What the tests pin

The CTest harness verifies both completed-output and mid-stream behavior. Slow mode is run to completion, is separately required to time out before one second, and is killed at 0.5 seconds to prove its first line was already flushed. On POSIX, a dedicated process test sends SIGTERM directly: `hang` must terminate on SIGTERM, while `ignore-cancel` must survive SIGTERM until the test escalates to SIGKILL and reaps it.

The fake-provider target exists only when `BUILD_TESTING=ON`, preventing test infrastructure from appearing in non-test/package-oriented builds.
