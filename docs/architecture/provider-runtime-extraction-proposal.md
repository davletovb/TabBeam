# Proposal: a shared provider runtime for Pervue and Conclave

**Status:** Proposed, for discussion  
**Date:** 2026-09-27  
**Revised:** 2026-09-28 (see §12)  
**Scope:** extracting a reusable library from `native/`, with [Conclave](https://github.com/davletovb/conclave) as its first outside consumer  
**Evidence:** Pervue at `5eb0cdf` (three providers, including Gemini), Conclave at `1379a37`

## Summary

- **Conclave justifies a new boundary between applications.** Pervue already extracted `pervue-core` under the two-provider rule in [framework §9.6](../product/browser-ai-extension-framework.md#96-extraction-rule). Conclave doesn't justify reuse for the first time. It justifies a *provider runtime* that two applications share.
  - The runtime owns provider execution: running an authenticated AI CLI safely, streaming its answer, and stopping it.
  - Each application keeps its conversation and product policy.
  - Native Messaging, browser context, provider sessions, and protocol v1 stay in Pervue.
- **Use a sidecar process, not linked code.** Conclave is a TypeScript/Node server. It should run the runtime as a **sidecar executable speaking a versioned JSON-RPC protocol over stdio**, not load it through a C ABI or a Node addon. ADR-0001 expected a C ABI for non-Rust consumers, so this decision would be recorded as ADR-0002.
- **Every provider runs as one process per turn.**
  - The runtime doesn't use long-lived provider servers, so Codex runs through `codex exec` only and `codex app-server` isn't adopted.
  - This keeps cancelling a turn a kill, and confines a crash to one turn.
  - It also lets each turn choose its tools with launch flags (§4).
- **Pilot with Gemini, then Claude.**
  - Both apps drive Antigravity (`agy`) almost identically, and neither keeps sessions for it. Gemini therefore needs no continuation support, which makes it the smallest first step.
  - Claude follows, and brings Conclave the larger fixes (§2.1).
- **Codex is Conclave's call.**
  - Moving Conclave's OpenAI provider to `codex-exec` hardens it, but gives up token streaming, the live model list, and the ChatGPT usage panel.
  - Keeping its TypeScript app-server adapter is also valid, and the runtime doesn't depend on that choice.
- **Grok stays in Conclave.** Pervue deferred PRO-09, so nothing about Grok moves.
- **Speed work comes after every provider runs on the runtime.** First measure cold start and time to first output with real CLIs, then optimize in one place (§9).
- **Incubate in `native/`, then split.**
  - Promote only what the next migration needs.
  - Don't move files while the remaining tracker items are in flight.
  - The runtime gets its own repository once Conclave runs on it.

## 1. What exists today

### Pervue

`pervue-core` already holds the primitives the providers share ([core/README](../../native/core/README.md)): `process`, `stream`, `discovery`, `exchange`, `protocol`, and `framing`. The host holds the rest of the plumbing for its three real providers (Codex, Claude, and Gemini), and some of it is copied between adapters:

| Host-owned piece | Where | Reuse today |
|---|---|---|
| Conversation→session map (`session_name`, read, save, forget, `new_conversation_id`) | [claude/mod.rs][p-claude], [codex/mod.rs][p-codex] | Claude and Codex each have a copy. Gemini keeps no sessions but has its own copies of the ID helpers |
| Status probe, sign-in from the exit status, data-directory lookup | All three adapters | A copy in each |
| `PATH` for npm shims | [providers/environment.rs][p-env] (`search_path_for`) | Shared since Gemini landed |
| Environment allowlist | [providers/environment.rs][p-env] | Shared |
| Private workspace and its ownership checks | [codex/workspace.rs][p-workspace] | All three import Codex's module |
| Removing provider transcripts | [providers/forget.rs][p-forget], [gemini/mod.rs][p-gemini] | Helpers are shared. Claude and Codex remove transcripts when a conversation is deleted. Gemini removes them after every turn, and keeps durable records for retrying deletions that fail |
| Request loop: in-flight table, start/idle timeouts, cancel → stop → kill, fairness, delta splitting | [host.rs][p-host] (`Session`, `Running`) | Tied to writing protocol-v1 events |

Gemini landed without changing the `Provider` trait, `host.rs`, the protocol, or `pervue-core`. A third provider fitting the contract unchanged is good evidence that `Exchange` and `Update` are ready to become the runtime's API.

Merging the copied helpers is worth doing inside the host. That cleanup doesn't justify moving them into the runtime (§4).

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
6. **Every Gemini call leaves a transcript behind.**
   - Conclave deletes each call's temporary workspace but never touches `~/.gemini/antigravity-cli/brain/`.
   - Pervue's adapter deletes the transcript `agy` saves there after every turn, and keeps durable records to retry deletions that fail.
   - Pervue's agent definition also sets `hooks: []`; Conclave's doesn't.
7. **Executables are found through the server's `PATH`** (`spawn("claude")`). Pervue resolves an absolute path from fixed install locations and allows an explicit override.
8. **Failures are classified once, inside the adapter.**
   - Outside a Pervue adapter, every failure has a `code`, a `reason`, and a `retryable` flag, so code that handles rate limits and transient errors never reads error text.
   - Underneath, the adapters still classify by matching phrases in the provider's message ([codex/output.rs:128][p-codex-output], [claude/output.rs:211][p-claude-output]), because failures arrive as text.
   - Unlike Conclave's regular expressions, they match specific phrases rather than bare words such as "rate" or "auth", right next to the parser for that provider's output. The Codex matching is tested against output captured from Codex CLI 0.156.1.
9. **Test assets.**
   - A fake provider binary with Codex, Claude, and Gemini personas.
   - The hostile-process matrix (TST-04).
   - The cross-provider contract suite (TST-10).

   All three can run against the sidecar.

### 2.2 What Pervue gains from Conclave's code

1. **Knowing how each CLI is billed.**
   - Pervue's environment allowlist keeps API keys in environment variables away from providers. It doesn't cover a CLI that was itself signed in with an API key or a Console account.
   - Conclave checks the sign-in method for Claude by reading `claude auth status` JSON.
   - For Codex, Conclave uses app-server's `account/read`. With `exec`, the equivalent is classifying the output of `codex login status`, which Pervue currently doesn't read. That output's exact wording hasn't been checked.
   - For Gemini, both apps strip Google API-key variables. Conclave also recognizes Antigravity's direct API-key mode from its error messages.
2. **Usage and live model lists, which Pervue now needs for its own providers.**
   - Codex's `turn.completed` event and Antigravity's `result` event both report token counts. Pervue ignores them, because `Update` has no usage.
   - Pervue runs `agy models` to check Gemini's status, but can't expose the list, because `ModelOption` holds only fixed strings.
   - Conclave already uses both.
3. **A cross-check for Gemini.**
   - The two apps have independent implementations of the same Antigravity integration. They agree on the command-line flags, the agent definition, the accepted permission modes, and the `init` check.
   - The differences are worth reconciling. Pervue fails a turn when Antigravity reports a step type it doesn't document, while Conclave ignores such steps. Pervue deletes transcripts. Conclave reads usage.
4. **A second application** to test the runtime's API against, so it isn't shaped only by Pervue's needs.

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
│              variables), private-workspace primitives, per-turn cleanup
├─ providers   gemini (the pilot), claude, codex-exec
├─ scheduler   concurrent turns, start/idle/absolute timeouts,
│              cancel → stop → kill, fairness, delta splitting
└─ sidecar     bounded, versioned JSON-RPC over stdio; a TypeScript client beside it

pervue-host
├─ provider-runtime, linked as a crate
├─ conversation→session map, stale-session policy, and transcript
│  removal when a conversation is deleted
├─ Native Messaging framing, origin checks, manifest, protocol v1
├─ browser context policy
└─ diagnostics and user-facing messages

conclave
├─ the sidecar client
├─ the Grok adapter, and the OpenAI adapter if it stays on app-server
├─ orchestration, retries, and budgets
├─ run persistence
└─ SearXNG search
```

**What moves, and why.**
- **The promotion rule.** Something moves into the runtime only when a second application uses it.
  - Session maps and conversation-scoped transcript removal stay in `pervue-host`, because Conclave runs every call statelessly.
  - Grok stays in Conclave, because Pervue deferred it.
  - `codex-exec` moves only when Conclave's OpenAI provider does (§7). Until then it stays in `pervue-host`.
- **One process per turn, for every provider.**
  - Codex's app-server is the only CLI server that serves many conversations from one process. Grok's ACP mode can hold several sessions in one process, but Conclave runs one process per call. Claude can keep one conversation's process alive. Antigravity has no such mode that either app uses.
  - The runtime adopts none of them. Pervue chooses each turn's tools with launch flags: `--tools ""` or `--tools WebSearch` for Claude, the `pervue-text` or `pervue-search` agent for Gemini, and `-c` feature switches for Codex. A long-lived process would fix those at launch.
  - A process kept alive for a conversation would carry earlier turns' page text into later turns.
  - A crash of a shared process fails every running turn. Cancelling becomes a request the CLI has to honor, instead of a kill.
  - Pervue's host lives only as long as the extension's service worker keeps its connection ([native-connection.js][p-native-connection]), so a warm provider process wouldn't stay warm.
  - Speed is handled separately (§9).
- **Continuation.**
  - The runtime's Claude provider accepts an opaque native-session handle to resume, and reports the native session it ran.
  - When that session no longer exists, it fails with a reason of its own before producing any output.
  - `pervue-host` keeps the map, its `conv_` IDs, and the retry-once-with-history policy.
  - Claude's session IDs will then cross a crate boundary they don't cross today, but only as opaque values.
  - The sidecar protocol doesn't expose continuation until a consumer needs it.
  - Gemini keeps no sessions, so the pilot doesn't need this hook. The hook arrives with Claude.
- **Cleanup has two scopes.**
  - **Per-turn cleanup is execution**, so it moves with the provider:
    - Gemini's Antigravity transcript.
    - If Conclave moves to `codex-exec`, the session file `exec` saves for each stateless turn, deleted using the ownership check in `forget_rollouts`. Whether `exec` has a flag to skip saving hasn't been checked.
  - **Cleanup when a conversation is deleted is policy**, so it stays with the app. In Pervue, that means Claude's and Codex's transcripts.
  - **Retrying failed deletions.** Pervue records failed Gemini deletions under its conversation IDs and retries them through `conversation.forget`. The runtime needs its own record, per namespace, retried when the runtime starts and on request, so Conclave gets retries without having conversations to hang them on. Pervue's `forget` can trigger the same retry.
- **Environment construction.**
  - The core README lists the credential allowlist as host policy, so moving it changes that boundary deliberately.
  - Conclave needs the same protection (§2.1, item 4), and a provider in the runtime can't start without its variables.
  - Applications can extend the list.

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
- Why: `agy models` is a live catalog. Pervue already runs it for Gemini's status but can't expose it, and Conclave shows it in its model picker.

**Usage**
- Today: `Update` has no usage.
- Needed: `Update::Usage`, a cumulative snapshot for the whole turn.
  - Both `input_tokens` and `output_tokens` are optional.
  - Neither value is ever lower than in the previous snapshot, and consumers keep the latest.
  - An adapter whose provider reports usage per message adds those reports up itself.
- Why: Conclave's budgets and run inspector depend on it, and its orchestrator already reads usage this way ([orchestrator.ts:437–455][c-usage]). Codex's `turn.completed` and Antigravity's `result` each report once, at the end of the turn.

**Requests**
- Today: `SendRequest` has `text`, `history`, browser `context`, and `native_search`.
- Needed: a neutral turn with `system`, `messages`, `model`, a tool policy (`None` or `NativeWebSearch`), and an optional continuation handle (§4).
- Why: Conclave needs a system prompt and runs every call statelessly. Pervue frames its browser context into messages before it calls the runtime.

**Namespace**
- Today: paths and thread names are hard-coded under `pervue`.
- Needed: an application namespace, fixed when the runtime starts.
  - `pervue-host` passes a constant.
  - The sidecar takes `--namespace` on its command line and refuses Pervue's namespace.
  - `initialize` reports the namespace but can't change it.
- Why: each application gets its own workspaces and cleanup records, so a mistake in one product can't touch the other's files. This protects against accidents, not attacks: any process running as the user can already write to both products' directories.

**Sign-in**
- Today: the status check reads only the exit code.
- Needed: an optional sign-in classification (`Subscription`, `ApiKey`, `Cloud`, `Unknown`) that keeps no identifiers. For Codex, it comes from classifying the output of `codex login status`, because `exec` has no account API.
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

**Reconciling what each app learned.** Each product found something the other missed, in two places:
- **Claude command lines.** Pervue uses `--strict-mcp-config`, stream-json input, and `DISABLE_AUTOUPDATER`. Conclave uses `--no-session-persistence`, `--max-turns 1`, `--system-prompt`, `--disable-slash-commands`, and `--safe-mode`.
- **Gemini agent definitions and step handling.** See §2.2, item 3.

## 6. Sidecar protocol sketch

The protocol is JSON-RPC 2.0 with one message per line. Conclave already has two clients in this style ([app-server-client.ts][c-codex-client], [acp-client.ts][c-grok-client]). Chrome's framing, with its length prefix in native byte order, is a browser detail that doesn't belong here.

```text
$ provider-runtime --namespace conclave
→ {"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocol":1}}
← {"jsonrpc":"2.0","id":1,"result":{"protocol":1,"runtime":"0.1.0","namespace":"conclave","providers":["gemini","claude"]}}
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
- Each provider moves separately, behind a flag: `CONCLAVE_RUNTIME_PROVIDERS=google` for the pilot, then `google,anthropic`.
- Each TypeScript adapter stays the default until its parity checks pass.
- The mock provider stays in TypeScript.

