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

Example:

```bash
./build/dev/pervue-fake-provider --mode normal
```

The test harness places strict timeouts around the non-terminating modes so CI never relies on manual cleanup.
