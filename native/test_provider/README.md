# Deterministic fake provider

`pervue-fake-provider` is a test-only executable used to exercise provider process supervision without coupling tests to Codex, Claude, or any other real provider. Run under the name `codex`, it acts as a fake Codex CLI instead (see [Fake Codex](#fake-codex)).

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
partial       emit the start of a line with no line ending, then remain alive
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

`tests/process_manager.rs` and `tests/process_stress.rs` test the host's provider process manager (NAT-04) against this binary, including the descendant modes; see "Provider processes" in `native/README.md`. `tests/stream_manager.rs` tests the stream manager (NAT-05) against the same modes, and `partial` exists for it: a stream cancelled while it holds an unfinished line must drop that line.

## Fake Codex

When the binary's file name is `codex` (`codex.exe` on Windows), it answers the two commands the Codex adapter runs, with the event shapes Codex CLI 0.156 prints:

- `codex login status` exits 0 like a signed-in Codex, printing a masked key to stderr that the adapter must never read;
- `codex exec --json [...] [resume <thread id>] -` reads the question from stdin to end of file, as Codex does, and prints a JSON event per line: `thread.started`, a non-fatal warning item, `turn.started`, a command item, the answer `You asked: <question>`, and `turn.completed`. It always writes a fake secret token to stderr, which must never reach events or diagnostics.

`tests/codex_adapter.rs` copies the binary into a fresh directory as `codex` and points the adapter at that directory. A file there named `codex-scenario` chooses the behaviors, one `login=<behavior>` line and one `exec=<behavior>` line:

```text
login=signed-in      exit 0 (the default)
login=signed-out     print "Not logged in" and exit 1
login=broken         exit 3
login=hangs          never answer

exec=answers         answer the question (the default)
exec=two-messages    answer in two agent messages
exec=slow            answer in two messages 300 ms apart
exec=huge            answer with 300,000 bytes, mostly multi-byte characters
exec=fails-401       fail the turn on a rejected API key, after a retry notice
exec=fails-429       fail the turn on a rate limit, after a retry notice
exec=fails-500       fail the turn on an overloaded service, after a retry notice
exec=crashes         abort after the turn starts
exec=no-result       exit 0 after the turn starts, without finishing it
exec=never-starts    announce the thread, then hang before the turn starts
exec=goes-quiet      start the turn, then hang
exec=ignores-cancel  block SIGTERM, start the turn, then hang
exec=malformed       print a line that isn't JSON, then hang
exec=oversized       print a 9 MiB agent message
exec=resume-fails    exit 1 with no JSON when asked to resume a thread
exec=lingers         finish the turn, then hang instead of exiting
```

Each run appends what the adapter sent next to the binary, so tests can check it: its arguments and the first `PATH` entry to `codex-invocations`, and each `exec`'s question to `codex-prompts` (NUL-separated) and its process ID to `codex-pids`.

The fake provider is its own workspace crate outside the default build, so `cargo build` in `native/` produces only the host; `cargo test --workspace` builds and tests the fake provider.
