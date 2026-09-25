# Pervue Native Host

This directory contains the native Rust companion/host.

The host serves the Chrome extension over Native Messaging. It validates each protocol-v1 request strictly, runs it through a provider adapter, and streams the answer back as protocol events. Codex CLI is the first real provider.

## Layout

```text
native/
├── Cargo.toml       Cargo workspace: shared version, Rust 1.85+, `unsafe` forbidden
├── host/            pervue-host: the Native Messaging host (binary + library)
│   ├── src/
│   │   ├── diagnostics.rs  structured lifecycle diagnostics (JSON lines on stderr)
│   │   ├── framing.rs   bounded length-prefixed frame reader/writer
│   │   ├── limits.rs    every bound on browser input (frame size, nesting, request IDs)
│   │   ├── manifest.rs  caller-origin checks and the Native Messaging manifest
│   │   ├── process.rs   provider process manager: start, stream, stop, clean up
│   │   ├── stream.rs    stream manager: provider output as bounded lines
│   │   ├── providers/   adapter contract, executable discovery, the fake and Codex adapters
│   │   ├── protocol/    strict request validation and event emission
│   │   ├── host.rs      request loop: requests side by side, cancellation, timeouts
│   │   └── main.rs      command-line entry point
│   └── tests/       command-line tests and the opt-in live Codex test
├── test_provider/   pervue-fake-provider: deterministic fake provider and fake `codex` for tests
└── fuzz/            cargo-fuzz targets and seed-corpus generators
```

## Requirements

