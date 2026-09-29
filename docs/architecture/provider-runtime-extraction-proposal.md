# Proposal: a shared provider runtime

**Status:** Proposed, for discussion; the basis for [ADR-0002](adr-0002-shared-provider-runtime.md)  
**Date:** 2026-09-27  
**Revised:** 2026-09-28 (see §11)  
**Scope:** hardening and extracting a reusable provider runtime from Pervue's `native/` so that a second application can use it. How Conclave adopts the runtime is recorded in Conclave's own repository, in its [adoption proposal][conclave-adoption].  
**Evidence:** Pervue at `fc7284b` (four providers: Codex, Claude, Gemini, Grok), Conclave at `1379a37`

## Summary

- **Conclave justifies a runtime shared between applications.** Pervue already extracted `pervue-core` under the two-provider rule in [framework §9.6](../product/browser-ai-extension-framework.md#96-extraction-rule). Conclave doesn't justify reuse for the first time. It justifies a *provider runtime* that two applications share.
  - The runtime owns provider execution: running an authenticated AI CLI safely, streaming its answer, and stopping it.
  - Each application keeps its conversation and product policy.
  - Native Messaging, browser context, provider sessions, and protocol v1 stay in Pervue.
- **Both apps now drive the same four CLIs:** Codex, Claude, Gemini (through Antigravity), and Grok. Each app has its own independent adapter for every one of them.
  - The runtime owns one shared adapter per supported execution mode: `codex exec`, Claude's print mode, Antigravity's one-shot mode, and Grok's one-shot headless mode.
  - An application can keep a different mode of the same CLI outside the shared set, such as Codex app-server or Grok ACP, when that mode's capabilities differ materially. It builds that adapter on `runtime-core`.
- **The runtime is a Rust crate workspace that applications use in-process.**
  - A single supervisor owns the scheduler.
  - `pervue-host` drives that supervisor from its own loop.
  - Async servers use a small service API, which runs the supervisor on a thread of its own.
  - If a non-Rust application ever needs the runtime, it gets a sidecar that wraps the service API.
  - There's no C ABI and no Node addon. ADR-0002 records this.
- **Conclave is the second consumer.**
  - It has committed to adopting the runtime for all four providers at once, in a new Rust server. Its [adoption proposal][conclave-adoption], in the Conclave repository, records those decisions.
  - This proposal needs Conclave as a consumer, but doesn't depend on how Conclave builds its server.
- **Every provider runs as one process per turn.** Codex runs through `exec` only, and Grok in one-shot headless mode, as Pervue's adapter already does.
- **Sign-in checks never run alongside a turn.** An application may choose to cache a successful check, but the runtime never caches by default (§8).
- **Three stages, each with an exit bar** (§7):
  1. **Harden the runtime inside Pervue.** Fix the known gaps, and verify the pending tracker items, until a defined exit bar is met.
  2. **Extract it as a library, with all four adapters at once.** Pervue switches to the library first, and its full test suite passing against the library proves the extraction.
  3. **Conclave integrates all four providers** into its Rust server. That work is planned in Conclave's own proposal.
- **Speed work comes after both applications run on the library** (§8).
- **The library gets its own repository in Stage 2,** and `pervue-core` becomes `runtime-core` inside it. Both applications pin it to a git revision.

## 1. What exists today

### Pervue

`pervue-core` already holds the primitives the providers share ([core/README](../../native/runtime-core/README.md)): `process`, `stream`, `discovery`, `exchange`, `protocol`, and `framing`. The host holds the rest of the plumbing for its four real providers, and some of it is still copied between adapters:

| Host-owned piece | Where | Reuse today |
|---|---|---|
| Conversation→session map (`session_name`, read, save, forget) | [claude/mod.rs][p-claude], [codex/mod.rs][p-codex] | Claude and Codex each have a copy. Gemini and Grok keep no sessions |
| Private files and directories, conversation IDs, stderr tails, deadlines | [private_fs.rs][p-private-fs] | Gemini and Grok share it since Grok landed. Claude and Codex keep their own copies |
| Status probe, sign-in from the exit status, data-directory lookup | All four adapters | A copy in each |
| `PATH` for npm shims, and the environment allowlist | [providers/environment.rs][p-env] | Shared |
| Private workspace and its ownership checks | [codex/workspace.rs][p-workspace] | All four import Codex's module |
| Removing provider files | [providers/forget.rs][p-forget], [gemini/mod.rs][p-gemini], [grok/mod.rs][p-grok] | Claude and Codex remove transcripts when a conversation is deleted. The other two clean up after every turn, with recovery for deletions that fail (details below the table) |
| Request loop: in-flight table, start/idle timeouts, cancel → stop → kill, fairness, delta splitting | [host.rs][p-host] (`Session`, `Running`) | Tied to writing protocol-v1 events |

The two per-turn cleanups work differently:
- **Gemini** deletes the transcript Antigravity saves, and keeps durable records so failed deletions can be retried.
- **Grok** deletes its whole private workspace, including Grok's session files and the prompt file. Heartbeat markers let startup recovery remove only directories that are stale.

Gemini and Grok both landed without changing the `Provider` trait, `host.rs`, the protocol, or `pervue-core`. Four providers fitting the contract unchanged is good evidence that `Exchange` and `Update` are ready to become the runtime's API.

### Conclave

Conclave is a TypeScript/Node application. Its `ProviderAdapter` ([packages/core/src/index.ts][c-core]) has three parts: `listModels`, `generate(request, emit)` with an `AbortSignal`, and an optional `limits`. Four adapters implement it, and each handles processes its own way:

| Provider | Runtime and transport | Prompt | Child environment | Stopping | Output |
|---|---|---|---|---|---|
| Anthropic | `claude -p` with stream-json, one process per call | Last command-line argument | Server's environment minus 5 names | SIGTERM to the child only | All stdout and stderr kept in strings |
| OpenAI | `codex app-server`, one long-lived JSON-RPC process | stdin | Server's full environment | `turn/interrupt`; the process gets SIGTERM | `readline`, no line limit |
| xAI | `grok agent stdio` (ACP), one process per call | stdin | Server's environment minus 4 names, plus lockdown flags | SIGTERM to the child only | `readline`, no line limit |
| Google | `agy` with stream JSON, a private temporary workspace per call | stdin | Server's environment minus a list of billing variables | SIGTERM to the process group, SIGKILL after 1 s | All stdout and stderr kept in strings |

Conclave also:

- decides whether to retry or report a rate limit by matching error messages against regular expressions ([orchestrator.ts:98–130][c-classify]);
- keeps no provider sessions: Codex threads are `ephemeral`, Claude runs with `--no-session-persistence`, and each prompt replays the history;
- refuses API-key, Console, and cloud billing by reading the account type (`claude auth status` JSON, Codex `account/read`), and runs that check before every Claude call ([anthropic-claude.ts:235][c-claude-auth]);
- bounds every call absolutely, with a 180-second `CONCLAVE_*_TURN_TIMEOUT_MS` per adapter, and re-arms a 180-second stall watchdog on every provider event ([orchestrator.ts:507][c-watchdog]);
- treats usage reports as cumulative snapshots and adds the increase over the previous one ([orchestrator.ts:437–455][c-usage]).

## 2. What each product gains

### 2.1 What Conclave gains from Pervue's code

In order of impact:

1. **The Claude prompt is a command-line argument** ([anthropic-claude.ts:254][c-claude-argv]).
   - Linux limits a single argument to 128 KiB (`MAX_ARG_STRLEN`), and Windows limits the whole command line to 32,767 characters. A Debate, Judge, or synthesis prompt that embeds other models' answers can fail to start.
   - On Linux, other local users can read the prompt with `ps`.
   - Pervue sends prompts on stdin, or through a private prompt file for Grok.
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
   - Pervue runs each provider in an empty private workspace whose ownership it checks.
6. **Every Gemini call leaves a transcript behind.**
   - Conclave deletes each call's temporary workspace but never touches `~/.gemini/antigravity-cli/brain/`.
   - Pervue's adapter deletes the transcript `agy` saves there after every turn, and keeps durable records to retry deletions that fail.
   - Pervue's agent definition also sets `hooks: []`; Conclave's doesn't.
7. **Grok runs with Conclave's own home and environment.**
   - Pervue gives each Grok turn a private `GROK_HOME`, passes only the cached OAuth file, and disables more of Grok's features: workflows, telemetry, feedback, and folder trust.
   - Pervue also explicitly denies the MCP umbrellas (`search_tool`, `use_tool`), which shipped Grok Build keeps available even with a restricted agent. It then verifies the `init` boundary before showing any text.
   - Conclave's Grok inherits the server's environment minus four names, and relies on refusing ACP permission requests.
8. **Executables are found through the server's `PATH`** (`spawn("claude")`). Pervue resolves an absolute path from fixed install locations and allows an explicit override.
9. **Failures are classified once, inside the adapter.**
   - Outside a Pervue adapter, every failure has a `code`, a `reason`, and a `retryable` flag, so code that handles rate limits and transient errors never reads error text.
   - Underneath, the adapters still classify by matching phrases in the provider's message ([codex/output.rs:128][p-codex-output], [claude/output.rs:211][p-claude-output]), because failures arrive as text.
   - Unlike Conclave's regular expressions, they match specific phrases rather than bare words such as "rate" or "auth", right next to the parser for that provider's output. The Codex matching is tested against output captured from Codex CLI 0.156.1.
10. **Test assets.**
    - A fake provider binary with Codex, Claude, Gemini, and Grok personas.
    - The hostile-process matrix (TST-04).
    - The cross-provider contract suite (TST-10).

### 2.2 What Pervue gains from Conclave's code

1. **Knowing how each CLI is billed.**
   - Pervue's environment allowlist keeps API keys in environment variables away from providers. It doesn't cover a CLI that was itself signed in with an API key or a Console account.
   - Conclave checks the sign-in method for Claude by reading `claude auth status` JSON.
   - For Codex, Conclave uses app-server's `account/read`. With `exec`, the equivalent is classifying the output of `codex login status`, which Pervue currently doesn't read. That output's exact wording hasn't been checked.
   - For Gemini and Grok, both apps strip API-key variables. For Grok, both also set `GROK_DISABLE_API_KEY_AUTH`.
2. **Usage and live model lists, which Pervue needs for its own providers.**
   - Codex's `turn.completed` event and Antigravity's `result` event both report token counts. Pervue ignores them, because `Update` has no usage.
   - Pervue runs `agy models` to check Gemini's status, but can't expose the list, because `ModelOption` holds only fixed strings.
3. **Cross-checks.** Both apps have independent implementations of the same CLIs, and the differences are worth reconciling:
   - **Gemini.** The two implementations agree on flags, the agent definition, permission modes, and the `init` check. Pervue fails a turn when Antigravity reports a step type it doesn't document, while Conclave ignores such steps. Pervue deletes transcripts, and Conclave reads usage.
   - **Grok.** The two use different modes. Conclave uses ACP, which streams text; Pervue uses one-shot headless mode, which doesn't. Pervue checks more at `init`.
4. **A second application** to test the runtime's API against, so it isn't shaped only by Pervue's needs.

## 3. How the runtime is consumed

- **`pervue-host`** links the runtime crates and drives the supervisor, and through it the scheduler, from its existing synchronous loop.
- **Async Rust servers** use the in-process service API (§6).
- **A non-Rust application** would get a sidecar executable: a thin wrapper around the service API that speaks a versioned protocol over stdio. It is built only if a consumer needs it.
- **Not planned:** a C ABI or a Node addon.
  - Both bring FFI, `unsafe` binding code, and in-process crashes into the host application.
  - A C ABI also needs a versioned C façade ([core/README](../../native/runtime-core/README.md#compatibility-and-extraction)).
  - A pure TypeScript port would duplicate the hardest code, and Node can't reproduce some of its guarantees. For example, it can't kill a process group before reaping the child.

**ADR-0002** records:
- the runtime is a Rust crate workspace, used in-process;
- it offers the service API;
- non-Rust consumers get a sidecar.

That supersedes ADR-0001's sentence about a C ABI for non-Rust consumers.

**Conclave** will use the service API from a new Rust server, as its [adoption proposal][conclave-adoption] records. A sidecar remains the fallback for Conclave if its port stalls.

## 4. Proposed boundary

The runtime owns **provider execution mechanics**. The applications keep **conversation and product policy**.

```text
provider-runtime workspace (working name)
├─ runtime-core   process, stream, discovery; neutral turn, error, and usage types
│                 (today's pervue-core, minus framing and Pervue's protocol vocabulary)
├─ platform       environment construction, private files and workspaces,
│                 per-turn cleanup with its retry record
├─ providers      gemini, grok, claude, codex-exec
├─ scheduler      concurrent turns, start/idle/absolute limits,
│                 cancel → stop → kill, fairness, delta splitting,
│                 and the supervisor under both entry points (§6)
└─ service        in-process API for async servers: runs the supervisor
                  on a thread of its own

pervue-host
├─ provider-runtime crates
├─ Native Messaging framing (moved out of pervue-core), origin checks, manifest, protocol v1
├─ conversation→session map, stale-session policy, and transcript
│  removal when a conversation is deleted
├─ browser context policy
└─ diagnostics and user-facing messages

A second application (Conclave: see its adoption proposal)
├─ provider-runtime crates, through the service API or a sidecar
└─ its own product: orchestration, storage, search, UI
```

**Crate ownership.**
- `pervue-core` doesn't survive as a name. Its reusable modules become `runtime-core`, inside the runtime workspace, and its Pervue-specific parts (framing and protocol-v1 vocabulary) move into `pervue-host`.
- Applications depend only on runtime crates, never on Pervue's crates.
- An application's own adapters build on `runtime-core`. Conclave-only adapters, such as an app-server client for Codex, fall under this rule.
- When the runtime gets its own repository, the whole workspace moves, and Pervue depends on it the same way Conclave does.

**What moves, and why.**
- **The promotion rule.** Something moves into the runtime only when a second application uses it.
  - A shared adapter is one execution mode of a CLI. It moves into the runtime when a second application has accepted a plan to use that same mode.
  - Conclave has committed to the library's mode for all four CLIs: `codex exec`, Claude's print mode, Antigravity's one-shot mode, and Grok's one-shot headless mode. That meets the bar for all four at once, so they move together in Stage 2 (§7).
  - A different mode of the same CLI that only one application uses would stay with that application. Codex app-server and Grok ACP are examples, and Conclave has decided against both.
  - Session maps and conversation-scoped transcript removal stay in `pervue-host`, because Conclave runs every call statelessly.
  - Search-result sanitizing becomes shareable only if Conclave's server moves to Rust.
- **One process per turn, for every provider.**
  - Codex's app-server is the only CLI server that serves many conversations from one process. Grok's ACP mode can hold several sessions in one process, and Claude can keep one conversation's process alive. Antigravity has no such mode that either app uses.
  - Pervue's Grok adapter already chose one-shot headless mode over ACP for the same reasons.
  - Pervue chooses each turn's tools with launch flags: `--tools ""` or `--tools WebSearch` for Claude, the `pervue-text` or `pervue-search` agent for Gemini, `-c` feature switches for Codex, and tool denials for Grok. A long-lived process would fix those at launch.
  - A process kept alive for a conversation would carry earlier turns' page text into later turns.
  - A crash of a shared process fails every running turn. Cancelling becomes a request the CLI has to honor, instead of a kill.
  - Pervue's host lives only as long as the extension's service worker keeps its connection ([native-connection.js][p-native-connection]), so a warm provider process wouldn't stay warm.
  - Speed is handled separately (§8).
- **Continuation.**
  - The runtime's Claude provider accepts an opaque native-session handle to resume. It reports the native session it ran through the `Session` event (§6), before `Started`, and again if the session changes.
  - When that session no longer exists, it fails with a reason of its own before producing any output.
  - `pervue-host` keeps the map, its `conv_` IDs, and the retry-once-with-history policy.
  - Claude's session IDs will then cross a crate boundary they don't cross today, but only as opaque values.
  - Gemini and Grok keep no sessions, so they need no hook.
  - Whether a turn keeps its native session at all is set by its session policy (§5), not guessed from whether it has a continuation handle.
- **Cleanup has two scopes.**
  - **Per-turn cleanup is execution**, so it moves with the provider:
    - Gemini's Antigravity transcript.
    - Grok's private workspace, with heartbeat markers and recovery of stale directories at startup.
    - `Ephemeral` `codex exec` turns (§5) need none: they run with `codex exec --ephemeral`, so Codex saves no session at all. Deleting rollout files isn't an equivalent fallback. Codex's state database, which `forget_rollouts` leaves untouched, can still hold the first user message.
  - **Cleanup when a conversation is deleted is policy**, so it stays with the application.
    - In Pervue, that means Claude's and Codex's transcripts for every session the conversation has used, not just its current one.
    - A conversation's session changes when a search turn starts a new one (§7), or when Claude reports a different session. Each time, the host records the superseded handle durably before switching, then removes that session's transcript in the background.
    - A removal that fails stays recorded, and `conversation.forget` removes every session recorded for the conversation.
  - **Retrying failed deletions.** The runtime keeps its own record of failed per-turn deletions, per namespace. It retries them when it starts and on request, so a consumer without conversations still gets retries. Pervue's `conversation.forget` can trigger the same retry.
- **Environment construction.**
  - The core README lists the credential allowlist as host policy, so moving it changes that boundary deliberately.
  - Conclave needs the same protection (§2.1, item 4), and a provider in the runtime can't start without its variables.
  - Applications can extend the list only through trusted configuration read at startup. The names of variables a provider inherits never come from a request, a page, a prompt, a model's output, or any other per-turn input.

## 5. API changes a second application needs

**Error wording**
- Today: `ErrorBody<'static>` carries Pervue's wording, such as "Pervue couldn't prepare a private folder…".
- Needed: the runtime returns only `code`, `reason`, and `retryable`, and each application writes its own messages.
- Why: `pervue-host` keeps a table of messages, so protocol v1 doesn't change.

**Error codes and capabilities**
- Today: `ErrorCode` and `Capabilities` mix runtime concepts with browser ones: `HostNotInstalled`, `ContextUnavailable`, `page_context`.
- Needed: runtime concepts in the runtime, marked `#[non_exhaustive]`. Browser concepts in the host.
- Why: the runtime can add codes without breaking semver.

**Models**
- Today: `ModelOption` holds `&'static str` values from a fixed list.
- Needed: owned values, discovered at run time.
- Why: `agy models` is a live catalog, and Grok resolves versioned aliases.

**Usage**
- Today: `Update` has no usage.
- Needed: `Update::Usage`, a cumulative snapshot for the whole turn.
  - Both `input_tokens` and `output_tokens` are optional.
  - Neither value is ever lower than in the previous snapshot, and consumers keep the latest.
  - An adapter whose provider reports usage per message adds those reports up itself.
- Why: Conclave's budgets and run inspector depend on it, and its orchestrator already reads usage this way ([orchestrator.ts:437–455][c-usage]).

**Requests**
- Today: `SendRequest` has `text`, `history`, browser `context`, and `native_search`.
- Needed: a neutral turn with `system`, `messages`, `model`, a tool policy (`None` or `NativeWebSearch`), a session policy (below), and an optional continuation handle (§4).
- Why: Conclave needs a system prompt and runs every call statelessly. Pervue frames its browser context into messages before it calls the runtime.

**Session policy**
- Today: each adapter decides for itself whether the provider keeps its native session.
- Needed: every turn states `Ephemeral` or `Persistent`.
  - **`Ephemeral`:** nothing the provider saves outlives the turn. No `Session` event is sent.
    - Where the CLI has a flag for this, the adapter uses it: Claude's `--no-session-persistence`, and `codex exec --ephemeral`.
    - Otherwise, per-turn cleanup removes what the CLI saved: Antigravity's transcript, or Grok's workspace.
    - Each mode's live smoke test verifies this. After an `Ephemeral` turn, nothing the CLI stores, files or databases, may hold the prompt.
    - A mode that can't meet that refuses `Ephemeral` rather than degrading. One example is an installed Codex without `--ephemeral`.
  - **`Persistent`:** the provider keeps its native session and reports it through `Session` (§6). Only execution modes that can resume (Claude and Codex) accept it; `start_turn` rejects it for the others. A turn that carries a `continuation` handle is always `Persistent`.
- Why: the same start, with no continuation handle, needs opposite behavior from the two applications. Pervue's first turn with Claude or Codex must keep the new session and report it, so the conversation can resume. Every Conclave call must leave nothing behind. A default either way would break one of them.
  - **Conclave:** always `Ephemeral`.
  - **Pervue:** `Persistent` for Claude and Codex, and `Ephemeral` for Gemini and Grok, as today. Search turns follow the Stage 1 rule (§7).

**Namespace**
- Today: paths and thread names are hard-coded under `pervue`.
- Needed: an application namespace, fixed when the runtime starts.
  - An application that links the runtime passes it in code.
  - A sidecar would take it as a launch argument.
- Why: each application gets its own workspaces and cleanup records, so a mistake in one product can't touch the other's files. This protects against accidents, not attacks: any process running as the user can already write to both products' directories.

**Sign-in**
- Today: the status check reads only the exit code. Claude's and Codex's adapters run it before every turn.
- Needed:
  - An optional sign-in classification (`Subscription`, `ApiKey`, `Cloud`, `Unknown`) that keeps no identifiers. For Codex, it comes from classifying the output of `codex login status`, because `exec` has no account API.
  - A per-turn check that the application turns on or off. When it's on, the check finishes before the turn's provider process starts, and never runs alongside it. The runtime doesn't cache the result unless the application sets a caching policy (§8).
- Why: Conclave's guarantee that it uses subscriptions only (see its adoption proposal). Refusing a sign-in stays the application's decision.

**Timeouts**
- Today: each adapter has fixed start, idle, and stop-grace constants, and nothing limits a turn's total duration.
- Needed:
  - An absolute `max_turn` limit alongside the others. Applications can override all of them, within ceilings.
  - A fixed rule for what resets the idle timer: deltas, sources, and provider events that the adapter recognizes as work. stderr and unrecognized output never reset it.
- Why:
  - Pervue's Claude adapter counts any unrecognized `stream_event` as progress ([claude/output.rs][p-claude-output]). A Claude build that keeps emitting such events keeps its turn alive indefinitely.
  - That is tolerable in a popup the user can close. It isn't in a server-owned run that nobody is watching.
  - Conclave already bounds every call absolutely.

**Reconciling what each app learned.**
- **Claude command lines.** Pervue uses `--strict-mcp-config`, stream-json input, and `DISABLE_AUTOUPDATER`. Conclave uses `--no-session-persistence`, `--max-turns 1`, `--system-prompt`, `--disable-slash-commands`, and `--safe-mode`.
- **Gemini agent definitions and step handling.** See §2.2, item 3.
- **Grok lockdown settings.** Pervue's list is the longer one (§2.1, item 7).

## 6. The in-process runtime API

The runtime stays synchronous and needs no async runtime of its own. Both ways in go through the same supervisor, which owns the scheduler (see Supervision below):
- **`pervue-host`** drives the supervisor from its existing synchronous loop.
- **Async servers** use the `service` API:
  - it runs the supervisor on a thread of its own;
  - each turn's events arrive on a channel that works from both blocking and async code.

A sketch, not a final API:

```rust
let runtime = Runtime::start(RuntimeConfig {
    namespace: Namespace::fixed("conclave"),
    providers: vec![gemini::installed(), grok::installed()],
    limits: Limits::default(),
})?;

let mut turn = runtime.start_turn(Turn {
    provider: "gemini".into(),
    model: None,
    system: Some(system_prompt),
    messages,
    tools: ToolPolicy::None,
    session: SessionPolicy::Ephemeral,
    continuation: None,
})?; // registered, with an ID the runtime generates, before this returns

let id = turn.id();            // for logs and the run inspector
let cancel = turn.canceller(); // cloneable, usable from another task

while let Some(event) = turn.next().await {
    match event {
        TurnEvent::Launched => { /* the provider process started */ }
        TurnEvent::Session(handle) => { /* resumable providers only: store it */ }
        TurnEvent::Started => { /* the provider accepted the turn */ }
        TurnEvent::Activity => {}
        TurnEvent::Delta(text) => { /* answer text */ }
        TurnEvent::Source(source) => { /* citation */ }
        TurnEvent::Usage(usage) => { /* cumulative snapshot */ }
        TurnEvent::Ended(outcome) => break, // exactly once per accepted turn
    }
}
```

**Events.**
- **`Launched`**: the provider process started successfully. It feeds the run inspector and the §8 timeline, and doesn't affect any limit.
- **`Session(handle)`**: the opaque native session the provider is running. Only `Persistent` turns send it (§5), and only execution modes that can resume (Claude and Codex) accept those.
  - It arrives before `Started`, so an application that keeps a conversation→session map can store the mapping before it announces the conversation. Pervue's Codex adapter works that way today.
  - It arrives again if the provider later reports a different session. For example, Claude's final `result` can name a session other than the one in `init`. The application then rewrites its mapping.
  - Pervue passes the handle back as the turn's `continuation` to resume. Conclave never uses it.
- **`Started`**: the provider accepted the turn. Its own start-of-turn event arrived and, where the adapter checks one, the `init` boundary passed: the tool set, agent, permission mode, model, and working directory.
  - No answer text comes before it.
  - The start limit runs until it arrives, which catches a CLI that launches and then hangs, such as a signed-out `codex exec` that keeps retrying.
  - Pervue's adapters use this meaning today: Codex at `turn.started`; Claude, Gemini, and Grok after `init`.
- **`Activity`**: the provider is working with nothing to show. It resets the idle timer, under the §5 rule.
- **`Delta`, `Source`, `Usage`**: answer text, a citation, and a cumulative usage snapshot.
- **`Ended(outcome)`**: completed, failed with a code, reason, and `retryable` flag, or cancelled. Every accepted turn gets one, whether or not `Launched` or `Started` came first.

**Turn lifecycle.**
1. **Order.** `start_turn` registers the turn and assigns its ID before it returns and before any provider work starts. Events follow on the turn's channel.
2. **IDs.** The runtime generates each ID from a counter that never repeats for the life of the process. Callers can't choose or reuse IDs, and only running turns are indexed, so no state builds up.
3. **Exactly one ending.**
   - Every turn that `start_turn` accepts produces exactly one `Ended`, whether or not `Launched` or `Started` was emitted. A provider that fails to launch, or hangs before `init`, still ends.
   - A request that `start_turn` rejects produces no events.
4. **Cancellation.** Cancelling a running turn succeeds, repeats included, and the turn still ends only once. Cancelling a turn that has already ended does nothing, because its `Ended` is already in its channel.

```text
start_turn accepts the turn
 │
 ├─ launch fails, or cancelled before launch ─────────────→ Ended
 └─ Launched
     ├─ init fails, start limit passes, or cancelled ─────→ Ended
     ├─ Session (resumable providers, before Started)
     └─ Started
         ├─ Activity, Delta, Source, Usage, Session (any number)
         └─ completes, fails, hits a limit, or cancelled ─→ Ended
```

A runtime loss can end a turn at any point on this diagram (see Supervision below). It still produces exactly one `Ended`.

**Supervision.** Failure outcomes come from code that survives the failure. One supervisor sits beneath both entry points, and a catch boundary surrounds each adapter.

```text
Supervisor (the same one under both entry points)
├─ running turns: ID → event sender, and whether the scheduler received the turn
├─ ID counter
├─ scheduler generation
└─ scheduler: every step runs behind catch_unwind
    └─ provider exchanges: every call runs behind catch_unwind; each owns its process

pervue-host   drives the supervisor from its own synchronous loop
service API   runs the supervisor on a thread of its own, for async servers
```

- **A panic in one adapter.**
  - Every entry into provider-controlled code runs behind a `catch_unwind` boundary:
    - constructing a provider;
    - `status`, `send`, and `forget`;
    - every later `Exchange::next` and `Exchange::cancel` call, which is where most output parsing happens;
    - dropping an exchange.
  - A panic in any of these ends only the affected turn, with a runtime error marked `maybe_started`, or fails only the affected status check.
  - The exchange is dropped, which kills and reaps its process.
- **A panic in the scheduler itself.**
  - Every scheduler step also runs behind `catch_unwind`, called by the supervisor. The supervisor's turn table lives outside the frame that unwinds, so it survives. In `pervue-host`, the loop that owns Native Messaging survives too.
  - After a panic, the supervisor drops the scheduler, which kills and reaps every provider process it owned. It then ends every turn it still holds.
    - A turn is marked `not_started` only if it never reached the scheduler. Every other turn is marked `maybe_started`, because a provider may have started, or even finished, before the panic.
    - The supervisor removes a turn from its table before sending that turn's `Ended`, so no turn ends twice.
  - The supervisor then starts a new scheduler generation.
- **A panic in the supervisor's own code** has no survivor inside the runtime, so each entry point handles it:
  - **Through the `service` API:** the thread's channels close. A turn handle whose channel closes before `Ended` reports `Ended` itself, as a runtime error marked `maybe_started`, so the caller still sees exactly one ending.
  - **In `pervue-host`:** the host process ends, and the extension sees the host disconnect, as it does today.
- **Aborts.** All of this relies on the default `panic = "unwind"`. With `panic = "abort"`, or an abort such as running out of memory, the whole process ends, and the application's own recovery applies. Conclave marks unfinished runs `interrupted` at startup, and Pervue's extension sees the host disconnect.
- **Tests.** Three tests prove this:
  - a fake-provider persona that makes its adapter panic;
  - a test hook that panics a scheduler step, driven both through `pervue-host`'s loop and through the `service` API;
  - a test hook that panics the `service` thread's supervisor.

  Each asserts that every turn ends exactly once with the right marker, and that no provider process survives. For the `pervue-host` case, it also asserts that the host keeps serving new requests.

**Other rules.**
- **Dropping the runtime** cancels every turn, waits out the stop grace, and kills what remains, as Pervue does with `INPUT_CLOSED`.
- **Diagnostics** go to a hook the application supplies. They contain no prompts, output, or account identifiers, as in OBS-01.
- **Versioning.** Consumers pin the runtime to a git revision while it incubates, and to a semver version once it's published.

## 7. Stages

The tracker isn't edited here, and the IDs below are only proposals.

**Before Stage 1: agree.** Nothing here changes code.
- Review this proposal.
- Write ADR-0002.
- Choose the library's name and license.

**Stage 1: harden inside Pervue** (proposed LIB-06 to LIB-09).

The native code is reshaped into the future library while it still lives in Pervue, so Stage 2 is a move rather than a redesign.

1. **Verify what's pending.** Move LIB-01 to LIB-05, PRO-05 to PRO-09, TST-10, and TST-11 to VERIFIED.
2. **Reshape the workspace:**
   - rename `pervue-core` to `runtime-core`;
   - move framing into `pervue-host`;
   - introduce the neutral turn, error, usage, and timeout types (§5);
   - move error wording into a message table in `pervue-host`.
3. **Extract the scheduler** from `host.rs`, and add the absolute limit and the rule for what resets the idle timer. The rule stops a Claude turn from staying alive on unrecognized events.
4. **Add the `service` API,** with its supervisor and panic boundary (§6).
5. **Close the gaps this proposal found:**
   - usage events, and live model lists (`agy models`, Grok's versioned aliases);
   - sign-in classification, including classifying the output of `codex login status`;
   - per-turn cleanup with the runtime's own retry record, for Gemini, Grok, and the session files `codex exec` saves for stateless turns;
   - moving Claude and Codex onto the shared private-file helpers, replacing their copies;
   - in `pervue-host`: **a search turn never resumes a native Claude or Codex session.**
     - The problem: the rule against combining page context with search is checked per turn only, so resuming a session that saw page context earlier gives that page text live search access.
     - The rule: a search turn starts a new `Persistent` session from the bounded dialogue history, which never includes page context. The host then replaces the conversation's mapping with that new session.
     - The superseded session, which may hold page text, is recorded and cleaned up as §4 describes, so deleting the conversation later still removes it.
     - It holds across host restarts without any stored metadata, because it doesn't depend on remembering which sessions saw page context.
     - The replayed history can still hold earlier answers that quoted a page, just as Gemini's and Grok's history does. The rule keeps raw page text out.
6. **Add live smoke tests for Gemini and Grok.** Today only Codex and Claude have them.
7. **Verify the session policies with real CLIs:**
   - After an `Ephemeral` turn, each CLI's stored state holds no prompt. That includes checking that `codex exec --ephemeral` writes neither a rollout nor a state-database entry.
   - A test runs the sequence "page-context turn → search turn → delete the conversation" across a host restart, and confirms that no transcript of either session remains.

**Stage 1 exit bar:**
- every item above is VERIFIED in the tracker;
- all green: the hostile matrix, the panic tests, the contract suite across all four providers, the fuzz targets, the protocol validator, and the golden fixtures;
- the live smoke tests pass for all four providers on current CLI versions;
- no high-severity security finding is open.

**Stage 2: extract the library, with all four adapters at once** (proposed LIB-10).
1. Move these into a new repository with its own CI:
   - the runtime crates: `runtime-core`, `platform`, `providers`, `scheduler`, and `service`;
   - the test assets: the fake provider, the hostile matrix, the contract suite, the panic tests, and the fuzz targets.
2. Switch Pervue to the library, pinned to a git revision. `pervue-host` keeps what §4 leaves it.
3. Keep the library at version 0.x. Expect API changes during Stage 3, and have both applications bump their pin deliberately.

**Stage 2 exit bar:**
- Pervue's full test suite passes against the library, including the live smoke tests;
- the library's own CI is green.

**Stage 3: Conclave integrates all four providers.** This happens in Conclave's repository, following its [adoption proposal][conclave-adoption].
- A problem in a shared adapter is fixed in the library, and both applications pick up the fix by bumping their pin.
- Conclave never patches shared adapter behavior locally. That would let the two apps drift apart again, which is what the extraction is meant to end.

**After Stage 3.**
- Speed work (proposed PRF-01 to PRF-03, §8).
- Publish the crates once the API settles.

**Working alongside the tracker.**
- Stage 1 is largely the remaining tracker work, so coordinate it with whoever holds those items.
- Reshaping the workspace and extracting the scheduler touch `host.rs` and every import of `pervue-core`. Start them after LIB-01 to LIB-05 are verified.
- The Gemini and Grok adapters landed on 2026-09-28, and Gemini was fixed the same day for real `agy` output. Both should settle before Stage 2.

## 8. Speed and cold start

Optimize after both applications run on the library. At that point the library is the one place where turns start, so a measurement or a fix covers every provider in both applications at once.

**What is measured today.**
- TST-09 sets three budgets ([performance.js][p-perf]):
  - the popup ready for input within 100 ms;
  - the native host ready within 250 ms;
  - the first response chunk within 1,500 ms.
- CI enforces the last two against the built host, but only with the fake provider. No real CLI's start-up is measured.

**What a turn costs today in Pervue.**

| Provider | Processes started per turn | Other work before the provider starts |
|---|---|---|
| Codex (`exec`) | 2: `codex login status`, then `codex exec` ([codex/mod.rs:460][p-codex]) | Workspace ownership check |
| Claude | 2: `claude auth status`, then `claude -p` ([claude/mod.rs:433][p-claude]) | Workspace ownership check |
| Gemini | 1: `agy` | Creating a private workspace and writing the agent definition; workspace ownership check |
| Grok | 1: `grok` | Creating a private workspace, `GROK_HOME`, the agent definition, and the prompt file; workspace ownership check |

`codex exec` and Grok's one-shot mode deliver each message whole, so their first visible text waits for the first complete message. No process-level optimization changes that.

**Step 1: measure** (PRF-01).
- For each provider, with real CLIs, record a timeline of each turn: request received → sign-in check done → `Launched` → `Started` → first answer text → turn ended → process exited.
- Record cold runs (the first turn after sign-in or a reboot) and warm runs (repeated turns).
- Run it as an opt-in benchmark, like the live smoke tests, because CI has no signed-in CLIs.

**Step 2: optimize** (PRF-02), roughly in order of expected gain for the risk:
1. **Sign-in checks, within the rule that they never run alongside a turn.**
   - The runtime doesn't cache a check by default.
   - An application may set a caching policy: a successful check is reused for a set time. Any authentication or provider failure invalidates it immediately.
   - The tradeoff has to be stated wherever caching is turned on. Within the cache's lifetime, a CLI that has switched to API-key or Console sign-in goes unnoticed, and a turn could be billed. That's why caching is the application's choice.
   - If the measurements show the check dominates a turn's latency, look for evidence the CLI itself reports before any model request, provider by provider, instead of starting billable work before authorization is established.
2. **Prepare per-turn resources ahead of time**, such as Gemini's and Grok's next private workspace and agent definition. The workspace ownership check must still run at launch, because caching it would open a race.
3. **Keep executable discovery across turns**, and look again only when a launch fails. Today each request does one lookup.

Long-lived provider processes stay out of scope (§4) unless the measurements show process start-up dominates and per-turn tool control can be kept.

**Step 3: budgets** (PRF-03).
- Set targets per provider from the measurements.
- Enforce the runtime's own overhead in CI with the fake provider.
- Track the real-CLI numbers in the opt-in benchmark report.

## 9. Risks

- **Premature generalization.** The §4 promotion rule decides what moves. Keep policy owned by a single application in that application.
- **One large extraction step.** Moving all four adapters at once is a bigger change than moving them one at a time. Two things carry that risk: the Stage 1 exit bar, and Pervue switching to the library before Conclave does.
- **Regressions from extracting the scheduler.**
  - `host.rs` holds protocol v1's lifecycle guarantees, so move it without changing behavior.
  - Keep TST-03, TST-04, and the golden fixtures as the gate.
- **The rename touches every import.** Renaming `pervue-core` to `runtime-core` is mechanical but broad. Do it in one change, and coordinate it with open branches.
- **Supervision is only as good as its tests.** The panic tests (§6) are part of Stage 1, not a follow-up.
- **Stricter checks on Gemini and Grok.**
  - Failing on anything the boundary doesn't expect is the right default: an undocumented Antigravity step type, or a Grok `init` field that doesn't match. But it broke real Gemini turns in Pervue until the 2026-09-28 fix, and any consumer inherits the behavior.
  - Run the live smoke tests for all four providers, including the Gemini and Grok tests Stage 1 adds, against each new CLI release.
- **Codex's state database.** `conversation.forget` doesn't clear it today, and it can hold a thread's first user message. That gap predates this proposal, and it is documented in `native/README.md`. Stage 1 should decide whether Pervue can remove a thread's entry through Codex itself, or documents the gap as accepted.
- **Windows.**
  - There is no Job Object yet ([process.rs][p-process]), so only the provider process itself is stopped, not its descendants.
  - Don't claim process-tree kills on Windows.
- **Licensing.** Pervue has no license yet. The library's is `MIT OR Apache-2.0` (decided 2026-09-29); add the license files and notice when its repository is created, before it is made public or its crates are published.

## 10. Open decisions

1. ~~The library's name.~~ Decided: `seatline` (2026-09-29).
2. ~~The license.~~ Decided: `MIT OR Apache-2.0`.
3. What should the default absolute turn limit be, and can applications raise it? Conclave uses 180 s today, and Pervue has no limit.
4. Should Pervue keep its per-turn sign-in checks for Claude and Codex as they are, or set a caching policy? Pervue's checks guard against a signed-out CLI, not against the wrong kind of billing.

## 11. Revision history

**2026-09-28, first review.**
- **Framing.** Conclave justifies a boundary between applications.
- **Boundary.** Session maps, managed continuation, and transcript removal stay in `pervue-host`, behind a continuation hook.
- **Protocol.** The turn lifecycle is specified.
- **Runtime loss.** Errors are marked `maybe_started` when a turn may have run.
- **Namespace.** It is fixed at start and described as accident protection.
- **Usage.** Defined as cumulative snapshots.
- **Timeouts.** An absolute limit and an idle-timer rule.
- **Releases.** Independent versioning.
- **Phases.** Scoped to each migration.

**2026-09-28, after Gemini landed.**
- **Evidence.** Updated to three providers.
- **Failures.** Corrected the claim about typed failures.
- **One process per turn.** The rule for every provider; `codex app-server` dropped.
- **Pilot.** Gemini goes first.
- **Cleanup.** Split into per-turn and per-conversation scopes.
- **Speed.** New section on cold start and speed.

**2026-09-28, Rust server for Conclave.** The proposal adopted a Rust server for Conclave with an in-process API, and found Conclave's authentication and Host-header gaps.

**2026-09-28, second review and Grok.**
- **Separated decisions.**
  - This document now covers only the runtime and ADR-0002.
  - Conclave's adoption, including the Rust server, its go/no-go, its security gaps, and the HTTP contract, moved to the [Conclave adoption proposal][conclave-adoption].
- **Sign-in checks.** They never run alongside a turn. Caching is an explicit policy of the application, invalidated by any failure, with its billing tradeoff stated (§5, §8).
- **Supervision.** A supervisor outside the scheduler thread, and `catch_unwind` around each adapter, produce the runtime-loss outcomes. Panic tests are part of Phase 1 (§6).
- **IDs.** The runtime generates turn IDs, so no history of used IDs builds up (§6).
- **Crate ownership.** `pervue-core` becomes `runtime-core` inside the runtime workspace. Applications never depend on Pervue's crates (§4).
- **Events.** `Started` keeps its current meaning, "the provider accepted the turn", because the start limit depends on it. A new `Launched` event marks the moment the process starts (§6).
- **Grok.** It landed in Pervue in one-shot headless mode, so it now moves into the runtime as the second stateless provider. The evidence is updated to Pervue `fc7284b` (§1, §7).

**2026-09-28, third review.**
- **Exactly one ending.** Every turn `start_turn` accepts ends exactly once, even if the provider never launches or never reaches `Started`. A lifecycle diagram shows every path (§6).
- **Second-consumer gate.** Phase 1 now only prepares the runtime inside Pervue. Each provider phase waits until a second application has accepted a plan to use that execution mode, starting with Gemini (§7).
- **Panic boundary.** It covers every entry into provider-controlled code, including every `Exchange::next` and `Exchange::cancel` call, not just the first call into an adapter (§6).
- **Environment extensions.** They come only from trusted startup configuration, never from per-turn input (§4).
- **Execution modes.** The runtime shares one adapter per execution mode, not per CLI. An application may keep a different mode of the same CLI outside the shared set (Summary, §4).

**2026-09-28, three-stage plan.**
- **Stages.** Three stages, each with an exit bar, replace the provider-by-provider phases:
  1. harden inside Pervue;
  2. extract the library with all four adapters at once, with Pervue switching to it first;
  3. Conclave integrates all four providers.

  The Gemini-first pilot is dropped (§7).
- **Second consumer.** Conclave committed to the library's mode for all four CLIs, including `codex exec` and Grok's one-shot headless mode, so all four adapters move together (§4).
- **Repository.** The library moves to its own repository in Stage 2 (Summary, §7).
- **Host-side hardening.** Stage 1 now includes a fix in `pervue-host`. A search turn must not resume a Claude or Codex session that carried page context in an earlier turn (§7).
- **Conclave's proposal.** It moved to the Conclave repository, where it belongs.

**2026-09-28, PR review (davletovb/pervue#40).**
- **Session event.** A new `Session(handle)` event carries a resumable provider's native session. It comes before `Started`, and again if the session changes, so `pervue-host` can keep its conversation→session map (§4, §6).
- **One supervisor for both entry points.** The same supervisor now sits beneath both `pervue-host`'s loop and the `service` thread. Every scheduler step runs behind `catch_unwind`, so a scheduler panic no longer unwinds the loop that owns Native Messaging. A turn handle whose channel closes before `Ended` reports `Ended` itself (§6).

**2026-09-28, second PR review (davletovb/pervue#40).**
- **Session policy.** Every turn now states `Ephemeral` or `Persistent`, so a shared adapter knows whether to keep the provider's native session. Pervue's first Claude or Codex turn keeps it, and every Conclave call leaves nothing behind (§5, §6).
- **Search turns.** A search turn never resumes a native Claude or Codex session. It starts a new one from the bounded dialogue history, which never includes page context, and the host replaces its mapping. The protection survives host restarts without stored metadata (§7).

**2026-09-28, third PR review (davletovb/pervue#40).**
- **Superseded sessions.** When a conversation's session is replaced, by a search turn or by Claude reporting a different session, the host records the old handle durably and removes its transcript. `conversation.forget` removes every session a conversation has used. A new test covers "page-context turn → search turn → delete" across a host restart (§4, §7).
- **Ephemeral Codex.** `Ephemeral` `codex exec` turns use `--ephemeral`. Deleting rollout files isn't accepted as a fallback, because Codex's state database can keep the first user message. Every mode's `Ephemeral` behavior is verified by its live smoke test, and a mode that can't meet it refuses `Ephemeral` (§4, §5, §7).

**2026-09-29, LIB-10 preparation (in-tree).**
- **Boundary.** Before the move, the adapters were made to depend on nothing else in `pervue-host`: a namespaced `Layout` for every path, runtime-only error codes and capabilities (with `tool_isolation`, which Codex already computed as page context), a Turn-based `Provider`, and the `conversations` layer that owns everything about conversations (§4).
- **Tool policy.** A third value, `ProviderDefault`, so a plain Pervue turn stays unrestricted (§5).
- **Session loss.** A resumed turn that can't resume says `SessionLost` (`Confirmed` or `Suspected`), and the application chooses whether to rebuild from history; Claude and Codex recover differently, as they already did (§6).
- **Cleanup.** Per-turn cleanup records are grouped by an opaque `cleanup_group` the application supplies, which keeps Gemini's record layout (§4).

**2026-09-29, LIB-10 step 2 (in-tree).**
- **Crates.** The platform layer is `runtime-platform` and the adapters are `runtime-providers`, beside `runtime-core`, the scheduler and the service. None depends on `pervue-host`, and a CI job (`scripts/check-runtime-independence.mjs`) fails if any crate not named `pervue*` depends on one that is.
- **Tests and fuzzing.** The fake provider is a library (`runtime-fake-provider`) with a small binary in each package that runs it. `runtime-tests` holds the adapter tests at the runtime's level (a `Turn` in, `Update`s out), a provider contract that all four adapters meet through the `Provider` trait alone, the hostile matrix under the scheduler's supervisor, the threaded service against real adapters, and the process and stream tests. Pervue's `test_provider` keeps what maps conversations onto turns, the host's hostile matrix, and the contract as Pervue serves the providers. `stream_lines` moved to `runtime-fuzz`, which depends on `runtime-core` alone; `frame_reader` and `protocol` stay with Pervue.

[p-claude]: ../../native/providers/src/claude/mod.rs
[p-claude-output]: ../../native/providers/src/claude/output.rs
[p-codex]: ../../native/providers/src/codex/mod.rs
[p-codex-output]: ../../native/providers/src/codex/output.rs
[p-gemini]: ../../native/providers/src/gemini/mod.rs
[p-grok]: ../../native/providers/src/grok/mod.rs
[p-private-fs]: ../../native/platform/src/private_fs.rs
[p-env]: ../../native/platform/src/environment.rs
[p-workspace]: ../../native/platform/src/workspace.rs
[p-forget]: ../../native/platform/src/forget.rs
[p-host]: ../../native/host/src/host.rs
[p-process]: ../../native/runtime-core/src/process.rs
[p-native-connection]: ../../extension/src/background/native-connection.js
[p-perf]: ../../extension/src/shared/performance.js
[c-core]: https://github.com/davletovb/conclave/blob/1379a37ccb13d484247122ee215257e642b166fc/packages/core/src/index.ts
[c-classify]: https://github.com/davletovb/conclave/blob/1379a37ccb13d484247122ee215257e642b166fc/apps/server/src/orchestrator.ts#L98-L130
[c-usage]: https://github.com/davletovb/conclave/blob/1379a37ccb13d484247122ee215257e642b166fc/apps/server/src/orchestrator.ts#L437-L455
[c-watchdog]: https://github.com/davletovb/conclave/blob/1379a37ccb13d484247122ee215257e642b166fc/apps/server/src/orchestrator.ts#L507
[c-claude-auth]: https://github.com/davletovb/conclave/blob/1379a37ccb13d484247122ee215257e642b166fc/apps/server/src/providers/anthropic-claude.ts#L235
[c-claude-argv]: https://github.com/davletovb/conclave/blob/1379a37ccb13d484247122ee215257e642b166fc/apps/server/src/providers/anthropic-claude.ts#L254
[c-claude-buf]: https://github.com/davletovb/conclave/blob/1379a37ccb13d484247122ee215257e642b166fc/apps/server/src/providers/anthropic-claude.ts#L154
[c-claude-kill]: https://github.com/davletovb/conclave/blob/1379a37ccb13d484247122ee215257e642b166fc/apps/server/src/providers/anthropic-claude.ts#L139
[c-grok-kill]: https://github.com/davletovb/conclave/blob/1379a37ccb13d484247122ee215257e642b166fc/apps/server/src/grok/acp-client.ts#L150
[c-codex-env]: https://github.com/davletovb/conclave/blob/1379a37ccb13d484247122ee215257e642b166fc/apps/server/src/codex/app-server-client.ts#L64
[conclave-adoption]: https://github.com/davletovb/conclave/blob/claude/eloquent-franklin-8qo1f1/docs/proposals/provider-runtime-adoption.md
