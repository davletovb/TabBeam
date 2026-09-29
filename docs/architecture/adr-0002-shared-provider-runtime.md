# ADR-0002: Extract a shared provider runtime as an in-process Rust library

**Status:** Accepted — Stage 1 implemented, verification in progress  
**Date:** 2026-09-28  
**Decided by:** Project owner, pending acceptance  
**Supersedes:** the C ABI path for non-Rust consumers in [ADR-0001](adr-0001-native-host-in-rust.md) (the last bullet of both Decision and Consequences) and in framework §9.7  
**Details:** the [provider runtime proposal](provider-runtime-extraction-proposal.md)  
**To be implemented by:** Stages 1 and 2 of that proposal (proposed LIB-06 to LIB-10)

## Context

**What exists.** Milestone F extracted `pervue-core` (`process`, `stream`, `discovery`, `exchange`, `protocol`, `framing`), and `pervue-host` is its only consumer. Since then, Pervue has grown to four providers: Codex, Claude, Gemini (through Antigravity), and Grok. The last two landed without any change to the `Provider` trait, `host.rs`, the protocol, or `pervue-core`. Much of the provider plumbing is still host-owned, though:
- the environment allowlist;
- private workspaces;
- per-turn cleanup;
- the request loop's timeouts, cancellation, and fairness.