- Rust 1.85 or newer (install with [rustup](https://rustup.rs))

## Development build

```bash
cd native
cargo build
cargo test --workspace
```

`cargo build` builds only the host (`target/debug/pervue-host`); the fake provider is a test fixture, so `cargo test --workspace` builds and tests it.

All project crates forbid `unsafe` code. The host crate's `clippy.toml` also bans `std::process::Command::new`, so only the provider process manager (`host/src/process.rs`, NAT-04) starts processes, through one explicitly allowed call (see `docs/security/trust-boundaries.md`). CI treats compiler and Clippy warnings as errors and checks formatting:

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
```

## Native Messaging framing

Pervue uses Chrome Native Messaging framing:

- 4-byte unsigned payload length in the platform's native byte order;
- followed by exactly that many payload bytes;
- zero-length payloads are valid;
- inbound and outbound frames are capped at `MAX_FRAME_SIZE` (1 MiB) from `host/src/limits.rs`, which tests keep equal to `docs/protocol/native-messaging-v1.json`, the copy the extension is tested against;
- oversized lengths are rejected before allocation;
- EOF before any prefix byte is clean end-of-stream;
- partial prefix/payload EOF is a truncated-frame error;
- short reads and writes are retried until the frame is complete or the stream fails.

NAT-02 validates and transports frames. NAT-03 validates protocol-v1 JSON envelopes/payloads and emits normalized protocol failures. NAT-04 and NAT-05 run provider processes and read their output, and PRO-01 to PRO-04 add the provider adapters, starting with Codex (see [Providers](#providers)).

A framing failure ends the host with a deterministic exit status, and the host's last diagnostics record (`host.stopped`, see [Diagnostics](#diagnostics)) names the reason and the exit status.

| Exit status | Meaning |
|---|---|
| 0 | Clean end of stream, or `--version` |
| 2 | I/O error while reading a request, writing an event, or printing the manifest |
| 3 | The stream ended inside a frame |
| 4 | A frame length exceeded the 1 MiB cap |
| 5 | A frame buffer could not be allocated |
| 64 | Missing or unexpected command-line arguments, including a caller origin or extension ID that isn't exact |

## Host behavior at this milestone

```bash
./target/debug/pervue-host --version
```

prints the host version and exits with status 0.

Chrome launches Native Messaging hosts with the caller origin as the first positional argument. Pervue accepts the production launch shape:

```bash
./target/debug/pervue-host chrome-extension://<extension-id>/
```

On Windows, Chrome also passes the calling window's handle as a second argument, `--parent-window=<decimal handle>` (0 when the caller is a service worker). The host accepts that shape and ignores the handle.

The caller origin is required and must be exactly `chrome-extension://<extension-id>/`, where the ID is 32 characters from `a` to `p`. With no arguments, any other origin, unknown flags, or extra positional arguments, the host exits with usage status 64 before reading a frame. To drive the host by hand, pass a well-formed origin such as `chrome-extension://aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa/`.

Normal execution first emits exactly one `host.ready` event, then reads bounded Native Messaging frames until EOF. Each frame must contain exactly one valid protocol-v1 JSON request object.

Malformed JSON, invalid envelopes/payloads, unsupported versions, and unknown methods produce normalized `response.failed` events. Malformed requests do not terminate an otherwise usable host stream.

No object in a request may repeat a member name, including members the host does not interpret; names are compared after decoding escapes, so `"a"` and `"\u0061"` collide. A repeat in the envelope is `INVALID_ENVELOPE` and anywhere inside a method payload is `INVALID_PAYLOAD`; syntax errors still take precedence.

Requests are validated by a strict pull reader (`host/src/protocol/json.rs`) rather than a serde deserializer, because protocol v1 depends on details serde hides: duplicate member names must be rejected, member order decides whether a request ID was recovered before a syntax error, request IDs are echoed byte-for-byte, and depth overflow is classified by where it happens. Outbound event payloads are serialized with serde_json.

Two providers answer requests: `fake`, a deterministic scaffold that answers in process and that protocol tests and the golden fixtures use, and `codex`, the Codex CLI adapter (see [Providers](#providers)). Any other provider ID fails as `PROVIDER_NOT_FOUND` / `PROVIDER_NOT_INSTALLED`. Rust's standard streams pass bytes through unchanged, including Windows pipes, so no binary-mode switch is needed before framing.

## Requests in flight

The host serves requests side by side. A reader thread hands over one frame at a time, and the request loop (`host/src/host.rs`) drives every running request without blocking on any of them, so it reads a `request.cancel` while the target is still streaming. A request that can be answered at once, such as the fake provider's, is answered before the next frame is read.

- **Request IDs.** A request ID must be unique among the requests in flight, including a `request.cancel` still waiting for its target. A request that reuses one fails with `INVALID_REQUEST` / `DUPLICATE_REQUEST_ID`, and the request already running is unaffected. After a request ends, its ID can be used again.
- **Cancellation.** `request.cancel` stops its target, which ends with `response.failed` / `REQUEST_CANCELLED` / `USER_CANCELLED`. The cancellation then ends with `request.cancelled`, naming the target (protocol v1 §7.6). From the moment it is cancelled, nothing more is sent for the target but that failure, not even output its provider had already produced. Every cancellation of the same target is confirmed. A cancellation whose target isn't in flight, has ended, or is already stopping after a timeout fails with `INVALID_REQUEST` / `UNKNOWN_TARGET_REQUEST`.
- **Timeouts.** Each provider sets how long a `conversation.send` may take to start answering (`REQUEST_TIMEOUT` / `PROVIDER_START_TIMEOUT`) and how long its answer may then go without progress (`REQUEST_TIMEOUT` / `PROVIDER_RESPONSE_TIMEOUT`). A request that times out is stopped like a cancelled one. Adapters bound their own status checks.
- **Stopping.** A stopped request's provider gets the provider's grace period to exit before it is killed. If the adapter still hasn't ended the request one second after that, the host ends it anyway and drops the adapter, which kills its processes.
- **End of input.** When the extension closes the stream, each request still running gets 250 ms to stop and ends with `REQUEST_CANCELLED` / `INPUT_CLOSED`; the host exits once all have ended. If stdout closes instead, the host can't answer anyone: it records the running requests as aborted, kills their processes, and exits with status 2.
- **Frame size.** Long answers are split into `response.delta` events of at most 64 KiB, cut between characters, so every event fits in a frame whatever JSON escaping adds.

## Registering the host

Chrome starts the host only for the extensions its Native Messaging manifest lists in `allowed_origins`. The host prints that manifest for exact extension IDs:

```bash
./target/debug/pervue-host --print-manifest <extension-id> [<extension-id>...]
```

The output names `com.pervue.host`, points `path` at the absolute path the host was run from, and lists one `chrome-extension://<id>/` origin per ID. Anything that isn't a 32-character `a`–`p` ID, such as a wildcard or a full origin, is refused with status 64. The path isn't canonicalized: on macOS a symlink or `..` is kept as typed, while Linux reports the resolved path. That is deliberate, because a stable symlink can be the right path to register, where its versioned target would break on upgrade. Installers should run the host from the path they want registered (PKG-01). `extension/README.md` shows where to save the manifest for development; installers register it later (PKG-01).

## Diagnostics

While it serves a session, the host writes structured diagnostics to stderr, one JSON object per line (OBS-01). stdout carries only Native Messaging frames, so diagnostics can't corrupt them. If writing to stderr fails, for example because it's closed, the host ignores the failure and carries on. A write that blocks isn't skipped, though: a stderr pipe that nobody reads would eventually stall the host. Chrome passes a host's stderr through to its own, so that only happens when Chrome's own stderr is such a pipe, and starting Chrome with `--enable-logging=stderr` shows the records.

```text
{"ts":"2026-09-25T01:21:49.903Z","event":"host.started","host_version":"0.1.0-dev","pid":1764}
{"ts":"2026-09-25T01:21:49.903Z","event":"request.completed","request_id":"req_a","method":"conversation.send","provider_id":"fake","conversation_id":"conv_1","duration_ms":0}
{"ts":"2026-09-25T01:21:49.903Z","event":"request.failed","request_id":"req_b","method":"conversation.send","provider_id":"codex","duration_ms":0,"error":{"code":"PROVIDER_NOT_FOUND","reason":"EXECUTABLE_NOT_FOUND"}}
{"ts":"2026-09-25T01:21:49.903Z","event":"request.rejected","error":{"code":"INVALID_REQUEST","reason":"MALFORMED_MESSAGE"}}
{"ts":"2026-09-25T01:21:49.903Z","event":"host.stopped","duration_ms":0,"reason":"end_of_input","exit_code":0,"requests":2,"rejected":1}
```

Every record has `ts` (RFC 3339 UTC, with milliseconds) and `event`. A field that doesn't apply is left out. Every request the host reads gets exactly one `request.*` record, including a request it was answering when it stopped. A completed or failed request's `conversation_id` is the conversation it continued or, when it started one, the ID its `conversation.created` event announced, so the first request of a conversation correlates with the ones that follow.

| Event | Written when | Fields |
|---|---|---|
| `host.started` | Before `host.ready` | `host_version`, `pid` |
| `request.completed` | The request ended successfully: `response.completed`, or `request.cancelled` for a cancellation | `request_id`, `method`, `provider_id`, `conversation_id`, `target_request_id` (for `request.cancel`), `duration_ms` |
| `request.failed` | The request ended with `response.failed` | The same fields, plus `error` (`code` and `reason`) |
| `request.aborted` | The host stopped while answering the request, such as when stdout closed, so the extension got no terminal event | The same fields as `request.completed`, plus `reason` (why the host stopped). Its `conversation_id` is the one the request continued or, if the provider had already reported it, the one the request created |
| `request.rejected` | The request failed validation, or reused the ID of a request in flight, and never reached a provider | `request_id` if one was recovered, `method` for a reused ID, and `error` (`INVALID_REQUEST` and its reason) |
| `host.stopped` | The host is about to exit | `reason` (`end_of_input`, `io_error`, `frame_truncated`, `frame_too_large` or `allocation_failed`), `exit_code`, `duration_ms` (uptime), `requests` (requests that passed validation and were served) and `rejected` (requests that failed validation or reused an ID in flight) |

A record never contains request content: no prompt text, page context, other payload members, raw frame bytes, or error messages. Nor does it contain provider output: a provider's stderr and error messages are discarded (see [Codex](#codex)). Every identifier it records is cut to 128 characters, and JSON escaping keeps each record on one line whatever they contain. SEC-02 reviews redaction across providers.

Command-line errors, such as a usage error or an invalid `--print-manifest` ID, are plain text on stderr, because no session is running.

## Providers

Provider support is four layers, each with its own tests: the adapter contract the request loop drives (PRO-01), the adapters behind it (the Codex adapter is PRO-02 to PRO-04), the stream manager that reads provider output as lines (NAT-05), and the process manager that runs provider processes (NAT-04).

### Adapter contract

`host/src/providers/mod.rs` defines the contract. It stays provisional until a second real provider works (framework §10.1).

- A `Provider` has an ID, which requests name, and `Timeouts`: how long a request may take to start answering, how long it may then go without progress, and how long a stopped request's process gets to exit.
- `status()` and `send(request)` start an `Exchange`: a state machine the request loop drives. `next(deadline)` returns the next `Update`, or `None` once the deadline passes, and never blocks longer, so one slow provider can't hold up other requests.
- Updates are in protocol terms: `ConversationCreated`, `Started`, `Delta`, `Status`, and `Activity` (progress with nothing to show, which counts for the idle timeout), then one terminal update, `Completed`, `Failed`, or `Stopped`. Command lines, output formats, and provider session IDs stay inside the adapter.
- `cancel(grace)` stops the work. The exchange then ends with `Stopped`, or with the terminal update it had already reached, and kills any process still running after `grace`.
- `Providers::installed()` is the registry of an installed host: `fake` and `codex`, which `provider.status` without a provider ID reports in that order. `Providers::scaffold()` holds only `fake`, which starts no processes, for fuzzing and protocol tests.

### Codex

`host/src/providers/codex/` is the Codex CLI adapter. It was written against Codex CLI 0.156.1 and verified against that release.

- **Discovery.** The adapter looks for an executable named `codex` in the host's `PATH` and then in the usual install locations that Chrome's minimal `PATH` can leave out: `/opt/homebrew/bin` (macOS), `/usr/local/bin`, `~/.local/bin`, `~/.npm-global/bin`, `~/.volta/bin`, `~/.bun/bin`, `~/bin`, and the `bin` directory of each Node version nvm installed, newest first. On Windows it looks for `codex.exe`, then `codex.cmd`, in `PATH` and `%APPDATA%\npm`. `PERVUE_PROVIDER_PATH`, a list of directories in `PATH` form, replaces all of these, for unusual installs and hermetic tests. Relative directories are skipped, and nothing in a request affects the lookup (`host/src/providers/discovery.rs`).
- **Status.** `provider.status` runs `codex login status` and reads only its exit status: 0 is `authenticated` and 1 `unauthenticated`. Any other status, or no answer within 10 seconds, is `unknown`. The command's output names the account and a masked key, so it is never read. If no executable is found, the availability is `not_found`; if it can't be started, `unavailable`.
- **Requests.** A signed-out `codex exec` keeps retrying instead of failing, so each `conversation.send` first checks the sign-in the same way and fails at once if Codex is signed out. Then it runs:

  ```text
  codex exec --json --skip-git-repo-check --sandbox read-only -C <work dir> [resume <thread id>] -
  ```

  The question goes on stdin, never in an argument. `<work dir>` is an empty `pervue-codex` directory in the system temporary directory, and the read-only sandbox keeps Codex from changing files. The executable's own directory is put first in Codex's `PATH`, because npm installs `codex` as a Node script that finds `node` there.
- **Answers.** Codex prints one JSON event per line (`host/src/providers/codex/output.rs`). The response starts at `turn.started`. Each completed agent message is a `response.delta`, with a blank line before each message after the first. Other items, such as reasoning and tool calls, count as progress for the idle timeout. `turn.completed` completes the request. Exec mode reports each message whole when it completes, not token by token.
- **Conversations.** A new conversation gets a random Pervue ID, `conv_` and 16 hex digits, which the adapter maps to the Codex thread; continuing the conversation resumes that thread. The map lives in the host process, so a conversation can be continued only while the host that started it runs; after that it fails with `UNKNOWN_CONVERSATION`. Conversations that outlive the host are Milestone C.
- **Limits.** Codex gets 60 seconds to start answering and 5 minutes without progress, because a model can think for minutes without any output. A stopped request's Codex gets 2 seconds to exit before it is killed. After the turn ends, Codex gets 5 seconds to save its session and exit before it is stopped; the answer stands either way. A line of output over 8 MiB ends the request.
- **Capabilities.** `streaming`, `continuation`, and `cancellation` are `true`, and `web_search` is `"unknown"`. `page_context`, `attachments`, and `model_selection` are `false` until Pervue passes them to Codex. A request that attaches browser context (`payload.context`) therefore fails before Codex runs, rather than being answered as if the user had attached nothing.

Failures map to the normalized errors of `docs/protocol/errors-and-capabilities-v1.md`. Codex's own messages and stderr can hold URLs, account details, and masked keys, so they are never forwarded or logged: every failure carries a fixed message.

| Situation | `code` / `reason` | Retryable |
|---|---|---|
| No `codex` executable | `PROVIDER_NOT_FOUND` / `EXECUTABLE_NOT_FOUND` | no |
| `codex login status` says signed out | `PROVIDER_NOT_AUTHENTICATED` / `LOGIN_REQUIRED` | no |
| A turn failed on a 401 or 403 status or a rejected API key | `PROVIDER_NOT_AUTHENTICATED` / `AUTH_REJECTED` | no |
| A turn failed on a 429 status, a rate or usage limit, or a quota | `PROVIDER_FAILED` / `PROVIDER_RATE_LIMITED` | yes |
| Any other failed turn | `PROVIDER_FAILED` / `PROVIDER_UNAVAILABLE` | yes |
| Codex exited with an error before the turn ended, as in a crash or a lost session | `PROVIDER_FAILED` / `PROCESS_EXITED` | yes |
| Output that isn't Codex's event stream: a line that isn't an event, events out of order, a line over 8 MiB, or a clean exit before the turn ended | `PROVIDER_FAILED` / `MALFORMED_PROVIDER_OUTPUT` | no |
| Codex couldn't be started | `PROVIDER_FAILED` / `PROVIDER_UNAVAILABLE` | no |
| A `conversation_id` the adapter doesn't know | `INVALID_REQUEST` / `UNKNOWN_CONVERSATION` | no |
| The request attaches browser context, which Codex doesn't receive yet | `INVALID_REQUEST` / `PAGE_CONTEXT_UNSUPPORTED` | no |

`test_provider/tests/codex_adapter.rs` runs the adapter, and the whole host, against a fake `codex`: the fake provider binary copied under that name (see `test_provider/README.md`). The tests cover discovery and sign-in status, a Codex that can't start, the exact command line and the question on stdin, streaming, continuing a conversation, refused page context, failed turns, crashes, malformed and oversized output, cancellation, and both timeouts. `host/src/providers/codex/fixtures/` holds `codex exec --json` output captured from Codex CLI 0.156.1, and `output.rs`'s tests parse it.

`host/tests/live_codex.rs` asks the installed, signed-in Codex one question through the built host and checks the whole answer. It needs Codex and its credentials, so it runs only when asked:

```bash
cd native
PERVUE_LIVE_CODEX=1 cargo test -p pervue-host --test live_codex -- --nocapture
```

### Stream manager

`host/src/stream.rs` is the stream manager (NAT-05). `LineStream` reads a provider process's stdout as lines. `next(deadline)` returns one complete line at a time, and then exactly one terminal state, which later calls repeat:

- `Final(exit)` when stdout ended and the process exited. A last line without a line ending is still delivered.
- `Error` when a line grew past the stream's limit or wasn't UTF-8. The process is killed at once.
- `Stopped(exit)` after `cancel(grace)`.

The process manager's chunks end wherever a read did, even inside a UTF-8 character. The stream manager reassembles lines across any number of chunks, drops a `\r` before a `\n`, and checks each line is UTF-8 once it is complete. The limit counts a line as delivered, without its line ending. It holds at most one line of up to the limit and the lines of one chunk, and the process manager reads at most 16 chunks ahead, so a provider that floods its output waits on its own writes instead of growing the host's memory. stderr is counted and discarded, because it is written for people and can hold secrets. `cancel(grace)` drops everything not yet delivered, asks the process to stop (`request_stop`), and kills it once `grace` passes.

`split_text` cuts outgoing text into pieces of bounded size, never inside a character; the host uses it for `response.delta`.

The unit tests in `stream.rs` pin the line splitting: characters cut between chunks, CRLF endings, a last line without an ending, the limit, and invalid UTF-8. `test_provider/tests/stream_manager.rs` runs the stream manager on real processes: 2,000 lines full of multi-byte characters, stderr, a line over the limit, a 2 MiB line read slowly, a crash, and cancelling between lines, mid-line, and against a provider that ignores SIGTERM.

### Provider processes

`host/src/process.rs` is the provider process manager (NAT-04). It is the only code in the host that starts a process; the Codex adapter reaches it through the stream manager.

```rust
let mut process = Process::spawn(&ProcessSpec::new(executable).args(["exec", "--json"]))?;
process.write(prompt.as_bytes())?;
process.close_stdin();

let deadline = Instant::now() + timeout;
while let Some(event) = process.next_event(deadline) {
    match event {
        Event::Stdout(bytes) => { /* provider output */ }
        Event::Stderr(bytes) => { /* provider diagnostics */ }
        Event::Exited(exit) => return Ok(exit),
    }
}
let exit = process.terminate(Duration::from_secs(2)); // the deadline passed
```

- **Starting.** The program must be an absolute path, and each argument is its own argv element: there is no shell and no `PATH` search. stdin, stdout, and stderr are always three separate pipes, so a provider never gets the host's own Native Messaging streams.
- **Input.** `write` queues bytes for a helper thread and returns at once, so a provider that isn't reading can't block the host. `close_stdin` sends end of file after the queued input.
- **Output.** `next_event` returns stdout and stderr chunks of at most 8 KiB (`MAX_CHUNK_BYTES`) as the provider writes them, then one final `Exited`. At most 16 chunks are read ahead of the caller; beyond that the provider waits on its own writes, so a flood can't grow the host's memory. Chunks end wherever a read did, so they can split lines and UTF-8 sequences; reassembling them is the stream manager's job (NAT-05).
- **Timeouts.** `next_event` returns `None` once its deadline passes, and the caller decides what happens next.
- **Stopping.** `terminate(grace)` closes stdin, sends SIGTERM to the provider's process group, waits up to `grace`, and then kills the group with SIGKILL. `kill()` sends SIGKILL at once. Both reap the provider and return its `Exit`: the status, whether it exited on its own (`Natural`), after the request (`Stopped`), or was `Killed`, and whether its output closed. `request_stop()` closes stdin and sends SIGTERM without waiting, for callers that keep reading events until the provider exits, as the stream manager does.
- **Cleanup.** Dropping a `Process` kills and reaps it, so no provider outlives its `Process`, not even as a zombie. The provider leads its own process group, so stopping it stops everything it started, and when it exits on its own the host kills whatever it left in the group, which would otherwise live on as an orphan and could hold its output open.

What it can't do yet:

- A descendant that leaves the process group, for example with `setsid`, is out of reach. If it holds the output open, the host stops waiting one second after the provider exits and reports `output_closed: false`.
- When a provider exits on its own, the host kills its group right after reaping it. If the provider left nothing behind, the group ID is free again by then. The kill would reach another group only if one took that ID within microseconds, which takes process IDs wrapping around in that window. Closing the gap needs a way to see the exit before reaping, which nix offers on Linux (`waitid` with `WNOWAIT`) but not on macOS (SEC-02).
- On Windows only the provider process itself is stopped; stopping its descendants too needs a Job Object (ADR-0001). Windows has no SIGTERM, so closing stdin is the only stop request there.
- If the host itself is killed outright, it can't clean up. A provider that reads stdin sees end of file.
- Providers inherit the host's environment and working directory, apart from variables their spec sets with `env`. SEC-02 narrows what they receive.

The fake provider's crate tests the manager, because only it can locate the fake provider binary: `test_provider/tests/process_manager.rs` covers success, a nonzero exit, a crash, a timeout, a graceful stop, an ignored stop escalated to SIGKILL, input written while output flows, and descendants that stay in the group, outlive the provider, or leave the group. `test_provider/tests/process_stress.rs` spawns and stops 120 providers at different points and checks that each was reaped and that no pipe or thread was left behind.

## Fuzz targets

With nightly Rust and [cargo-fuzz](https://github.com/rust-fuzz/cargo-fuzz) (`cargo install cargo-fuzz`):

```bash
cd native
python3 fuzz/create_corpus.py fuzz/corpus/frame_reader
python3 fuzz/create_protocol_corpus.py fuzz/corpus/protocol --fixtures ../docs/protocol/fixtures/v1-golden.json
cargo +nightly fuzz run frame_reader fuzz/corpus/frame_reader -- -runs=1000 -max_len="$(python3 fuzz/max_len.py frame_reader)"
cargo +nightly fuzz run protocol fuzz/corpus/protocol -- -runs=2000 -max_len="$(python3 fuzz/max_len.py protocol)"
```

cargo-fuzz builds the targets with AddressSanitizer. `frame_reader` reads frames from memory until the first non-frame result. `protocol` runs each input through the whole host as one request frame and fails if any emitted frame is not a JSON object.

The harnesses read directly from memory rather than creating a temporary file per input. The generated corpora seed empty, small valid, exact-maximum, oversized-prefix, truncated-prefix, and truncated-payload frames, plus requests derived from the golden protocol fixtures, so smoke runs start from structurally meaningful inputs.