**Parity checks for Gemini (the pilot).** Through the runtime, Gemini must match Conclave's current behavior on:
- listing `gemini-` models from `agy models`. Conclave keeps mapping its legacy aliases (`auto`, `pro`, `flash`, `flash-lite`) itself before calling the runtime.
- refusing Antigravity's direct Gemini API-key mode;
- the tool-free agent and the `init` checks;
- usage from the final `result`;
- the absolute turn timeout;
- cancellation;
- the same failures in the run inspector.

Conclave also gets three changes in behavior:
- transcripts are deleted after each call;
- the agent definition sets `hooks: []`;
- a turn fails when Antigravity reports a step type it doesn't document. This check failed real turns in Pervue until the 2026-09-28 fix.

**Parity checks for Claude (second).** Through the runtime, Claude must match Conclave's current behavior on:
- refusing sign-ins that aren't subscriptions;
- the `sonnet`, `opus`, and `haiku` aliases;
- usage events;
- no tools;
- no session persistence;
- the system prompt;
- the absolute turn timeout;
- cancellation;
- the same failures in the run inspector.

**OpenAI is Conclave's decision.**
- **Moving to `codex-exec`** gains the hardening in §2.1, but loses:
  - token-by-token streaming;
  - the live model list (`model/list`), so the model picker needs a list kept by hand;
  - the ChatGPT usage panel (`/provider-limits`, fed by `account/rateLimits/read`), which would show as unavailable.

  The subscription check moves to classifying `codex login status`.