**A second application now needs it.** [Conclave](https://github.com/davletovb/conclave) drives the same four CLIs from its own TypeScript adapters, with weaker process handling:
- a prompt passed on the command line;
- unbounded output buffers;
- a single SIGTERM to stop a call;
- a denylisted environment;
- CLIs running in the server's own working directory.

Conclave has decided to adopt a shared runtime for all four providers at once, in a new Rust server. Its [adoption proposal][conclave-adoption] records that decision.

**The extraction rule.** Framework §9.6 promotes a component to the reusable library when two applications need it. Conclave meets that bar.

**The C ABI path.** ADR-0001 expected a C ABI for any non-Rust consumer. Conclave is choosing a Rust server, so it doesn't need one. The review of this proposal also found better ways to serve a non-Rust consumer than a C ABI.

**Performance wasn't a deciding factor.** Model latency dominates, as ADR-0001 found.

## Decision

**1. A provider runtime library.** Pervue's provider-execution code becomes a Rust crate workspace with five crates:
- `runtime-core`: process, stream, and discovery code, plus neutral turn, error, and usage types. It is today's `pervue-core`, minus framing and Pervue's protocol vocabulary.
- `platform`: environment construction, private files and workspaces, and per-turn cleanup with its retry record.
- `providers`: one shared adapter per supported execution mode. Today those are `codex exec`, Claude's print mode, Antigravity's one-shot mode, and Grok's one-shot headless mode.
- `scheduler`: concurrent turns; start, idle, and absolute limits; cancel → stop → kill; fairness; delta splitting. It also holds the supervisor that both entry points use.
- `service`: an in-process API for async servers, which runs the supervisor on a thread of its own.

**2. The boundary.** The library owns provider execution mechanics. Each application keeps its conversation and product policy.
- `pervue-host` keeps:
  - Native Messaging framing, origin checks, and the manifest;
  - protocol v1 and browser-context policy;
  - conversation-to-session maps and the continuation policy;
  - removing transcripts when a conversation is deleted, for every session it has used, with superseded sessions recorded durably and cleaned up when replaced;
  - diagnostics and all user-facing wording.
- The library returns failures as a code, a reason, and a `retryable` flag. Each application writes its own messages, so protocol v1 doesn't change.
- Resumable providers take and report native sessions as opaque handles.
- A search turn in Pervue never resumes a native Claude or Codex session. It starts a new one from the bounded dialogue history, which never includes page context, so the protection needs no stored metadata. The superseded session is cleaned up like any other.
- Applications depend only on the library's crates, never on Pervue's. An application's own adapters build on `runtime-core`.

**3. In-process consumption.**
- `pervue-host` drives the supervisor, and through it the scheduler, from its existing synchronous loop.
- Async servers use the `service` API.
- If a non-Rust application ever needs the library, it gets a sidecar executable that wraps the `service` API and speaks a versioned protocol over stdio.
- There is no C ABI and no Node addon.

**4. Execution rules.**
- **One process per turn,** for every shared adapter. The library adopts no long-lived provider server.
- **Every turn states a session policy.**
  - `Ephemeral`: nothing the provider saves outlives the turn.
    - It uses a CLI flag where one exists (Claude's `--no-session-persistence`, `codex exec --ephemeral`) and per-turn cleanup otherwise.
    - Each mode's live smoke test verifies that no prompt content remains in anything the CLI stores.
    - A mode that can't meet that refuses `Ephemeral` rather than degrading.
  - `Persistent`: the provider keeps its native session and reports it. Only modes that can resume (Claude, Codex) accept it, and a turn that carries a continuation handle is always `Persistent`.

  Conclave's turns are always `Ephemeral`.
- **Every turn states its tool policy.** `None` runs the provider with no tools, for text the application doesn't control; an adapter that can't guarantee that (see `Capabilities::tool_isolation`) refuses the turn. `NativeWebSearch` allows only the provider's own search. `ProviderDefault` leaves the user's own provider configuration in charge, for text the application wrote itself. The third value exists because Codex restricts tools only on turns that carry page text or search: a plain turn must keep working for a user with MCP servers configured, which a neutral `None` that means "restricted" would refuse.
- **Sign-in checks** that an application requests finish before a turn's provider process starts, and never run alongside it. The library doesn't cache them. Caching is an explicit policy of the application, and any authentication or provider failure invalidates it.
- **Each application's namespace is fixed when it starts the runtime.** Its workspaces and cleanup records live under that namespace.
- **The environment allowlist** can be extended only through trusted configuration read at startup. It never takes names from per-turn input: requests, pages, prompts, or model output.
- **The scheduler supports an absolute turn limit for every turn.** The application chooses the value. Stage 1 preserves Pervue's pre-ADR effective behavior by using an unbounded practical default (`Duration::MAX`) until the default-limit open question below is decided; explicit callers and tests can set a finite bound. The idle timer resets only on deltas, sources, and provider events that the adapter recognizes as work.

**5. The turn contract.**
- **IDs.** The runtime generates turn IDs from a counter that never repeats for the life of the process. `start_turn` registers the turn before it returns.
- **Exactly one ending.** Every turn that `start_turn` accepts produces exactly one `Ended`, whether or not it reached `Launched` or `Started`. A request that `start_turn` rejects produces no events.
- **`Launched` and `Started`.**
  - `Launched` means the provider process started.
  - `Started` means the provider accepted the turn and, where the adapter checks one, its `init` boundary passed. No answer text comes before it, and the start limit runs until it arrives.
- **Native sessions.** A provider that can resume (Claude, Codex) reports its native session as an opaque handle in a `Session` event. A turn that resumes one it can't use reports `SessionLost` (`Confirmed` when the provider said it is gone, `Suspected` when the run merely ended before the turn began) before its terminal failure, and the application decides whether to start over.
  - The event comes before `Started`, so the application can store its mapping before announcing the conversation.
  - It comes again if the provider reports a different session later.
  - Only `Persistent` turns send it.
- **Cancellation.** Cancelling a running turn is idempotent, and cancelling a turn that has ended does nothing.
- **Supervision.** One supervisor sits beneath both entry points: `pervue-host`'s loop and the `service` thread. It owns the table of running turns, outside the frames that can unwind.
  - **Adapter panics.** Every entry into provider-controlled code runs behind `catch_unwind`. That covers constructing a provider; `status`, `send`, and `forget`; every `Exchange::next` and `Exchange::cancel`; and dropping an exchange. A panic there ends only the affected turn.
  - **Scheduler panics.** Every scheduler step also runs behind `catch_unwind`. If one panics, the supervisor drops the scheduler, which kills its processes, and ends every turn it holds. A turn is marked `not_started` only if it never reached the scheduler, and `maybe_started` otherwise. The supervisor then starts a new scheduler. In `pervue-host`, the loop that owns Native Messaging keeps running.
  - **Supervisor panics.** A panic in the supervisor itself on the `service` thread closes its channels. A turn handle whose channel closes before `Ended` then reports `Ended` itself, marked `maybe_started`.
  - All of this relies on `panic = "unwind"`.

**6. The promotion rule.**
- A shared adapter exists for an execution mode that two applications use.
- A different mode of the same CLI that only one application uses stays with that application, built on `runtime-core`. Codex app-server and Grok ACP are examples.
- Conclave has committed to all four of the library's modes, so all four move together.

**7. Delivery.**
- **Stage 1:** harden inside Pervue.
- **Stage 2:** extract the library, with all four adapters, into its own repository. Pervue switches to it first.
- **Stage 3:** Conclave integrates it.

Both applications pin the library to a git revision, and it stays at version 0.x until its API settles. A problem in a shared adapter is fixed in the library, never patched locally by one application.

## Alternatives considered

- **Keep `pervue-core` internal, and leave Conclave on its TypeScript adapters.** This keeps two implementations of the hardest code, and they drift apart. Node also can't reproduce some of the library's guarantees. For example, it can't kill a process group before reaping the child.
- **A C ABI, as ADR-0001 expected.** This brings FFI, `unsafe` binding code, manual ownership, and a versioned C façade with cross-version tests, all for a consumer that no longer needs it.
- **A Node addon (napi-rs).** The binding crate would need an exemption from `forbid(unsafe_code)`. A panic or abort would take down the Node server. Reaping children would also share the process with libuv's own `SIGCHLD` handling, an interaction nobody has tested.
- **A sidecar as the main way to use the library.** This is viable, and it remains the path for non-Rust consumers and Conclave's fallback. It isn't the main path, because Conclave chose a Rust server.
- **Long-lived provider servers** (Codex app-server, Grok ACP, or Claude kept alive per conversation). These were rejected for five reasons:
  - Pervue chooses each turn's tools with launch flags, and a long-lived process fixes them when it starts.
  - A process kept alive for a conversation carries earlier turns' page text into later turns.
  - A crash of a shared process fails every running turn.
  - Cancelling becomes a request the CLI has to honor, instead of a kill.
  - Pervue's host doesn't live long enough to keep a process warm.
- **Extracting one provider at a time, Gemini first.** This was replaced by hardening everything inside Pervue and extracting it in one step. The Stage 1 exit bar, and Pervue switching to the library before Conclave does, carry the risk of the larger step. Conclave also switches all four providers at once anyway.

## Consequences

- **Conclave's providers become as hardened as Pervue's,** in all four modes:
  - prompts that never go on the command line;
  - bounded memory;
  - process-group kill with escalation;
  - an allowlisted environment;
  - private workspaces;
  - typed failures.
- **Pervue gets usage events, live model lists, sign-in classification, and scheduler support for an absolute turn limit.** Stage 1 does not silently choose the unresolved product default; Pervue keeps its previous effective duration behavior until that value is decided. Unrecognized provider events do not reset the idle timer.
- **Both applications share the fake provider, the hostile-process matrix, the contract suite, and the panic tests.**
- **Renaming `pervue-core` to `runtime-core` touches every import,** and extracting the scheduler changes `host.rs`, the host's most heavily tested file.
- **Pervue depends on another repository.** Pin bumps and API churn during 0.x are the cost.
- **Claude's and Codex's native session IDs cross a crate boundary** they don't cross today, though only as opaque values.
- **The environment allowlist moves out of the host.** That reverses what `native/core/README.md` says about it.
- **Three documents are updated when Stage 1 lands:** `native/core/README.md`'s compatibility section, framework §9.7, and ADR-0001's C ABI bullets.
- **Windows still has no Job Object,** so only the provider process itself is stopped. The library doesn't claim process-tree kills on Windows.

## Verification

- **The Stage 1 exit bar:**
  - the pending tracker items (LIB-01 to LIB-05, PRO-05 to PRO-09, TST-10, TST-11) and every Stage 1 item are VERIFIED;
  - the hostile matrix, the panic tests, the contract suite across all four providers, the fuzz targets, the protocol validator, and the golden fixtures are green;
  - live smoke tests pass for all four providers on current CLI versions;
  - no high-severity security finding is open.
- **The Stage 2 exit bar:** Pervue's full test suite passes against the extracted library, including the live smoke tests, and the library's own CI is green.

## Not decided here

- The default absolute turn limit.
- Whether Pervue caches its sign-in checks.

The [runtime proposal §10](provider-runtime-extraction-proposal.md#10-open-decisions) tracks these.

## Amendments

- **2026-09-29, LIB-10 preparation.**
  - The tool policy gained `ProviderDefault` (see Decision 4), and the neutral turn gained an opaque `cleanup_group` that groups a turn's per-turn cleanup records so an application can retry a group together. Pervue passes the conversation ID, which keeps Gemini's existing on-disk record layout, so installed hosts lose no pending deletion.
  - The conversation, its session map, the superseded-session records, and the choice between resuming and replaying the dialogue live in `pervue-host`'s `conversations` layer, above the adapters, as Decision 2 says. The runtime's `Update` no longer has conversation variants.
  - A lone plain first question is sent as it is to every provider, as it always was to Claude and Codex; Gemini and Grok used to add a "Current user question:" label to it.
- **2026-09-29, LIB-10 step 2.**
  - The runtime is now made of crates that need nothing of Pervue: `runtime-core`, `runtime-platform` (environment allowlist, private files and workspaces, discovery policy, layout), `runtime-providers` (the `Provider` trait and the four adapters), the scheduler and the service, with their fake provider (`runtime-fake-provider`), tests (`runtime-tests`) and fuzz target (`runtime-fuzz`). CI fails if a crate not named `pervue*` depends on one that is.
  - The environment allowlist moved out of the host, as the risks above expected.
  - A namespace is refused when Windows would reserve it as a device name, and the service queues at most one unread `Activity` update per turn, so neither a consumer's choice of name nor a slow reader can break the runtime.
  - The provider contract now runs against all four adapters, at the runtime's level and as Pervue serves them, which closes the part of the LIB-09 gap that concerned Gemini and Grok in that suite.
- **2026-09-29, before step 3.**
  - **Name.** The library is called `seatline`: the seat is the subscription a user is already signed in with, and the line is the line-oriented output of the provider CLIs it reads. At step 3 its crates become `seatline-core`, `seatline-platform`, `seatline-providers`, `seatline-scheduler` and `seatline-service` (all free on crates.io on 2026-09-29), replacing the `runtime-*` and `provider-runtime-*` names, and `scripts/check-runtime-independence.mjs` and the docs follow.
  - **License.** The library is licensed `MIT OR Apache-2.0`, as most Rust libraries are, so a consumer may take either. The crates' manifests say so now; the license files, with their copyright notice, are added when the repository is created.
  - **System prompts.** `Turn::system` is sent by all four adapters and never on a command line, where anyone on the machine could read it. Codex, Claude and Grok get it as the first part of the prompt on the channel that already carries the question, under a plain introduction ("Instructions for this conversation, from the application:"): no provider CLI takes one without either exposing it or depending on a flag this runtime cannot check. Antigravity (Gemini) gets it in the system prompt of the agent the adapter writes for each turn, because real runs showed its model reads instructions in a message that claim to "take precedence" as a prompt injection, and refuses them. An application that resumes native sessions sends `system` on a session's first turn only, and one that sets a system prompt and asks for web search means the two to agree.
  - **Names.** The Gemini agent names and the Grok owner file and agent name come from the application's namespace (`gemini::agent_name`, `grok::owner_file`), so no runtime crate says `pervue` in what it does. Pervue's namespace gives the names it has always used, pinned by tests in `pervue-host`.
  - **Live smoke tests.** Gemini and Grok have them, at the runtime's level (`live_gemini`, `live_grok`), with workflows that start them by hand; they have not yet run against the real CLIs.

[conclave-adoption]: https://github.com/davletovb/conclave/blob/claude/eloquent-franklin-8qo1f1/docs/proposals/provider-runtime-adoption.md
