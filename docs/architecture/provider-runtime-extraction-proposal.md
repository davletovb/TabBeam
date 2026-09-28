# Proposal: a shared provider runtime for Pervue and Conclave

**Status:** Proposed, for discussion  
**Date:** 2026-09-27  
**Revised:** 2026-09-28 (see §12)  
**Scope:** extracting a reusable provider runtime from `native/`, and rewriting [Conclave](https://github.com/davletovb/conclave)'s server in Rust so that it links the runtime directly  
**Evidence:** Pervue at `5eb0cdf` (three providers, including Gemini), Conclave at `1379a37`

## Summary

- **Conclave justifies a new boundary between applications.** Pervue already extracted `pervue-core` under the two-provider rule in [framework §9.6](../product/browser-ai-extension-framework.md#96-extraction-rule). Conclave doesn't justify reuse for the first time. It justifies a *provider runtime* that two applications share.
  - The runtime owns provider execution: running an authenticated AI CLI safely, streaming its answer, and stopping it.
  - Each application keeps its conversation and product policy.
  - Native Messaging, browser context, provider sessions, and protocol v1 stay in Pervue.
- **Conclave's server becomes a Rust server that links the runtime directly.**
  - It replaces the Fastify server and keeps the same HTTP API, so Conclave's React/TypeScript web app doesn't change.
  - There is no sidecar process, no second protocol, and no npm binary packages.
  - All four of Conclave's providers get the runtime's process hardening. That includes Grok, which is built in Conclave on `pervue-core`'s process code.
- **Better security, with modest speed gains.**
  - The runtime brings the provider-side protections (§2.1).
  - The rewrite also closes two server gaps found in Conclave today: there's no authentication and no Host-header check (§7).
  - Speed gains come mostly from removing redundant sign-in checks and caching status, not from the language (§9).
- **Every provider runs as one process per turn.**
  - The runtime doesn't use long-lived provider servers, so it runs Codex through `codex exec` only.
  - This keeps cancelling a turn a kill, and confines a crash to one turn.
  - It also lets each turn choose its tools with launch flags (§4).
- **Pilot with Gemini, then Claude.** Both apps drive Antigravity (`agy`) almost identically, and neither keeps sessions for it. Gemini therefore needs no continuation support and is the smallest first step.
- **OpenAI in Conclave is a decision.** `codex-exec` loses token streaming, the live model list, and the ChatGPT usage panel. A Conclave-only app-server client keeps them (§7).
- **Speed work comes after every provider runs on the runtime.** First measure with real CLIs, then optimize in one place (§9).
- **Incubate the runtime in `native/`.**
  - Conclave depends on it pinned to a git revision.
  - Don't move files while the remaining tracker items are in flight.
  - The runtime gets its own repository once Conclave's Rust server runs on it.

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

Conclave's server is 5,712 lines of TypeScript plus 3,440 lines of tests. Its `ProviderAdapter` ([packages/core/src/index.ts][c-core]) has three parts: `listModels`, `generate(request, emit)` with an `AbortSignal`, and an optional `limits`. Four adapters implement it, and each handles processes its own way:

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
- treats usage reports as cumulative snapshots and adds the increase over the previous one ([orchestrator.ts:437–455][c-usage]);
- has no authentication, only a CORS allowlist of browser origins, and doesn't check the Host header ([index.ts][c-index]);
- starts CLI processes to answer `/providers` and `/models` on every request, and runs `claude auth status` before every Claude call ([anthropic-claude.ts:235][c-claude-auth]).

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

   All three can run in-process against Conclave's Rust server.

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

## 3. How Conclave uses the runtime

Five ways to connect Conclave to a Rust runtime:

| Option | What it means for Conclave |
|---|---|
| **Rust server** (decided) | Conclave's server is rewritten in Rust and links the runtime crate. The web app stays TypeScript |
| **Sidecar executable** | The Node server stays, and the runtime runs as a separate process speaking versioned JSON-RPC over stdio |
| **Node addon** (napi-rs) | The Node server loads the runtime as a `.node` module |
| **C ABI** (the path ADR-0001 expected) | The Node server loads the runtime through an FFI package |
| **Shared contract, TypeScript port** | Conclave reimplements the runtime's behavior in TypeScript |

**Rust server**
- For:
  - No sidecar, no second protocol, no per-platform npm packages, and no version pinning across two languages. The runtime is an ordinary Cargo dependency.
  - Conclave-only adapters (Grok, and an OpenAI app-server client if Conclave keeps one) are built on `pervue-core`'s process and stream code. All four providers get the same hardening.
  - One binary that also serves the built web app. Users need neither Node nor pnpm; Node is only needed to build the web app. Pervue's macOS and Windows packaging can be reused.
  - Pervue's fake provider and hostile-process matrix run in-process against Conclave's orchestration.
  - Provider knowledge lives in one language and one codebase.
  - The rewrite is the natural time to fix the server's authentication and Host-header gaps. Those fixes don't depend on the language, though (§7).
- Against:
  - A large port. About 4,200 lines of server code, including the 1,746-line orchestrator and the Grok adapter, plus about 2,300 lines of non-provider tests. Subtle behavior must match exactly, and stored data must still load.
  - The TypeScript types the web app shares with the server (`packages/core`, used by 15 web files) must be generated from Rust, or they drift.
  - Slower iteration on the product logic that changes most (orchestration modes, workflows, prompts), and contributors need Rust for everyday Conclave work.
  - Less isolation than a separate process. A runtime panic is contained to its thread, but an abort, such as running out of memory, takes the server down.
  - No meaningful speed gain from the language itself. Model latency dominates, as ADR-0001 also found.

**Sidecar executable**
- For:
  - The smallest change to Conclave: only its provider adapters are replaced.
  - A runtime crash can't take down the Node server.
- Against:
  - A binary has to reach `pnpm dev` users, and a second protocol has to be versioned.
  - Grok, which stays Conclave-only, keeps its TypeScript process handling.
  - A lost runtime process adds its own failure mode.

**Node addon, C ABI, and TypeScript port**
- All three keep the Node server, so they lose everything the Rust server gains.
- **The addon and the C ABI** bring FFI, `unsafe` binding code, and in-process crashes into the Node server. The C ABI also needs a versioned C façade ([core/README](../../native/core/README.md#compatibility-and-extraction)).
- **The TypeScript port** duplicates the hardest code, and Node can't reproduce some of its guarantees. For example, it can't kill a process group before reaping the child.

**Decision (2026-09-28): the Rust server.**
- It hardens every provider in one language, removes the sidecar layer, and ships as one binary.
- The accepted costs are the port, generated types, and slower iteration on product logic.
- The sidecar remains the fallback if the port stalls (§10).
- **ADR-0002 (Pervue)** records that the runtime is shared as a Rust crate, used in-process. ADR-0001's sentence about a C ABI for non-Rust consumers stands, because no non-Rust consumer remains.
- **Conclave's own ADR** records the server rewrite.

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
└─ service     in-process API for async servers: start a turn, receive its
               events, cancel it (§6)

pervue-host
├─ provider-runtime, linked as a crate; drives the scheduler from its own loop
├─ conversation→session map, stale-session policy, and transcript
│  removal when a conversation is deleted
├─ Native Messaging framing, origin checks, manifest, protocol v1
├─ browser context policy
└─ diagnostics and user-facing messages

conclave
├─ web app (React/TypeScript), with types generated from the server
└─ Rust server
   ├─ provider-runtime, linked as a crate through the service API
   ├─ HTTP API (same routes and JSON), serving the built web app
   ├─ orchestration, retries, and budgets
   ├─ run persistence and replay
   ├─ SearXNG search
   └─ Grok adapter, and an OpenAI app-server client if Conclave keeps one,
      built on pervue-core's process and stream code
```

**What moves, and why.**
- **The promotion rule.** Something moves into the runtime only when a second application uses it.
  - Session maps and conversation-scoped transcript removal stay in `pervue-host`, because Conclave runs every call statelessly.
  - Grok stays in Conclave, because Pervue deferred it. Conclave builds it on the shared process and stream primitives, which is allowed without promoting the adapter.
  - `codex-exec` moves when Conclave's Rust server uses it for OpenAI (§7).
  - Search-result sanitizing becomes shareable now that both apps are Rust. Pervue's version is the stricter one: it bounds text, strips tags, and validates URLs. It moves into the runtime, or a small shared crate, when Conclave's Rust search uses it.
- **One process per turn, for every provider in the runtime.**
  - Codex's app-server is the only CLI server that serves many conversations from one process. Grok's ACP mode can hold several sessions in one process, but Conclave runs one process per call. Claude can keep one conversation's process alive. Antigravity has no such mode that either app uses.
  - The runtime adopts none of them. Pervue chooses each turn's tools with launch flags: `--tools ""` or `--tools WebSearch` for Claude, the `pervue-text` or `pervue-search` agent for Gemini, and `-c` feature switches for Codex. A long-lived process would fix those at launch.
  - A process kept alive for a conversation would carry earlier turns' page text into later turns.
  - A crash of a shared process fails every running turn. Cancelling becomes a request the CLI has to honor, instead of a kill.
  - Pervue's host lives only as long as the extension's service worker keeps its connection ([native-connection.js][p-native-connection]), so a warm provider process wouldn't stay warm.
  - Conclave's turns always use the same tool setup (none), so these reasons weigh less for it. A Conclave-only app-server client for OpenAI stays possible (§7), but it would be Conclave's code, outside the runtime.
  - Speed is handled separately (§9).
- **Continuation.**
  - The runtime's Claude provider accepts an opaque native-session handle to resume, and reports the native session it ran.
  - When that session no longer exists, it fails with a reason of its own before producing any output.
  - `pervue-host` keeps the map, its `conv_` IDs, and the retry-once-with-history policy.
  - Claude's session IDs will then cross a crate boundary they don't cross today, but only as opaque values.
  - Conclave never uses continuation.
  - Gemini keeps no sessions, so the pilot doesn't need this hook. The hook arrives with Claude.
- **Cleanup has two scopes.**
  - **Per-turn cleanup is execution**, so it moves with the provider:
    - Gemini's Antigravity transcript.
    - For Conclave's stateless `codex-exec` turns, the session file `exec` saves for each turn, deleted using the ownership check in `forget_rollouts`. Whether `exec` has a flag to skip saving hasn't been checked.
  - **Cleanup when a conversation is deleted is policy**, so it stays with the app. In Pervue, that means Claude's and Codex's transcripts.
  - **Retrying failed deletions.** Pervue records failed Gemini deletions under its conversation IDs and retries them through `conversation.forget`. The runtime needs its own record, per namespace, retried when the runtime starts and on request, so Conclave gets retries without having conversations to hang them on. Pervue's `forget` can trigger the same retry.
- **Environment construction.**
  - The core README lists the credential allowlist as host policy, so moving it changes that boundary deliberately.
  - Conclave needs the same protection (§2.1, item 4), and a provider in the runtime can't start without its variables.
  - Applications can extend the list.

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
- Needed: an application namespace, fixed in code when the runtime starts. `pervue-host` passes `pervue`, and Conclave's server passes `conclave`.
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
  - These two limits replace Conclave's stall watchdog and per-adapter turn timeouts. Its `CONCLAVE_STEP_STALL_TIMEOUT_MS` and `CONCLAVE_*_TURN_TIMEOUT_MS` settings map onto them.

**Reconciling what each app learned.** Each product found something the other missed, in two places:
- **Claude command lines.** Pervue uses `--strict-mcp-config`, stream-json input, and `DISABLE_AUTOUPDATER`. Conclave uses `--no-session-persistence`, `--max-turns 1`, `--system-prompt`, `--disable-slash-commands`, and `--safe-mode`.
- **Gemini agent definitions and step handling.** See §2.2, item 3.

## 6. The in-process runtime API

The runtime stays synchronous and needs no async runtime of its own.
- **`pervue-host`** drives the scheduler directly from its existing loop.
- **Async servers such as Conclave's** use the `service` API:
  - The scheduler runs on its own thread.
  - Each turn's events arrive on a channel that works from both blocking and async code.

A sketch, not a final API:

```rust
let runtime = Runtime::start(RuntimeConfig {
    namespace: Namespace::fixed("conclave"),
    providers: vec![gemini::installed(), claude::installed()],
    limits: Limits::default(),
})?;

let turn = runtime.start_turn(TurnId::new(), Turn {
    provider: "claude".into(),
    model: Some("sonnet".into()),
    system: Some(system_prompt),
    messages,
    tools: ToolPolicy::None,
    continuation: None,
})?; // the turn is registered before this returns

while let Some(event) = turn.events.recv().await {
    match event {
        TurnEvent::Started | TurnEvent::Activity => {}
        TurnEvent::Delta(text) => { /* text_delta */ }
        TurnEvent::Source(source) => { /* citation */ }
        TurnEvent::Usage(usage) => { /* cumulative snapshot */ }
        TurnEvent::Ended(outcome) => break, // exactly once per started turn
    }
}

runtime.cancel(&turn.id)?; // Err(UnknownTurn) once the turn has ended
```

**Turn lifecycle:**

1. **Order.** `start_turn` registers the turn before it returns and before any provider work starts. Events follow on the turn's channel.
2. **Turn IDs.**
   - A turn ID is unique for the life of the runtime, not only while the turn is running, so a late cancel can never reach a newer turn.
   - Reusing an ID fails with `DuplicateTurnId`, and the turn that already holds the ID is unaffected.
3. **Exactly one ending.** A started turn gets exactly one `Ended`. A rejected `start_turn` returns an error and produces no events.
4. **Cancellation.**
   - `cancel` for a running turn succeeds, including repeats, and the turn still ends only once.
   - For a turn that never started or has already ended, `cancel` fails with `UnknownTurn`.
   - If a turn ends just before its cancel arrives, its `Ended` is already in its channel, so the caller can ignore the error.

**Other rules:**
- **Dropping the runtime** cancels every turn, waits out the stop grace, and kills what remains, as Pervue does with `INPUT_CLOSED`.
- **A scheduler-thread panic** ends every running turn with a runtime error marked `maybe_started`, because the provider may have started or even finished. The application decides whether to retry, and can start a new runtime.
- **If the whole server process dies,** Conclave's existing recovery marks unfinished runs `interrupted` at startup.
- **Diagnostics** go to a hook the application supplies. They contain no prompts, output, or account identifiers, as in OBS-01.
- **Versioning.** Conclave pins the runtime to a git revision while it incubates, and to a semver version once it's published. There's no wire protocol to version.

## 7. The Conclave Rust server

**Same functionality.** The web app keeps talking to the same HTTP API.

| Today (TypeScript) | Rust server |
|---|---|
| Fastify routes and CORS | axum plus tower-http, with the same routes and JSON shapes |
| Newline-delimited JSON event streams with `after=<seq>` replay | A streamed response body: same format, same replay |
| Orchestrator (12 modes, workflow graphs, budgets, retries) | tokio tasks for parallel steps, plus cancellation tokens |
| Stall watchdog plus 180 s per-adapter timeouts | The runtime's idle and absolute limits (§5): one layer instead of two |
| Retry decisions by regex on error text | The runtime's `retryable` flag and reason codes. A rate-limit notice comes from the reason, not the wording |
| `state.json` and `runs/*.ndjson`, with 0700/0600 permissions | Same file formats, so existing `~/.conclave` data still loads |
| SearXNG client | reqwest, with the shared search-result sanitizer (§4) |
| Claude, Gemini, and Codex adapters | The runtime, through the service API (§6) |
| Grok adapter | Conclave code built on `pervue-core`'s process and stream code |
| Mock provider | Rust, for development and tests |
| Web app served by Vite in development | The server also serves the built web app |

**Web app types.**
- The server's Rust types become the source of truth, and the TypeScript types in `packages/core` are generated from them, for example with ts-rs.
- CI fails if the generated types differ from the committed ones.

**Security.**
1. **From the runtime, for all four providers:**
   - prompts on stdin instead of the command line;
   - bounded memory;
   - process-group kill with escalation;
   - an allowlisted environment;
   - private workspaces and absolute executable paths;
   - Antigravity transcript cleanup;
   - strict step checks.
2. **Server gaps the rewrite must close:**
   - **Authentication.** Any local process can drive today's API, and CORS only restrains browsers. The Rust server requires a per-install token on every request.
   - **DNS rebinding.** Without a Host-header check, a malicious website can rebind its domain to `127.0.0.1`, then read conversations and start runs as if it were same-origin. Recent Chrome versions block some of this, but not every browser does. The Rust server rejects any Host not on its allowlist.
   - **Request bodies.** Explicit size limits, and unknown JSON fields are rejected.
3. **What Rust itself adds is smaller than it sounds.** JavaScript is memory-safe too. The real gain is that validation is harder to skip when requests become typed structs.

The two server gaps exist today and don't depend on the language. Patching them in the TypeScript server now is recommended, because the port will take months.

**Speed.** Rust handles each event in microseconds and uses tens of MB less memory than Node, but users won't notice either. The visible gains come from what the server stops doing, and the rewrite should include them:
- no `claude auth status` before every Claude call;
- cached provider status and model lists instead of starting CLIs on every `/providers` and `/models` request;
- no runtime start-up, because the runtime is in-process.

The rest of the plan is in §9. One change is slower: with `codex-exec`, OpenAI's first visible text arrives later, because `exec` delivers whole messages.

**OpenAI is a decision.**
- **`codex-exec` through the runtime** is simpler, but loses:
  - token-by-token streaming;
  - the live model list (`model/list`), so the model picker needs a list kept by hand;
  - the ChatGPT usage panel (`/provider-limits`, fed by `account/rateLimits/read`), which would show as unavailable.

  The subscription check moves to classifying `codex login status`.
- **A Conclave-only app-server client,** built on the core process code, keeps those features. Conclave's turns never change tools, so the one-process-per-turn reasons weigh less there (§4).

**Parity checks.**
- **Orchestration.** The HTTP contract fixtures recorded from the TypeScript server (§8) must pass: every mode, the event sequences, the errors, and the stored files.
- **Gemini (the pilot)** must match Conclave's current behavior on:
  - listing `gemini-` models from `agy models`, with Conclave still mapping its legacy aliases (`auto`, `pro`, `flash`, `flash-lite`);
  - refusing Antigravity's direct Gemini API-key mode;
  - the tool-free agent and the `init` checks;
  - usage from the final `result`;
  - the absolute turn limit;
  - cancellation;
  - the same failures in the run inspector.

  It also brings three changes in behavior:
  - transcripts are deleted after each call;
  - the agent definition sets `hooks: []`;
  - a turn fails when Antigravity reports a step type it doesn't document. This check failed real turns in Pervue until the 2026-09-28 fix.
- **Claude (second)** must match Conclave's current behavior on:
  - refusing sign-ins that aren't subscriptions;
  - the `sonnet`, `opus`, and `haiku` aliases;
  - usage events;
  - no tools;
  - no session persistence;
  - the system prompt;
  - the absolute turn limit;
  - cancellation;
  - the same failures in the run inspector.

**Switching over.**
- During the port, the Rust server runs alongside the TypeScript server on another port.
- It becomes the default once three things hold:
  - it passes the contract fixtures;
  - it serves all four providers;
  - it loads real `~/.conclave` data.
- The TypeScript server stays available for one release as a fallback, and is then removed.

## 8. Phases

The tracker isn't edited here, and the IDs below are only proposals. Each phase promotes only what the next real use needs.

**Phase 0: agree.** Nothing in this phase changes the runtime.
- Review this proposal.
- Write ADR-0002 in Pervue (the runtime is shared as a Rust crate, used in-process), and Conclave's own ADR for the server rewrite.
- Choose a name, a license, and where the runtime lives.
- Recommended: patch the TypeScript server's authentication and Host-header gaps now (§7).

**Phase 1: the runtime, with Gemini** (Pervue; proposed LIB-06 to LIB-09).
1. Introduce the neutral turn, error, usage, and timeout types from §5.
2. Move the Gemini provider into the runtime, along with only the mechanics it needs:
   - environment construction;
   - private-workspace primitives;
   - per-turn transcript cleanup, with the runtime's own retry record.

   No continuation hook is needed. `pervue-host` keeps passing all of its tests.
3. Extract the scheduler from `host.rs`, and add the absolute limit and the rule for what counts as progress. The protocol validator and golden fixtures must stay green.
4. Add the `service` API (§6), and run the hostile matrix and the contract suite through it.

**Phase 2: the Rust server's foundations** (Conclave). The TypeScript server stays the default throughout.
1. Record the HTTP contract fixtures: run the TypeScript server with the mock provider through every mode, and save the requests and responses, event sequences, errors, and stored files.
2. Port storage and the HTTP API against those fixtures. Generate the web app's types, and add authentication and the Host allowlist.
3. Port the orchestrator together with its tests.
4. Connect Gemini through the runtime.

**Phase 3: Claude.**
- Pervue moves the Claude provider into the runtime, with the continuation hook. `pervue-host` keeps its sessions and its transcript removal when a conversation is deleted.
- Conclave's Rust server connects Claude.

**Phase 4: Codex and Grok.**
- Pervue moves `codex-exec` into the runtime, with per-turn cleanup of `exec`'s session files for stateless turns. Alternatively, Conclave builds its own app-server client (§7).
- Conclave builds Grok on the core process and stream code, and ports the mock provider.

**Phase 5: switch.** Test against real `~/.conclave` data, make the Rust server the default, and keep the TypeScript server for one release.

**Phase 6: speed** (proposed PRF-01 to PRF-03). See §9.

**Phase 7: split and publish.** Move the runtime to its own repository and publish its crates.

Merging Pervue's copied helpers (§1) is cleanup inside the host. It can happen at any time and stays in the host.

**Working alongside the tracker.**
- Phase 1 should start only after LIB-01 to LIB-05 and PRO-08 are verified, so nothing is restructured while it is being verified.
- Pervue's Gemini adapter landed on 2026-09-28 and was fixed the same day for real `agy` output. It should settle before it moves.
- Phase 1 also changes `host.rs`, the host's largest and most heavily tested file. Coordinate that with whoever holds the remaining tracker items.
- Phase 2's first three steps don't depend on the runtime, so they can start in parallel with Phase 1.

## 9. Speed and cold start

Optimize after every provider runs on the runtime. At that point the runtime is the one place where turns start, so a measurement or a fix covers every provider in both apps at once.

**What is measured today.**
- TST-09 sets three budgets ([performance.js][p-perf]):
  - the popup ready for input within 100 ms;
  - the native host ready within 250 ms;
  - the first response chunk within 1,500 ms.
- CI enforces the last two against the built host, but only with the fake provider. No real CLI's start-up is measured.

**What a turn costs today.**

| Where | Processes started | Other work |
|---|---|---|
| Codex turn (`exec`) | 2: `codex login status`, then `codex exec` ([codex/mod.rs:460][p-codex]) | Workspace ownership check |
| Claude turn | 2: `claude auth status`, then `claude -p` ([claude/mod.rs:433][p-claude]). Conclave does the same ([anthropic-claude.ts:235][c-claude-auth]) | Workspace ownership check |
| Gemini turn | 1: `agy` | Creating a private workspace and writing the agent definition; workspace ownership check |
| Conclave page load | `claude auth status`, `agy models`, and a whole Grok ACP process for `/providers`, plus more for `/models` | — |

`codex exec` delivers each message whole, so Codex's first visible text waits for its first complete message. No process-level optimization changes that.

**Step 1: measure** (PRF-01).
- For each provider, with real CLIs, record a timeline of each turn: request received → sign-in check done → provider started → first output line → first answer text → turn ended → process exited.
- Record cold runs (the first turn after sign-in or a reboot) and warm runs (repeated turns).
- Run it as an opt-in benchmark, like the live smoke tests, because CI has no signed-in CLIs.

**Step 2: optimize** (PRF-02), roughly in order of expected gain for the risk:
1. **Stop paying for the sign-in check on every turn.** Either:
   - cache a successful check for a short time and re-check after any authentication failure; or
   - run the check alongside the turn instead of before it, and stop the turn if the check says signed out.

   Either one removes a process start from the critical path of every Codex and Claude turn. Codex needs the check because a signed-out `exec` keeps retrying instead of failing. A stale cache there is bounded by the start timeout.
2. **Cache provider status and model lists** in Conclave's server, and refresh them in the background instead of starting CLIs on every page load.
3. **Prepare per-turn resources ahead of time**, such as Gemini's next private workspace and agent definition. The workspace ownership check must still run at launch, because caching it would open a race.
4. **Keep executable discovery across turns**, and look again only when a launch fails. Today each request does one lookup.

Long-lived provider processes stay out of scope for the runtime (§4) unless the measurements show process start-up dominates and per-turn tool control can be kept.

**Step 3: budgets** (PRF-03).
- Set targets per provider from the measurements.
- Enforce the runtime's own overhead in CI with the fake provider.
- Track the real-CLI numbers in the opt-in benchmark report.

## 10. Risks

- **The port stalls or drifts.**
  - About 4,200 lines and 2,300 lines of tests must match subtle behavior: custom-workflow failure handling, deferred retries, budget accounting, degraded results, event replay, and recovering interrupted runs.
  - The contract fixtures, running both servers side by side, and the TypeScript fallback limit the damage.
  - If the port stalls, the sidecar design from the earlier revision can still bring the runtime's hardening to the TypeScript server.
- **Stored data compatibility.** Existing `state.json` and `runs/*.ndjson` files must load unchanged. Test with real data before switching.
- **Feature work during the port.** Either freeze Conclave's features, or port in parallel and re-sync (§11).
- **Less isolation in-process.** A runtime panic is contained to its thread (§6), but an abort takes the server down. The runtime bounds memory, which makes the most likely abort, running out of memory, unlikely.
- **Contributors need Rust** for everyday Conclave server work. The web app stays TypeScript.
- **Premature generalization.** The §4 promotion rule decides what moves. Move one provider at a time, and keep policy owned by a single application in that application.
- **Regressions from extracting the scheduler.**
  - `host.rs` holds protocol v1's lifecycle guarantees, so move it without changing behavior.
  - Keep TST-03, TST-04, and the golden fixtures as the gate.
- **Stricter Gemini checks.**
  - Failing on step types Antigravity doesn't document is the right default for a tool-free boundary. But it broke real turns in Pervue until the 2026-09-28 fix, and Conclave would inherit it.
  - Pervue has live smoke tests for Codex and Claude, but none yet for Gemini. Add one, and run it against each new Antigravity release.
- **Stricter defaults for Conclave.**
  - The environment allowlist may drop a variable someone relies on, so each provider can extend it.
  - A private workspace means project `CLAUDE.md` files no longer apply. That's intended, but users should be told.
- **Windows.**
  - There is no Job Object yet ([process.rs][p-process]), so only the provider process itself is stopped, not its descendants. Conclave has the same gap today.
  - Don't claim process-tree kills on Windows.
- **Licensing.** Neither repository has a license. Choose one before publishing crates.

## 11. Open decisions

1. Incubate the runtime in `native/` and split it later (recommended), or start a new repository now?
2. The runtime's name. `provider-runtime` is a placeholder.
3. The license.
4. OpenAI in Conclave: `codex-exec` through the runtime, or a Conclave-only app-server client that keeps streaming (§7)?
5. Should Rust become the source of truth for the web app's types (recommended), or should `packages/core` stay the source and the Rust server be checked against it?
6. Freeze Conclave's features during the port, or port in parallel and re-sync?
7. Patch the TypeScript server's authentication and Host-header gaps now, before the port (recommended)?
8. Does Conclave accept Pervue's stricter defaults, the environment allowlist and the private workspace, as they are?
9. What should the default absolute turn limit be, and can applications raise it? Conclave uses 180 s today, and Pervue has no limit.
10. Who keeps the record of per-turn cleanups that failed: the runtime (recommended, §4) or the application?

## 12. Revision history

**2026-09-28, after review.** A review checked the proposal against both repositories at the original evidence commits. It led to these changes:

- **Framing.** Conclave justifies a new boundary between applications. It doesn't satisfy the extraction rule for the first time, because Pervue already met it with two providers.
- **Boundary.** Session maps, managed continuation, and transcript removal stay in `pervue-host`. The runtime offers a continuation hook instead (§4).
- **Protocol.** Cancellation gets a response, and the turn lifecycle is specified: when a turn is registered, when an ID may be reused, what happens with unknown or repeated cancellations, and that every started turn ends exactly once. These rules now apply to the in-process API (§6).
- **Runtime loss.** Errors carry `maybe_started` when a turn may have run, and retrying it is the application's decision (§6).
- **Namespace.** It is fixed when the runtime starts, not chosen by a caller. It is described as protection against accidents, not a security boundary (§5).
- **Usage.** Defined as cumulative snapshots for each turn (§5).
- **Timeouts.** An absolute `max_turn` limit and a rule for what resets the idle timer (§5). Checking the code for this showed that Pervue's Claude adapter can be kept alive indefinitely today.
- **Releases.** The runtime is versioned and pinned independently of both applications (§6).
- **Phases.** Phase 1 was scoped to what its first migration needs (§8).

**2026-09-28, after Gemini landed in Pervue.**

- **Evidence.** Updated to Pervue `5eb0cdf`, which has three providers. The §1 tables now reflect it.
- **Failures.** Corrected the claim that typed failures don't depend on how messages are worded (§2.1, item 8).
- **One process per turn.** This is now the rule for every provider in the runtime. `codex app-server` is no longer planned as a runtime backend, and PRO-10 is dropped (§4).
- **Pilot.** Gemini goes first because it needs no continuation support, and Claude follows (§8).
- **Cleanup.** Split into per-turn cleanup (runtime) and cleanup when a conversation is deleted (application) (§4).
- **Speed.** New section on cold start and speed, for after every provider runs on the runtime (§9).

**2026-09-28, Rust server for Conclave.**

- **Decision.** Conclave's Fastify server is to be rewritten in Rust and link the runtime directly, keeping the same HTTP API. This replaces the sidecar recommendation (§3).
- **Protocol.** The sidecar and its JSON-RPC protocol are dropped. An in-process `service` API keeps the same turn-lifecycle rules (§6).
- **Server security.** Found two gaps in Conclave's current server, no authentication and no Host-header check. The Rust server closes them, and patching them in the TypeScript server now is recommended (§7).
- **Grok.** It stays in Conclave but is built on `pervue-core`'s process code, so all four providers are hardened (§4).
- **Search.** Search-result sanitizing becomes shareable between the two apps (§4).
- **OpenAI.** Conclave chooses between `codex-exec` and its own app-server client (§7).
- **Phases, risks, and open decisions** rewritten for the port (§8, §10, §11).

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
[c-index]: https://github.com/davletovb/conclave/blob/1379a37ccb13d484247122ee215257e642b166fc/apps/server/src/index.ts
[c-classify]: https://github.com/davletovb/conclave/blob/1379a37ccb13d484247122ee215257e642b166fc/apps/server/src/orchestrator.ts#L98-L130
[c-usage]: https://github.com/davletovb/conclave/blob/1379a37ccb13d484247122ee215257e642b166fc/apps/server/src/orchestrator.ts#L437-L455
[c-watchdog]: https://github.com/davletovb/conclave/blob/1379a37ccb13d484247122ee215257e642b166fc/apps/server/src/orchestrator.ts#L507
[c-claude-auth]: https://github.com/davletovb/conclave/blob/1379a37ccb13d484247122ee215257e642b166fc/apps/server/src/providers/anthropic-claude.ts#L235
[c-claude-argv]: https://github.com/davletovb/conclave/blob/1379a37ccb13d484247122ee215257e642b166fc/apps/server/src/providers/anthropic-claude.ts#L254
[c-claude-buf]: https://github.com/davletovb/conclave/blob/1379a37ccb13d484247122ee215257e642b166fc/apps/server/src/providers/anthropic-claude.ts#L154
[c-claude-kill]: https://github.com/davletovb/conclave/blob/1379a37ccb13d484247122ee215257e642b166fc/apps/server/src/providers/anthropic-claude.ts#L139
[c-grok-kill]: https://github.com/davletovb/conclave/blob/1379a37ccb13d484247122ee215257e642b166fc/apps/server/src/grok/acp-client.ts#L150
[c-codex-env]: https://github.com/davletovb/conclave/blob/1379a37ccb13d484247122ee215257e642b166fc/apps/server/src/codex/app-server-client.ts#L64