- **Keeping the TypeScript app-server adapter** keeps those features, and also keeps that adapter's weaknesses.

## 8. Phases

The tracker isn't edited here, and the IDs below are only proposals. Each phase promotes only what the next real use needs.

**Phase 0: agree.** Nothing in this phase changes code.
- Review this proposal.
- Write ADR-0002 (a sidecar instead of a C ABI).
- Choose a name, a license, and where the code lives.

**Phase 1: Gemini through the runtime** (proposed LIB-06 to LIB-09).
1. Introduce the neutral turn, error, usage, and timeout types from §5.
2. Move the Gemini provider into the runtime, along with only the mechanics it needs:
   - environment construction;
   - private-workspace primitives;
   - per-turn transcript cleanup, with the runtime's own retry record.

   No continuation hook is needed. `pervue-host` keeps passing all of its tests.
3. Extract the scheduler from `host.rs`, and add the absolute limit and the rule for what counts as progress. The protocol validator and golden fixtures must stay green.
4. Build the sidecar, serving Gemini and the fake provider, and run the hostile matrix and the contract suite through it.

**Phase 2: Conclave on Gemini** (work in the Conclave repository).
1. Build the TypeScript client.
2. Route Gemini through the runtime behind the flag, with typed errors and runtime-loss handling.
3. Put release and compatibility pinning in place.
4. Make the runtime the default for Gemini.

