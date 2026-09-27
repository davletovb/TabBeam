# Proposal: a shared provider runtime for Pervue and Conclave

**Status:** Proposed, for discussion  
**Date:** 2026-09-27  
**Scope:** extracting a reusable library from `native/`, with [Conclave](https://github.com/davletovb/conclave) as its first outside consumer  
**Evidence:** Pervue at `2cc35ef`, Conclave at `1379a37`

## Summary

- **Conclave is the second consumer, but for only part of the code.** The extraction rule (framework §9.6) has been waiting for a second consumer. Conclave needs only the *provider runtime*: running an authenticated AI CLI safely, streaming its answer, and stopping it. Native Messaging, browser context, and protocol v1 stay in Pervue.
- **Use a sidecar process, not linked code.** Conclave is a TypeScript/Node server. The recommendation is that it run the library as a **sidecar executable speaking a versioned JSON-RPC protocol over stdio**, not load it through a C ABI or a Node addon. ADR-0001 expected a C ABI for non-Rust consumers, so this decision would be recorded as ADR-0002.
- **Start with Claude.** Both products drive `claude -p` with stream-json, so Pervue's adapter already covers most of what Conclave needs. It also replaces Conclave's weakest adapter (§2.1).
- **Codex needs a new adapter first.** Conclave uses `codex app-server`, while Pervue uses `codex exec`. Moving Conclave to `exec` would lose token streaming, the live model list, and rate-limit data. An app-server adapter in the library would give Pervue token-level streaming too.
- **Grok and Gemini knowledge flows the other way.** Conclave's working TypeScript adapters are the reference for Pervue's backlog items PRO-08 and PRO-09.
- **Incubate in `native/`, then split.** Add new crates next to the existing ones, and don't move files while the remaining tracker items are in flight. The library gets its own repository once Conclave runs on it.

## 1. What exists today

### Pervue

`pervue-core` already holds the primitives that Codex and Claude share ([core/README](../../native/core/README.md)): `process`, `stream`, `discovery`, `exchange`, `protocol`, and `framing`. The next layer is provider plumbing, not browser policy, but it still lives in the host, and most of it is copied between the two adapters:

| Host-owned piece | Where | Reuse today |
|---|---|---|
| Conversation→session map (`session_name`, read, save, forget, `new_conversation_id`) | [claude/mod.rs][p-claude], [codex/mod.rs][p-codex] | Each adapter has its own copy |
| Status probe, sign-in from the exit status, `PATH` for npm shims, data-directory lookup | [claude/mod.rs][p-claude], [codex/mod.rs][p-codex] | Each adapter has its own copy |
| Environment allowlist | [providers/environment.rs][p-env] | Shared |
| Private workspace and its ownership checks | [codex/workspace.rs][p-workspace] | Claude imports Codex's module |
| Removing provider transcripts | [providers/forget.rs][p-forget] | Shared |
| Request loop: in-flight table, start/idle timeouts, cancel → stop → kill, fairness, delta splitting | [host.rs][p-host] (`Session`, `Running`) | Tied to writing protocol-v1 events |

Consolidating the copied rows is worth doing whatever happens with Conclave.

### Conclave

Conclave's `ProviderAdapter` ([packages/core/src/index.ts][c-core]) has three parts: `listModels`, `generate(request, emit)` with an `AbortSignal`, and an optional `limits`. Four adapters implement it, and each handles processes its own way:

| Provider | Runtime and transport | Prompt | Child environment | Stopping | Output |
|---|---|---|---|---|---|
| Anthropic | `claude -p` with stream-json, one process per call | Last command-line argument | Server's environment minus 5 names | SIGTERM to the child only | All stdout and stderr kept in strings |
| OpenAI | `codex app-server`, one long-lived JSON-RPC process | stdin | Server's full environment | `turn/interrupt`; the process gets SIGTERM | `readline`, no line limit |
| xAI | `grok agent stdio` (ACP), one process per call | stdin | Server's environment minus 4 names, plus lockdown flags | SIGTERM to the child only | `readline`, no line limit |
| Google | `agy` with stream JSON, a private temporary workspace per call | stdin | Server's environment minus a list of billing variables | SIGTERM to the process group, SIGKILL after 1 s | All stdout and stderr kept in strings |

Conclave also:

- decides whether to retry or report a rate limit by matching error messages against regular expressions ([orchestrator.ts:98–130][c-classify]);
- keeps no provider sessions: Codex threads are `ephemeral`, Claude runs with `--no-session-persistence`, and each prompt replays the history;
- refuses API-key, Console, and cloud billing by reading the account type (`claude auth status` JSON, Codex `account/read`);
- re-arms a 180-second stall watchdog on every provider event ([orchestrator.ts:507][c-watchdog]).

## 2. What each product gains

### 2.1 What Conclave gains from Pervue's code

In order of impact:

1. **The Claude prompt is a command-line argument** ([anthropic-claude.ts:254][c-claude-argv]).
   - Linux limits a single argument to 128 KiB (`MAX_ARG_STRLEN`), and Windows limits the whole command line to 32,767 characters. A Debate, Judge, or synthesis prompt that embeds other models' answers can fail to start.
   - On Linux, other local users can read the prompt with `ps`.
   - Pervue sends the prompt on stdin as a stream-json message.
2. **Memory is unbounded.**
   - Claude's and Gemini's output accumulates in strings for the whole call ([anthropic-claude.ts:154][c-claude-buf]), and the `readline` clients accept lines of any length.
   - Pervue caps a line at 8 MiB, reads at most 16 chunks ahead, and keeps only a small stderr tail that it never logs. The hostile matrix shows peak memory grows by at most 64 MiB during a 128 MiB stderr flood.
3. **Stopping a call is best effort.**
   - Claude, Grok, and the Codex app-server get one SIGTERM to the direct child, with no escalation ([anthropic-claude.ts:139][c-claude-kill], [acp-client.ts:150][c-grok-kill]). A CLI that ignores the signal, or a Node process it started, outlives the call.
   - Pervue signals the process group, escalates to SIGKILL after a grace period, reaps the child, and kills whatever the child left in its group. Dropping the handle does the same.
4. **The environment is a denylist.**
   - A child inherits every variable not named, including other vendors' keys, `NODE_OPTIONS`, and `LD_PRELOAD`. The Codex app-server inherits the server's whole environment ([app-server-client.ts:64][c-codex-env]).
   - Pervue passes a short allowlist plus each provider's own settings.
5. **Claude and Codex run in the server's working directory.**
   - Project instruction files there can apply to every council call: `CLAUDE.md` for Claude, `AGENTS.md` for Codex, and for Codex also those in directories above it up to the git root. Conclave passes `--safe-mode` to Claude, which may limit this for Claude; its Codex calls have no equivalent.
   - Pervue runs each provider in an empty private workspace whose ownership it checks. Conclave's Gemini adapter already does something similar.
6. **Executables are found through the server's `PATH`** (`spawn("claude")`). Pervue resolves an absolute path from fixed install locations and allows an explicit override.
7. **Failures are typed.** Every Pervue failure has a `code`, a `reason`, and a `retryable` flag, so handling rate limits and transient errors doesn't depend on how an error message is worded.
8. **Test assets.**
   - A fake provider binary with Codex and Claude personas.
   - The hostile-process matrix (TST-04).
   - The cross-provider contract suite (TST-10).

   All three can run against the sidecar.

### 2.2 What Pervue gains from Conclave's code

1. **Codex app-server.**
   - It streams token-level deltas. `codex exec` delivers each message whole ([native/README](../../native/README.md#codex)).
   - It exposes the live model list (`model/list`), subscription usage windows (`account/rateLimits/read`), and the account type (`account/read`).
   - It has `turn/interrupt`, which stops a turn without killing the process.
2. **Knowing how the CLI is billed.**
   - Pervue's environment allowlist keeps API keys in environment variables away from providers. It doesn't cover a CLI that was itself signed in with an API key or a Console account.
   - Conclave detects that case. The library can classify the sign-in method without keeping account identifiers, and each product decides what to do with the result.
3. **Grok and Gemini adapters.** Conclave has already worked out the operational details: the lockdown environment flags, denying ACP permission requests, and the text-only Antigravity agent definition with its init-event check. They are the reference for PRO-08 and PRO-09.
4. **Token usage events**, which `Update` doesn't have yet.
5. **A second consumer** that tests the shape of the API, which is what the extraction rule asks for.

## 3. Crossing the language boundary

Four ways Conclave could use a Rust library:

| Option | How Conclave uses it |
|---|---|
| **Sidecar executable** with a versioned stdio protocol | Starts one runtime process and speaks JSON-RPC over NDJSON, the same pattern as its Codex and Grok clients |
| **Node addon** (napi-rs) | Imports a `.node` module |
| **C ABI** (the path ADR-0001 expected) | Loads it through an FFI package |
| **Shared contract, TypeScript port** | Reimplements the same specification in TypeScript |

**Sidecar executable**
- For:
  - No FFI and no `unsafe` code.
  - A crash in the runtime can't take down the Fastify server and every run it owns.
  - It reuses the host's multiplexing, cancellation, timeouts, and fairness, and the hostile-process tests cover it end to end.
  - Any language can use it.
  - Pervue's release workflows already build macOS and Windows binaries.
- Against:
  - A binary has to reach `pnpm dev` users.
  - A second protocol to version.
  - One more process hop, which is negligible next to model latency.

**Node addon (napi-rs)**
- For:
  - Runs in-process, with an idiomatic JavaScript API.
- Against:
  - `pervue-core` is synchronous and polls against deadlines, using helper threads. Bridging that to libuv needs threadsafe-function glue.
  - The binding crate needs an exemption from the workspace's `forbid(unsafe_code)`.
  - A panic or abort kills the server.
  - Reaping children inside Node shares the process with libuv's own `SIGCHLD` handling, an interaction nobody has tested.
  - It still needs a prebuilt binary for each platform.

**C ABI**
- For:
  - Language-neutral in principle.
- Against:
  - Everything that counts against the addon.
  - Manual ownership.
  - A versioned C façade with cross-version tests ([core/README](../../native/core/README.md#compatibility-and-extraction)).

**Shared contract, TypeScript port**
- For:
  - There's no binary to ship.
- Against:
  - Two implementations of the hardest code, which will drift apart.
  - Node reaps children itself, so some guarantees can't be ported. One example: killing the process group before reaping the child, so the group's ID can't be reused in between.

**Recommendation:** the sidecar. Record the decision as ADR-0002, superseding ADR-0001's sentence about a C ABI for non-Rust consumers.

## 4. Proposed shape

```text
provider-runtime (working name)
├─ core        process, stream, discovery, exchange,        today's pervue-core, minus framing
│              errors, capabilities
├─ platform    environment allowlist, private workspace     moved from pervue-host and parameterized
│              and its checks, session-map store,           by an app namespace
│              removing provider transcripts
├─ providers   claude, codex-exec; later codex-app-server,  configured by the app that embeds them,
│              grok-acp, gemini-agy                         never by request text
├─ scheduler   in-flight table, start/idle timeouts,        extracted from host.rs
│              cancel → stop → kill, fairness, delta split
├─ runtime     the sidecar binary: JSON-RPC over stdio, on top of the scheduler
└─ clients/ts  typed client, with types generated from the protocol schema

pervue-host  = scheduler + providers + framing, origin checks, manifest, protocol v1,
               browser context, diagnostics
conclave     = clients/ts + its own orchestration, budgets, persistence, and search
```

**What stays in Pervue:**
- Native Messaging framing, which moves back into `pervue-host` with its fuzz target.
- Origin checks and the manifest.
- The protocol-v1 validator.
- Browser context and how it is framed in prompts.
- Diagnostics records and redaction rules.

**What stays in Conclave:**
- Orchestration modes, budgets, and retries.
- Run persistence and replay.
- SearXNG evidence.
- How models are presented.

The two products sanitize search results in similar ways, but those are small pure functions in two languages, and a round trip through the sidecar would cost more than sharing them saves.

## 5. API changes a second consumer needs

**Error wording**
- Today: `ErrorBody<'static>` carries Pervue's wording, such as "Pervue couldn't prepare a private folder…".
- Needed: the library returns only `code`, `reason`, and `retryable`, and each app writes its own messages.
- Why: `pervue-host` keeps a table of messages, so protocol v1 doesn't change.

**Error codes and capabilities**
- Today: `ErrorCode` and `Capabilities` mix runtime concepts with browser ones: `HostNotInstalled`, `ContextUnavailable`, `page_context`.
- Needed: runtime concepts in the library, marked `#[non_exhaustive]`. Browser concepts in the host.
- Why: the library can add codes without breaking semver.

**Models**
- Today: `ModelOption` holds `&'static str` values from a fixed list.
- Needed: owned values, discovered at run time.
- Why: Codex `model/list` and `agy models` are live catalogs.

**Usage**
- Today: `Update` has no usage.
- Needed: `Update::Usage { input_tokens, output_tokens }`.
- Why: Conclave's budgets and run inspector use it.

**Requests**
- Today: `SendRequest` has `text`, `history`, browser `context`, and `native_search`.
- Needed: a neutral turn with `system`, `messages`, `model`, a tool policy (`None` or `NativeWebSearch`), and persistence (`Ephemeral` or `Managed`).
- Why: Conclave needs a system prompt and no persistence. Pervue frames its browser context into messages before it calls the library.

**Paths**
- Today: paths and thread names are hard-coded under `pervue`.
- Needed: an app namespace, such as `pervue` or `conclave`.
- Why: each app gets its own workspaces and session stores. Removal proves ownership by the workspace a transcript ran in, so Pervue can never delete Conclave's transcripts, and the other way round.

**Sign-in**
- Today: the status check reads only the exit code.
- Needed: an optional sign-in classification (`Subscription`, `ApiKey`, `Cloud`, `Unknown`) that keeps no identifiers.
- Why: Conclave's subscription-only policy. Refusing a sign-in stays the app's decision.

**Timeouts**
- Today: each adapter has fixed timeout constants.
- Needed: overrides from the app, within ceilings.
- Why: Conclave's `CONCLAVE_*_TIMEOUT_MS` settings and its stall watchdog.

The two products' Claude command lines should also be reconciled once. Each product found something the other missed:
- Pervue uses `--strict-mcp-config`, stream-json input, and `DISABLE_AUTOUPDATER`.
- Conclave uses `--no-session-persistence`, `--max-turns 1`, `--system-prompt`, `--disable-slash-commands`, and `--safe-mode`.

## 6. Sidecar protocol sketch

The protocol is JSON-RPC 2.0 with one message per line. Conclave already has two clients in this style ([app-server-client.ts][c-codex-client], [acp-client.ts][c-grok-client]). Chrome's framing, with its length prefix in native byte order, is a browser detail that doesn't belong here.

```text
→ {"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocol":1,"namespace":"conclave"}}
← {"jsonrpc":"2.0","id":1,"result":{"protocol":1,"runtime":"0.1.0","providers":["claude","codex"]}}
→ {"jsonrpc":"2.0","id":2,"method":"turn/start","params":{"turn":"t_7f3a","provider":"claude","model":"sonnet",
     "system":"…","messages":[{"role":"user","text":"…"}],"tools":"none","persistence":"ephemeral"}}
← {"jsonrpc":"2.0","id":2,"result":{}}
← {"jsonrpc":"2.0","method":"turn/event","params":{"turn":"t_7f3a","type":"delta","text":"…"}}
← {"jsonrpc":"2.0","method":"turn/event","params":{"turn":"t_7f3a","type":"usage","input_tokens":812,"output_tokens":95}}
← {"jsonrpc":"2.0","method":"turn/ended","params":{"turn":"t_7f3a","outcome":"completed"}}
→ {"jsonrpc":"2.0","id":3,"method":"turn/cancel","params":{"turn":"t_9c01"}}
← {"jsonrpc":"2.0","method":"turn/ended","params":{"turn":"t_9c01","outcome":"failed",
     "error":{"code":"REQUEST_CANCELLED","reason":"USER_CANCELLED","retryable":false}}}
```

- **Methods:** `initialize`, `provider/status`, `provider/models`, `turn/start`, `turn/cancel`, and `shutdown`.
- **Notifications:**
  - `turn/event` reports `started`, `delta`, `activity`, `source`, and `usage`.
  - `turn/ended` follows, exactly once per turn.
- **Turn IDs come from the client**, so a cancel can't race the start of its turn.
- **Lines are bounded.** The runtime reads them with the existing `LineSplitter`.
- **End of input** cancels every turn and exits, as Pervue does with `INPUT_CLOSED`.
- **Diagnostics on stderr** hold no prompts, output, or account identifiers, as in OBS-01.
- **The protocol has its own version**, separate from the crate version and from Pervue protocol v1. `initialize` refuses a mismatched version.

## 7. Integrating Conclave

**Adapter.** A `RuntimeProvider` implements Conclave's `ProviderAdapter`. `generate` sends `turn/start`, and runtime events map to Conclave events:

| Runtime event | Conclave event |
|---|---|
| `delta` | `text_delta` |
| `usage` | `usage` |
| `source` | `citation` |
| `activity` | A new `progress` event that the UI doesn't show, so the stall watchdog re-arms without extra noise |

Aborting the `AbortSignal` sends `turn/cancel`.

**Errors.**
- A failed turn rejects with `RuntimeError extends Error { code, reason, retryable }`.
- `isRetryableStepError` and `isRateLimitError` check for it first.
- The regular expressions stay as the fallback for adapters that haven't moved.

**Sidecar lifecycle.**
- The server runs one runtime process, started on first use.
- If that process exits, its in-flight turns fail as retryable, so Conclave's existing single retry covers the crash.
- The next call starts a new process.

**Finding the binary.**
- During development, `CONCLAVE_RUNTIME_BIN` points at it.
- Later, per-platform npm packages ship it as `optionalDependencies`, the pattern esbuild and swc use. Pervue's existing release workflows can build them.

**Rollout.**
- Each provider moves separately, behind a flag such as `CONCLAVE_RUNTIME_PROVIDERS=anthropic`.
- The TypeScript adapter stays the default until the parity checks pass.
- The mock provider stays in TypeScript.

**Parity checks for Claude.** Through the runtime, Claude must match Conclave's current behavior on:
- refusing sign-ins that aren't subscriptions;
- the `sonnet`, `opus`, and `haiku` aliases;
- usage events;
- no tools;
- no session persistence;
- the system prompt;
- cancellation;
- the same failures in the run inspector.

## 8. Phases

The tracker isn't edited here, and the IDs below are only proposals.

**Phase 0: agree.** Nothing in this phase changes code.
- Review this proposal.
- Write ADR-0002 (a sidecar instead of a C ABI).
- Choose a name, a license, and where the code lives.

**Phase 1: additive work in `native/`** (proposed LIB-06 to LIB-09).
1. Consolidate the copied adapter plumbing.
2. Introduce the neutral types from §5.
3. Extract the scheduler from `host.rs` without changing behavior. The protocol validator and golden fixtures must stay green.
4. Build the sidecar binary and its schema, and run the hostile matrix and the contract suite through it.

**Phase 2: Conclave on Claude** (work in the Conclave repository).
1. Build the TypeScript client.
2. Route Claude through the runtime behind the flag, with typed errors.
3. Make the runtime the default once the parity checks pass.

**Phase 3: Codex app-server** (proposed PRO-10).
- Add the app-server adapter to the library and move Conclave to it.
- Moving Pervue off `exec` is a separate decision. It needs its own security review of the sandbox, the workspace, and removing app-server threads.

**Phase 4: more providers, then split** (PRO-08, PRO-09).
- Port the Grok and Gemini adapters from Conclave.
- Move the library to its own repository.
- Publish the crates and npm packages.

**Working alongside the tracker.** Phase 1 changes both adapters and `host.rs`, the host's largest and most heavily tested file.
- It should start after LIB-01 to LIB-05 are verified, so nothing is restructured while it is being verified.
- Until then, coordinate it with whoever holds the remaining tracker items.

## 9. Risks

- **Premature generalization.**
  - Move one provider at a time.
  - Add the Codex app-server adapter only when both products want it.
  - Keep framing, browser context, and conversation removal out of the shared API until a second consumer needs them.
- **Regressions from extracting the scheduler.**
  - `host.rs` holds protocol v1's lifecycle guarantees, so move it without changing behavior.
  - Keep TST-03, TST-04, and the golden fixtures as the gate.
- **Two protocols to version.** Each gets its own version number and handshake.
- **Shipping a binary to a `pnpm dev` project.** Until prebuilt packages exist, contributors who want the runtime need Rust. The TypeScript adapters stay as the fallback.
- **Stricter defaults for Conclave.**
  - The environment allowlist may drop a variable someone relies on, so each provider can extend the list.
  - A private workspace means project `CLAUDE.md` files no longer apply. That's intended, but users should be told.
- **Windows.**
  - There is no Job Object yet ([process.rs][p-process]), so only the provider process itself is stopped, not its descendants. Conclave has the same gap today.
  - Don't claim process-tree kills on Windows.
- **Licensing.** Neither repository has a license. Choose one before publishing crates or npm packages.

## 10. Open decisions

1. A sidecar or a Node addon? The recommendation is the sidecar.
2. Incubate in `native/` and split later (recommended), or start a new repository now?
3. The library's name. `provider-runtime` is a placeholder.
4. The license.
5. Should the library's Codex adapter move to app-server (recommended), or keep only `exec`?
6. Does Conclave accept Pervue's stricter defaults, the environment allowlist and the private workspace, as they are?

[p-claude]: ../../native/host/src/providers/claude/mod.rs
[p-codex]: ../../native/host/src/providers/codex/mod.rs
[p-env]: ../../native/host/src/providers/environment.rs
[p-workspace]: ../../native/host/src/providers/codex/workspace.rs
[p-forget]: ../../native/host/src/providers/forget.rs
[p-host]: ../../native/host/src/host.rs
[p-process]: ../../native/core/src/process.rs
[c-core]: https://github.com/davletovb/conclave/blob/1379a37ccb13d484247122ee215257e642b166fc/packages/core/src/index.ts
[c-classify]: https://github.com/davletovb/conclave/blob/1379a37ccb13d484247122ee215257e642b166fc/apps/server/src/orchestrator.ts#L98-L130
[c-watchdog]: https://github.com/davletovb/conclave/blob/1379a37ccb13d484247122ee215257e642b166fc/apps/server/src/orchestrator.ts#L507
[c-claude-argv]: https://github.com/davletovb/conclave/blob/1379a37ccb13d484247122ee215257e642b166fc/apps/server/src/providers/anthropic-claude.ts#L254
[c-claude-buf]: https://github.com/davletovb/conclave/blob/1379a37ccb13d484247122ee215257e642b166fc/apps/server/src/providers/anthropic-claude.ts#L154
[c-claude-kill]: https://github.com/davletovb/conclave/blob/1379a37ccb13d484247122ee215257e642b166fc/apps/server/src/providers/anthropic-claude.ts#L139
[c-grok-kill]: https://github.com/davletovb/conclave/blob/1379a37ccb13d484247122ee215257e642b166fc/apps/server/src/grok/acp-client.ts#L150
[c-codex-env]: https://github.com/davletovb/conclave/blob/1379a37ccb13d484247122ee215257e642b166fc/apps/server/src/codex/app-server-client.ts#L64
[c-codex-client]: https://github.com/davletovb/conclave/blob/1379a37ccb13d484247122ee215257e642b166fc/apps/server/src/codex/app-server-client.ts
[c-grok-client]: https://github.com/davletovb/conclave/blob/1379a37ccb13d484247122ee215257e642b166fc/apps/server/src/grok/acp-client.ts
