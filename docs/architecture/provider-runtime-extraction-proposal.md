# Proposal: a shared provider runtime for Pervue and Conclave

**Status:** Proposed, for discussion  
**Date:** 2026-09-27  
**Revised:** 2026-09-28, after review (see §11)  
**Scope:** extracting a reusable library from `native/`, with [Conclave](https://github.com/davletovb/conclave) as its first outside consumer  
**Evidence:** Pervue at `2cc35ef`, Conclave at `1379a37`

## Summary

- **Conclave justifies a new boundary between applications.** Pervue already extracted `pervue-core` under the framework's two-provider rule (§9.6). Conclave doesn't justify reuse for the first time. It justifies a *provider runtime* that two applications share.
  - The runtime owns provider execution: running an authenticated AI CLI safely, streaming its answer, and stopping it.
  - Each application keeps its conversation and product policy.
  - Native Messaging, browser context, provider sessions, and protocol v1 stay in Pervue.
- **Use a sidecar process, not linked code.** Conclave is a TypeScript/Node server. It should run the runtime as a **sidecar executable speaking a versioned JSON-RPC protocol over stdio**, not load it through a C ABI or a Node addon. ADR-0001 expected a C ABI for non-Rust consumers, so this decision would be recorded as ADR-0002.
- **Start with Claude.** Both products drive `claude -p` with stream-json, so Pervue's adapter already covers most of what Conclave needs. It also replaces Conclave's weakest adapter (§2.1).
- **Treat the two Codex paths as separate backends.** Conclave uses `codex app-server`, while Pervue uses `codex exec`. Their lifecycles and capabilities differ. The runtime should add `codex-app-server` as a backend of its own, next to `codex-exec`, not as a replacement for it.
- **Grok and Gemini knowledge flows the other way.** Conclave's working TypeScript adapters are the reference for Pervue's backlog items PRO-08 and PRO-09.
- **Keep the first step small, and incubate in `native/`.**
  - Promote only what Claude in Conclave needs.
  - Don't move files while the remaining tracker items are in flight.
  - The runtime gets its own repository once Conclave runs on it.

## 1. What exists today

### Pervue

`pervue-core` already holds the primitives that Codex and Claude share ([core/README](../../native/core/README.md)): `process`, `stream`, `discovery`, `exchange`, `protocol`, and `framing`. The host holds the rest of the provider plumbing, and some of it is copied between the two adapters:

| Host-owned piece | Where | Reuse today |
|---|---|---|
| Conversation→session map (`session_name`, read, save, forget, `new_conversation_id`) | [claude/mod.rs][p-claude], [codex/mod.rs][p-codex] | Each adapter has its own copy |
| Status probe, sign-in from the exit status, `PATH` for npm shims, data-directory lookup | [claude/mod.rs][p-claude], [codex/mod.rs][p-codex] | Each adapter has its own copy |
| Environment allowlist | [providers/environment.rs][p-env] | Shared |
| Private workspace and its ownership checks | [codex/workspace.rs][p-workspace] | Claude imports Codex's module |
| Removing provider transcripts | [providers/forget.rs][p-forget] | Shared |
| Request loop: in-flight table, start/idle timeouts, cancel → stop → kill, fairness, delta splitting | [host.rs][p-host] (`Session`, `Running`) | Tied to writing protocol-v1 events |

Merging the copied session code is worth doing inside the host. That cleanup doesn't justify moving it into the runtime: Conclave has no provider sessions (§4).

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
- bounds every call absolutely, with a 180-second `CONCLAVE_*_TURN_TIMEOUT_MS` per adapter;
- re-arms a 180-second stall watchdog on every provider event ([orchestrator.ts:507][c-watchdog]);
- treats usage reports as cumulative snapshots and adds the increase over the previous one ([orchestrator.ts:437–455][c-usage]).

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
   - Conclave detects that case. The runtime can classify the sign-in method without keeping account identifiers, and each product decides what to do with the result.
3. **Grok and Gemini adapters.** Conclave has already worked out the operational details: the lockdown environment flags, denying ACP permission requests, and the text-only Antigravity agent definition with its init-event check. They are the reference for PRO-08 and PRO-09.
4. **Token usage events**, which `Update` doesn't have yet.
5. **A second application** to test the runtime's API against, so it isn't shaped only by Pervue's needs.

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
  - Losing the runtime process in the middle of a turn has to be handled (§7).

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

## 4. Proposed boundary

The runtime owns **provider execution mechanics**. The applications keep **conversation and product policy**.

```text
provider-runtime (working name)
├─ core        process, stream, discovery; neutral turn, error, and usage types
├─ platform    environment construction (a base allowlist plus each provider's
│              variables), private-workspace primitives
├─ providers   claude; later codex-app-server, grok-acp, gemini-agy
├─ scheduler   concurrent turns, start/idle/absolute timeouts,
│              cancel → stop → kill, fairness, delta splitting
└─ sidecar     bounded, versioned JSON-RPC over stdio; a TypeScript client beside it

pervue-host
├─ provider-runtime, linked as a crate
├─ the codex-exec provider, until another application needs it
├─ conversation→session map, stale-session policy, transcript removal
├─ Native Messaging framing, origin checks, manifest, protocol v1
├─ browser context policy
└─ diagnostics and user-facing messages

conclave
├─ the sidecar client
├─ orchestration, retries, and budgets
├─ run persistence
└─ SearXNG search
```

**What moves, and why.**
- **The promotion rule.** Something moves into the runtime only when a second application uses it. Conclave runs every call ephemerally and reaches Codex through app-server. That's why session maps, transcript removal, and `codex-exec` stay in `pervue-host`.
- **Continuation.**
  - The runtime's Claude provider accepts an opaque native-session handle to resume, and reports the native session it ran.
  - When that session no longer exists, it fails with a reason of its own before producing any output.
  - `pervue-host` keeps the map, its `conv_` IDs, and the retry-once-with-history policy.
  - Claude's session IDs will then cross a crate boundary they don't cross today, but only as opaque values.
  - The sidecar protocol doesn't expose continuation until a consumer needs it.
- **Environment construction.**
  - The core README lists the credential allowlist as host policy, so moving it changes that boundary deliberately.
  - Conclave needs the same protection (§2.1, item 4), and a Claude provider in the runtime can't start without its variables.
  - Applications can extend the list.
- **Codex backends.**
  - `codex-exec` and `codex-app-server` stay separate: one is a one-shot process, the other a persistent server. They also differ in streaming granularity, model discovery, usage and rate-limit data, and interruption.
  - A product can present either one as "OpenAI via Codex".
  - The runtime treats them as separate until app-server reaches parity.

The two products sanitize search results in similar ways. Those are small pure functions in two languages, though, and a round trip through the sidecar would cost more than sharing them saves.

## 5. API changes a second application needs

**Error wording**
- Today: `ErrorBody<'static>` carries Pervue's wording, such as "Pervue couldn't prepare a private folder…".
- Needed: the runtime returns only `code`, `reason`, and `retryable`, and each app writes its own messages.
- Why: `pervue-host` keeps a table of messages, so protocol v1 doesn't change.

**Error codes and capabilities**
- Today: `ErrorCode` and `Capabilities` mix runtime concepts with browser ones: `HostNotInstalled`, `ContextUnavailable`, `page_context`.
- Needed: runtime concepts in the runtime, marked `#[non_exhaustive]`. Browser concepts in the host.
- Why: the runtime can add codes without breaking semver.

**Models**
- Today: `ModelOption` holds `&'static str` values from a fixed list.
- Needed: owned values, discovered at run time.
- Why: Codex `model/list` and `agy models` are live catalogs.

**Usage**
- Today: `Update` has no usage.
- Needed: `Update::Usage`, a cumulative snapshot for the whole turn.
  - Both `input_tokens` and `output_tokens` are optional.
  - Neither value is ever lower than in the previous snapshot, and consumers keep the latest.
  - An adapter whose provider reports usage per message adds those reports up itself.
- Why: Conclave's budgets and run inspector depend on it, and its orchestrator already reads usage this way ([orchestrator.ts:437–455][c-usage]).

**Requests**
- Today: `SendRequest` has `text`, `history`, browser `context`, and `native_search`.
- Needed: a neutral turn with `system`, `messages`, `model`, a tool policy (`None` or `NativeWebSearch`), and an optional continuation handle (§4).
- Why: Conclave needs a system prompt and runs every call ephemerally. Pervue frames its browser context into messages before it calls the runtime.

**Namespace**
- Today: paths and thread names are hard-coded under `pervue`.
- Needed: an application namespace, fixed when the runtime starts.
  - `pervue-host` passes a constant.
  - The sidecar takes `--namespace` on its command line and refuses Pervue's namespace.
  - `initialize` reports the namespace but can't change it.
- Why: each application gets its own workspaces, so a mistake in one product can't touch the other's files. This protects against accidents, not attacks: any process running as the user can already write to both products' directories.

**Sign-in**
- Today: the status check reads only the exit code.
- Needed: an optional sign-in classification (`Subscription`, `ApiKey`, `Cloud`, `Unknown`) that keeps no identifiers.
- Why: Conclave's subscription-only policy. Refusing a sign-in stays the application's decision.

**Timeouts**
- Today: each adapter has fixed start, idle, and stop-grace constants, and nothing limits a turn's total duration.
- Needed:
  - An absolute `max_turn` limit alongside the others. Applications can override all of them, within ceilings.
  - A fixed rule for what resets the idle timer: deltas, sources, and provider events that the adapter recognizes as work. stderr and unrecognized output never reset it.
- Why:
  - Pervue's Claude adapter counts any unrecognized `stream_event` as progress ([claude/output.rs][p-claude-output]). A Claude build that keeps emitting such events keeps its turn alive indefinitely.
  - That is tolerable in a popup the user can close. It isn't in a server-owned run that nobody is watching.
  - Conclave already bounds every call absolutely, and the runtime must preserve that.

The two products' Claude command lines should also be reconciled once. Each product found something the other missed:
- Pervue uses `--strict-mcp-config`, stream-json input, and `DISABLE_AUTOUPDATER`.
- Conclave uses `--no-session-persistence`, `--max-turns 1`, `--system-prompt`, `--disable-slash-commands`, and `--safe-mode`.

## 6. Sidecar protocol sketch

The protocol is JSON-RPC 2.0 with one message per line. Conclave already has two clients in this style ([app-server-client.ts][c-codex-client], [acp-client.ts][c-grok-client]). Chrome's framing, with its length prefix in native byte order, is a browser detail that doesn't belong here.

```text
$ provider-runtime --namespace conclave
→ {"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocol":1}}
← {"jsonrpc":"2.0","id":1,"result":{"protocol":1,"runtime":"0.1.0","namespace":"conclave","providers":["claude"]}}
→ {"jsonrpc":"2.0","id":2,"method":"turn/start","params":{"turn":"t_7f3a","provider":"claude","model":"sonnet",
     "system":"…","messages":[{"role":"user","text":"…"}],"tools":"none"}}
← {"jsonrpc":"2.0","id":2,"result":{}}
← {"jsonrpc":"2.0","method":"turn/event","params":{"turn":"t_7f3a","type":"delta","text":"…"}}
← {"jsonrpc":"2.0","method":"turn/event","params":{"turn":"t_7f3a","type":"usage","input_tokens":812,"output_tokens":95}}
← {"jsonrpc":"2.0","method":"turn/ended","params":{"turn":"t_7f3a","outcome":"completed"}}
→ {"jsonrpc":"2.0","id":3,"method":"turn/cancel","params":{"turn":"t_9c01"}}
← {"jsonrpc":"2.0","id":3,"result":{}}
← {"jsonrpc":"2.0","method":"turn/ended","params":{"turn":"t_9c01","outcome":"failed",
     "error":{"code":"REQUEST_CANCELLED","reason":"USER_CANCELLED","retryable":false}}}
→ {"jsonrpc":"2.0","id":4,"method":"turn/cancel","params":{"turn":"t_0000"}}
← {"jsonrpc":"2.0","id":4,"error":{"code":-32000,"message":"unknown turn","data":{"reason":"UNKNOWN_TURN"}}}
```

- **Methods.** `initialize`, `provider/status`, `provider/models`, `turn/start`, `turn/cancel`, and `shutdown`. Every method gets a response.
- **Notifications.**
  - `turn/event` reports `started`, `delta`, `activity`, `source`, and `usage`.
  - `turn/ended` follows each started turn.

**Turn lifecycle:**

1. **Order.**
   - The runtime reads its input in order.
   - `turn/start` registers the turn's ID before the runtime reads the next message or does any provider work. So a `turn/cancel` sent right after it always finds the turn.
   - The response to `turn/start` comes before any event of that turn.
2. **Turn IDs.**
   - A turn ID is unique for the life of the runtime process, not only while the turn is running, so a late cancel can never reach a newer turn.
   - A `turn/start` that reuses an ID fails with `DUPLICATE_TURN_ID`, and the turn that already holds the ID is unaffected.
3. **Exactly one ending.** A started turn gets exactly one `turn/ended`. A rejected `turn/start` gets an error response and no `turn/ended`.
4. **Cancellation.**
   - `turn/cancel` for a running turn is acknowledged, including repeats, and the turn still ends only once.
   - For a turn that never started or has already ended, `turn/cancel` fails with `UNKNOWN_TURN`.
   - If a turn ends just before its cancel arrives, the runtime has already written its `turn/ended` ahead of that error, so the client can ignore the error.

**Other rules:**
- **Lines are bounded.** The runtime reads them with the existing `LineSplitter`.
- **End of input** cancels every turn and exits, as Pervue does with `INPUT_CLOSED`.
- **Diagnostics on stderr** contain no prompts, output, or account identifiers, as in OBS-01.
- **Versioning.** The protocol has its own version, separate from the runtime's version and from Pervue protocol v1. `initialize` refuses a mismatched version.

## 7. Integrating Conclave

**Adapter.** A `RuntimeProvider` implements Conclave's `ProviderAdapter`. `generate` sends `turn/start`, and runtime events map to Conclave events:

| Runtime event | Conclave event |
|---|---|
| `delta` | `text_delta` |
| `usage` | `usage` |
| `source` | `citation` |
| `activity` | A new `progress` event that the UI doesn't show. It re-arms the stall watchdog without adding noise, and `max_turn` still bounds the call |

Aborting the `AbortSignal` sends `turn/cancel`.

**Errors.**
- A failed turn rejects with `RuntimeError extends Error { code, reason, retryable }`.
- `isRetryableStepError` and `isRateLimitError` check for it first.
- The regular expressions stay as the fallback for adapters that haven't moved.

**When the runtime process dies.**
- The client fails every running turn with a runtime-loss error that says how far the turn got:
  - `not_started` if the client never wrote the `turn/start` to the runtime;
  - otherwise `maybe_started`, because a runtime that received the request may have launched the provider, or even finished the turn, before it died.
- Conclave's retry policy decides what to do:
  - A `not_started` turn can be retried freely.
  - A `maybe_started` turn is retried only if the budget allows, and it counts as a new call, because the first attempt may already have used a model call and some of the rate limit.
- A runtime that is killed outright can't stop its providers, just as the host can't today ([native/README](../../native/README.md#provider-processes)). So a `maybe_started` provider may still be running.
- The next call starts a new runtime process.

**Release and compatibility.** These must be in place before Conclave makes the runtime its default:
- The runtime has its own version and release tags, separate from Pervue's application releases, even while it lives in this repository.
- `initialize` reports the runtime version and the protocol version.
- After a new protocol version ships, the runtime keeps supporting the previous one for a documented period.
- The per-platform npm packages carry the runtime's version.
- Conclave pins an exact runtime version in its lockfile, and declares the protocol version it speaks.
- Conclave's CI runs its runtime-backed tests against the pinned version.

**Finding the binary.**
- During development, `CONCLAVE_RUNTIME_BIN` points at it.
- Later, per-platform npm packages ship it as `optionalDependencies`, the pattern esbuild and swc use.

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
- the absolute turn timeout;
- cancellation;
- the same failures in the run inspector.

## 8. Phases

The tracker isn't edited here, and the IDs below are only proposals. Each phase promotes only what the next real use needs.

**Phase 0: agree.** Nothing in this phase changes code.
- Review this proposal.
- Write ADR-0002 (a sidecar instead of a C ABI).
- Choose a name, a license, and where the code lives.

**Phase 1: Claude through the runtime** (proposed LIB-06 to LIB-09).
1. Introduce the neutral turn, error, usage, and timeout types from §5.
2. Move the Claude provider into the runtime, along with only the mechanics it needs: environment construction, private-workspace primitives, and the status probe. The continuation hook comes with it. `pervue-host` keeps its sessions and transcript removal, and all of its tests pass unchanged.
3. Extract the scheduler from `host.rs`, and add the absolute limit and the rule for what counts as progress. The protocol validator and golden fixtures must stay green.
4. Build the sidecar, serving Claude and the fake provider, and run the hostile matrix and the contract suite through it.

**Phase 2: Conclave on Claude** (work in the Conclave repository).
1. Build the TypeScript client.
2. Route Claude through the runtime behind the flag, with typed errors and runtime-loss handling.
3. Put release and compatibility pinning in place.
4. Make the runtime the default.

**Phase 3: Codex app-server** (proposed PRO-10).
- Add `codex-app-server` to the runtime as a backend of its own, and move Conclave to it.
- `codex-exec` stays in `pervue-host`. Whether Pervue ever switches is a separate decision, with its own security review of the sandbox, the workspace, and removing app-server threads.

**Phase 4: more providers, then split** (PRO-08, PRO-09).
- Port the Grok and Gemini adapters from Conclave.
- Move the runtime to its own repository.
- Publish the crates and npm packages.

Merging Pervue's copied session code (§1) is cleanup inside the host. It can happen at any time and stays in the host.

**Working alongside the tracker.** Phase 1 changes the Claude adapter and `host.rs`, the host's largest and most heavily tested file.
- It should start after LIB-01 to LIB-05 are verified, so nothing is restructured while it is being verified.
- Until then, coordinate it with whoever holds the remaining tracker items.

## 9. Risks

- **Premature generalization.** The §4 promotion rule decides what moves. Move one provider at a time, and keep policy owned by a single application in that application.
- **Regressions from extracting the scheduler.**
  - `host.rs` holds protocol v1's lifecycle guarantees, so move it without changing behavior.
  - Keep TST-03, TST-04, and the golden fixtures as the gate.
- **Losing the runtime in the middle of a turn.** A retry can repeat work that already happened. Runtime-loss errors say whether the turn may have started, and the application decides (§7).
- **Two protocols to version.** Each gets its own version number and handshake, and Conclave pins the runtime (§7).
- **Shipping a binary to a `pnpm dev` project.** Until prebuilt packages exist, contributors who want the runtime need Rust. The TypeScript adapters stay as the fallback.
- **Stricter defaults for Conclave.**
  - The environment allowlist may drop a variable someone relies on, so each provider can extend it.
  - A private workspace means project `CLAUDE.md` files no longer apply. That's intended, but users should be told.
- **Windows.**
  - There is no Job Object yet ([process.rs][p-process]), so only the provider process itself is stopped, not its descendants. Conclave has the same gap today.
  - Don't claim process-tree kills on Windows.
- **Licensing.** Neither repository has a license. Choose one before publishing crates or npm packages.

## 10. Open decisions

1. A sidecar or a Node addon? The recommendation is the sidecar.
2. Incubate in `native/` and split later (recommended), or start a new repository now?
3. The runtime's name. `provider-runtime` is a placeholder.
4. The license.
5. Should Pervue ever move from `codex-exec` to `codex-app-server`? The runtime can carry both.
6. Does Conclave accept Pervue's stricter defaults, the environment allowlist and the private workspace, as they are?
7. What should the default absolute turn limit be, and can applications raise it? Conclave uses 180 s today, and Pervue has no limit.

## 11. Changes after review

A review on 2026-09-28 checked this proposal against both repositories at the evidence commits. It led to these changes:

- **Framing.** Conclave justifies a new boundary between applications. It doesn't satisfy the extraction rule for the first time, because Pervue already met it with two providers.
- **Boundary.**
  - Session maps, managed continuation, and transcript removal stay in `pervue-host`. The runtime offers a continuation hook instead (§4).
  - `codex-exec` also stays in the host until another application needs it.
- **Protocol.**
  - `turn/cancel` now gets a response.
  - The turn lifecycle is specified: when a turn is registered, when an ID may be reused, what happens with unknown or repeated cancellations, and that every started turn ends exactly once (§6).
- **Runtime loss.** Errors carry `not_started` or `maybe_started`, and retrying a `maybe_started` turn is the application's decision (§7).
- **Namespace.** It is fixed when the runtime starts, not chosen in `initialize`. It is described as protection against accidents, not a security boundary (§5).
- **Usage.** Defined as cumulative snapshots for each turn (§5).
- **Timeouts.** An absolute `max_turn` limit and a rule for what resets the idle timer (§5). Checking the code for this showed that Pervue's Claude adapter can be kept alive indefinitely today.
- **Codex.** `codex-exec` and `codex-app-server` are separate backends (§4).
- **Releases.** The runtime is versioned and pinned independently before Conclave makes it the default (§7).
- **Phases.** Phase 1 is now scoped to what Claude in Conclave needs (§8).

[p-claude]: ../../native/host/src/providers/claude/mod.rs
[p-claude-output]: ../../native/host/src/providers/claude/output.rs
[p-codex]: ../../native/host/src/providers/codex/mod.rs
[p-env]: ../../native/host/src/providers/environment.rs
[p-workspace]: ../../native/host/src/providers/codex/workspace.rs
[p-forget]: ../../native/host/src/providers/forget.rs
[p-host]: ../../native/host/src/host.rs
[p-process]: ../../native/core/src/process.rs
[c-core]: https://github.com/davletovb/conclave/blob/1379a37ccb13d484247122ee215257e642b166fc/packages/core/src/index.ts
[c-classify]: https://github.com/davletovb/conclave/blob/1379a37ccb13d484247122ee215257e642b166fc/apps/server/src/orchestrator.ts#L98-L130
[c-usage]: https://github.com/davletovb/conclave/blob/1379a37ccb13d484247122ee215257e642b166fc/apps/server/src/orchestrator.ts#L437-L455
[c-watchdog]: https://github.com/davletovb/conclave/blob/1379a37ccb13d484247122ee215257e642b166fc/apps/server/src/orchestrator.ts#L507
[c-claude-argv]: https://github.com/davletovb/conclave/blob/1379a37ccb13d484247122ee215257e642b166fc/apps/server/src/providers/anthropic-claude.ts#L254
[c-claude-buf]: https://github.com/davletovb/conclave/blob/1379a37ccb13d484247122ee215257e642b166fc/apps/server/src/providers/anthropic-claude.ts#L154
[c-claude-kill]: https://github.com/davletovb/conclave/blob/1379a37ccb13d484247122ee215257e642b166fc/apps/server/src/providers/anthropic-claude.ts#L139
[c-grok-kill]: https://github.com/davletovb/conclave/blob/1379a37ccb13d484247122ee215257e642b166fc/apps/server/src/grok/acp-client.ts#L150
[c-codex-env]: https://github.com/davletovb/conclave/blob/1379a37ccb13d484247122ee215257e642b166fc/apps/server/src/codex/app-server-client.ts#L64
[c-codex-client]: https://github.com/davletovb/conclave/blob/1379a37ccb13d484247122ee215257e642b166fc/apps/server/src/codex/app-server-client.ts
[c-grok-client]: https://github.com/davletovb/conclave/blob/1379a37ccb13d484247122ee215257e642b166fc/apps/server/src/grok/acp-client.ts