**Phase 3: Claude.**
- Move the Claude provider into the runtime, with the continuation hook. `pervue-host` keeps its sessions and its transcript removal when a conversation is deleted.
- Route Conclave's Anthropic provider through the runtime behind the flag, then make the runtime its default.

**Phase 4: Codex, if Conclave chooses it.**
- Move `codex-exec` into the runtime, with per-turn cleanup of `exec`'s session files for stateless turns.
- Route Conclave's OpenAI provider through the runtime behind the flag.

**Phase 5: speed** (proposed PRF-01 to PRF-03). See §9.

**Phase 6: split and publish.**
- Move the runtime to its own repository.
- Publish the crates and npm packages.

Merging Pervue's copied helpers (§1) is cleanup inside the host. It can happen at any time and stays in the host.

**Working alongside the tracker.**
- Phase 1 should start only after LIB-01 to LIB-05 and PRO-08 are verified, so nothing is restructured while it is being verified.
- Pervue's Gemini adapter landed on 2026-09-28 and was fixed the same day for real `agy` output. It should settle before it moves.
- Phase 1 also changes `host.rs`, the host's largest and most heavily tested file. Coordinate that with whoever holds the remaining tracker items.

## 9. Speed and cold start

Optimize after every provider runs on the runtime. At that point the runtime is the one place where turns start, so a measurement or a fix covers every provider in both apps at once, instead of four adapters in two languages.

**What is measured today.**
- TST-09 sets three budgets ([performance.js][p-perf]):
  - the popup ready for input within 100 ms;
  - the native host ready within 250 ms;
  - the first response chunk within 1,500 ms.
- CI enforces the last two against the built host, but only with the fake provider. No real CLI's start-up is measured.

**What a turn costs today.**

| Provider | Processes started per turn | Other work before the provider starts |
|---|---|---|
| Codex (`exec`) | 2: `codex login status`, then `codex exec` ([codex/mod.rs:460][p-codex]) | Workspace ownership check |
| Claude | 2: `claude auth status`, then `claude -p` ([claude/mod.rs:433][p-claude]) | Workspace ownership check |
| Gemini | 1: `agy` | Creating a private workspace and writing the agent definition; workspace ownership check |

In Conclave, the runtime process itself also starts once. `codex exec` delivers each message whole, so Codex's first visible text waits for its first complete message. No process-level optimization changes that.

**Step 1: measure** (PRF-01).
- For each provider, with real CLIs, record a timeline of each turn: request received → sign-in check done → provider started → first output line → first answer text → turn ended → process exited.
- Record cold runs (the first turn after sign-in or a reboot) and warm runs (repeated turns).
- Run it as an opt-in benchmark, like the live smoke tests, because CI has no signed-in CLIs.

**Step 2: optimize** (PRF-02), roughly in order of expected gain for the risk:
1. **Stop paying for the sign-in check on every turn.** Either:
   - cache a successful check for a short time and re-check after any authentication failure; or
   - run the check alongside the turn instead of before it, and stop the turn if the check says signed out.

   Either one removes a process start from the critical path of every Codex and Claude turn. Codex needs the check because a signed-out `exec` keeps retrying instead of failing. A stale cache there is bounded by the start timeout.
2. **Prepare per-turn resources ahead of time**, such as Gemini's next private workspace and agent definition. The workspace ownership check must still run at launch, because caching it would open a race.
3. **Keep executable discovery across turns**, and look again only when a launch fails. Today each request does one lookup.
4. **Start the runtime when Conclave's server starts**, not on the first call.

Long-lived provider processes stay out of scope (§4) unless the measurements show process start-up dominates and per-turn tool control can be kept.

**Step 3: budgets** (PRF-03).
- Set targets per provider from the measurements.
- Enforce the runtime's own overhead in CI with the fake provider.
- Track the real-CLI numbers in the opt-in benchmark report.

## 10. Risks

- **Premature generalization.** The §4 promotion rule decides what moves. Move one provider at a time, and keep policy owned by a single application in that application.
- **Regressions from extracting the scheduler.**
  - `host.rs` holds protocol v1's lifecycle guarantees, so move it without changing behavior.
  - Keep TST-03, TST-04, and the golden fixtures as the gate.
- **Stricter Gemini checks.**
  - Failing on step types Antigravity doesn't document is the right default for a tool-free boundary. But it broke real turns in Pervue until the 2026-09-28 fix, and Conclave would inherit it.
  - Pervue has live smoke tests for Codex and Claude, but none yet for Gemini. Add one, and run it against each new Antigravity release.
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

## 11. Open decisions

1. A sidecar or a Node addon? The recommendation is the sidecar.
2. Incubate in `native/` and split later (recommended), or start a new repository now?
3. The runtime's name. `provider-runtime` is a placeholder.
4. The license.
5. Should Conclave move its OpenAI provider to `codex-exec`, or keep its TypeScript app-server adapter (§7)?
6. Does Conclave accept Pervue's stricter defaults, the environment allowlist and the private workspace, as they are?
7. What should the default absolute turn limit be, and can applications raise it? Conclave uses 180 s today, and Pervue has no limit.
8. Who keeps the record of per-turn cleanups that failed: the runtime (recommended, §4) or the application?

## 12. Revision history

**2026-09-28, after review.** A review checked the proposal against both repositories at the original evidence commits. It led to these changes:

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
- **Releases.** The runtime is versioned and pinned independently before Conclave makes it the default (§7).
- **Phases.** Phase 1 was scoped to what its first migration needs (§8).

**2026-09-28, after Gemini landed in Pervue.**

- **Evidence.** Updated to Pervue `5eb0cdf`, which has three providers. The §1 tables now reflect it.
- **Failures.** Corrected the claim that typed failures don't depend on how messages are worded (§2.1, item 8).
- **One process per turn.** This is now the rule for every provider. `codex app-server` is no longer planned as a runtime backend, and PRO-10 is dropped (§4).
- **Pilot.** Gemini goes first because it needs no continuation support, and Claude follows (§8).
- **Cleanup.** Split into per-turn cleanup (runtime) and cleanup when a conversation is deleted (application) (§4).
- **Grok.** It stays in Conclave (§4).
- **Codex.** Moving Conclave's OpenAI provider to `codex-exec` is Conclave's decision (§7).
- **Speed.** New section on cold start and speed, for after every provider runs on the runtime (§9).

[p-claude]: ../../native/host/src/providers/claude/mod.rs
[p-claude-output]: ../../native/host/src/providers/claude/output.rs
[p-codex]: ../../native/host/src/providers/codex/mod.rs
[p-codex-output]: ../../native/host/src/providers/codex/output.rs
[p-gemini]: ../../native/host/src/providers/gemini/mod.rs
[p-env]: ../../native/host/src/providers/environment.rs
[p-workspace]: ../../native/host/src/providers/codex/workspace.rs
[p-forget]: ../../native/host/src/providers/forget.rs
[p-host]: ../../native/host/src/host.rs
[p-process]: ../../native/core/src/process.rs
[p-native-connection]: ../../extension/src/background/native-connection.js
[p-perf]: ../../extension/src/shared/performance.js
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
