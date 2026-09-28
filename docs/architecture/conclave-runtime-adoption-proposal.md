# Proposal: how Conclave adopts the provider runtime

**Status:** Proposed, for discussion. Once agreed, it moves into the Conclave repository as Conclave's ADR  
**Date:** 2026-09-28  
**Depends on:** the [provider runtime proposal](provider-runtime-extraction-proposal.md), which doesn't depend on this one  
**Evidence:** Conclave at `1379a37`, Pervue at `fc7284b`

## Summary

- **The decision here is how Conclave uses the provider runtime.**
  - The project owner prefers rewriting Conclave's server in Rust, linking the runtime in-process.
  - The alternative is to keep the Node server and use the runtime through a sidecar.
  - Either choice makes Conclave the runtime's second consumer, and the runtime proposal stands whichever is chosen.
- **The Rust server has its own go/no-go.** It rests on evidence: a finished runtime Phase 1, a recorded HTTP contract, and a spike that ports one slice of the server (§4). If the answer is no, Conclave takes the sidecar.
- **Two things to do now, whatever the choice** (§3):
  - patch the current server's security gaps (no authentication, no Host-header check);
  - record its HTTP contract.
- **The subscription-only guarantee is kept.**
  - A sign-in check that authorizes billing finishes before every model call that needs one, and never runs alongside it.
  - Caching that check is an explicit Conclave setting, off by default (§7).
- **Codex and Grok each need a decision.** The runtime's Codex (`exec`) and Grok (one-shot) adapters deliver answers a message at a time. Conclave's current app-server and ACP adapters stream text token by token (§7).

## 1. Conclave today

- **Size.** The server is 5,712 lines of TypeScript plus 3,440 lines of tests. The orchestrator alone is 1,746 lines, covering 12 modes and custom workflow graphs.
- **Shared types.** The web app (React and TypeScript) shares types with the server through `packages/core`, which 15 web files import.
- **Providers.** Four adapters, whose process handling the [runtime proposal §1](provider-runtime-extraction-proposal.md#conclave) describes.
- **Event streams.** Events reach the browser as newline-delimited JSON over HTTP, with `after=<seq>` replay. Runs belong to the server and outlive the browser tab.
- **Storage.** Runs and conversations live in `~/.conclave/state.json` and `runs/*.ndjson`, with 0700/0600 permissions.
- **Security gaps** ([index.ts][c-index]):
  - there's no authentication, only a CORS allowlist of browser origins, so any local process can drive the API;
  - there's no Host-header check, so the server is exposed to DNS rebinding. A malicious website can rebind its domain to `127.0.0.1`, then read conversations and start runs as if it were same-origin. Recent Chrome versions block some of this, but not every browser does.
- **Per-call and per-page-load processes.**
  - `claude auth status` runs before every Claude call ([anthropic-claude.ts:235][c-claude-auth]).
  - `/providers` starts `claude auth status`, `agy models`, and a whole Grok ACP process on every request. `/models` starts similar work.

## 2. Options

| Option | What changes |
|---|---|
| **Rust server** (preferred) | The server is rewritten in Rust and links the runtime through its service API. The web app and the HTTP API stay the same |
| **Sidecar** | The Node server stays. The runtime runs as a separate process, a thin wrapper around its service API, and replaces the provider adapters one at a time |
| **Keep the TypeScript adapters** | No runtime. The worst provider problems are fixed by hand: prompts on stdin, kill escalation, bounded buffers |

**Rust server**
- For:
  - All provider code lives in one language, and every provider gets the runtime's hardening. That includes any Conclave-only adapter, which builds on `runtime-core`.
  - No sidecar, no second protocol, no per-platform npm packages, and no version pinning across two languages.
  - One binary that also serves the built web app. Users need neither Node nor pnpm; Node is only needed to build the web app.
  - Pervue's fake provider and hostile-process matrix run in-process against Conclave's orchestration.
  - The search-result sanitizer can be shared with Pervue.
- Against:
  - A large port. About 4,200 lines of server code and about 2,300 lines of non-provider tests. Subtle behavior must match exactly, and stored data must still load.
  - The web app's types must be generated from Rust, or they drift.
  - Slower iteration on the product logic that changes most (orchestration modes, workflows, prompts), and contributors need Rust for everyday server work.
  - Less isolation than a separate process. The runtime's supervisor contains panics, but an abort, such as running out of memory, takes the server down.
  - No meaningful speed gain from the language itself, because model latency dominates.

**Sidecar**
- For:
  - The smallest change: only the provider adapters are replaced, one at a time behind a flag.
  - A runtime crash can't take down the server.
  - Little extra runtime work, because the sidecar only wraps the service API.
- Against:
  - A binary has to reach `pnpm dev` users, and a second protocol has to be versioned.
  - A lost runtime process adds its own failure mode.
  - Conclave-only adapters stay in TypeScript, with their current process handling.

**Keep the TypeScript adapters**
- For:
  - Nothing new to ship.
- Against:
  - The hardest process code has to be duplicated.
  - Node can't reproduce some of Pervue's guarantees. For example, it can't kill a process group before reaping the child.
  - The two apps' provider knowledge keeps drifting apart.

## 3. Do now, whatever the choice

1. **Patch the current server's security gaps:**
   - a per-install token on every request;
   - a Host-header allowlist;
   - explicit request-body limits.

   These don't depend on the language, the gaps are live, and the port would take months.
2. **Record the HTTP contract** from the current server, running on the mock provider. The recording is a regression suite for any option, and it's the parity test for a Rust server.
   - **Every endpoint:**
     - status codes;
     - response headers, including `Content-Type` and `Content-Disposition` for exports;
     - CORS responses, for allowed and denied origins and preflights;
     - after the patch, authentication and Host checks;
     - body-size rejection;
     - how invalid bodies and unknown fields are handled.
   - **Event streams:**
     - NDJSON framing and event order;
     - reconnecting with `after=<seq>`;
     - each event flushed promptly rather than buffered, measured with timestamps;
     - cancelling a run while its stream is open;
     - what happens when the client disconnects.
   - **Every orchestration mode:** the event sequences, the errors, and the files stored under the data directory.

Patching first makes authentication, Host checks, and body limits part of the recorded contract, so a Rust server has to match them rather than reinvent them.

## 4. Deciding on the Rust server

**Evidence to gather:**
1. **Runtime Phase 1 is done.** The service API and its supervisor pass the hostile matrix and the panic tests.
2. **The contract is recorded** (§3).
3. **A spike ports one slice** of the server into Rust:
   - storage;
   - the conversation and run endpoints;
   - one orchestration mode, such as Panel;
   - Gemini through the runtime.

   It must pass the contract fixtures for what it covers, and it measures how long the port takes.
4. **The goals behind the rewrite still hold:**
   - installing Conclave without Node;
   - maintainers working in Rust day to day;
   - orchestration stable enough that slower iteration is acceptable.

**Go** if the spike's effort, extrapolated to the whole server, is acceptable and the goals hold. **No-go** means the sidecar (§6), with the Node server kept.

## 5. The Rust server, if go

**Same functionality.** The web app keeps talking to the same HTTP API.

| Today (TypeScript) | Rust server |
|---|---|
| Fastify routes and CORS | axum plus tower-http, with the same routes and JSON shapes |
| Newline-delimited JSON event streams with `after=<seq>` replay | A streamed response body: same format, same replay, flushed per event |
| Orchestrator (12 modes, workflow graphs, budgets, retries) | tokio tasks for parallel steps, plus cancellation tokens |
| Stall watchdog plus 180 s per-adapter timeouts | The runtime's idle and absolute limits. `CONCLAVE_STEP_STALL_TIMEOUT_MS` and `CONCLAVE_*_TURN_TIMEOUT_MS` map onto them |
| Retry decisions by regex on error text | The runtime's `retryable` flag and reason codes |
| `state.json` and `runs/*.ndjson`, with 0700/0600 permissions | Same file formats, so existing data still loads |
| SearXNG client | reqwest, with the search-result sanitizer shared with Pervue |
| Provider adapters | The runtime's adapters, plus any Conclave-only adapter (§7) built on `runtime-core` |
| Web app served by Vite in development | The server also serves the built web app |

**Web app types.**
- The server's Rust types become the source of truth, and the TypeScript types in `packages/core` are generated from them, for example with ts-rs.
- CI fails if the generated types differ from the committed ones.

**Security.**
- From the runtime, for every provider:
  - prompts that never go on the command line;
  - bounded memory;
  - process-group kill with escalation;
  - an allowlisted environment;
  - private workspaces and absolute executable paths;
  - per-turn cleanup;
  - strict `init` checks.
- From the server: the §3 fixes, and request bodies parsed into typed structs.
- Rust itself adds less than it sounds, because JavaScript is memory-safe too.

**Speed.** Rust handles each event in microseconds and uses tens of MB less memory than Node, but users won't notice either. The visible gains:
- Cache provider status and model lists, and refresh them in the background instead of starting CLIs on every page load. These don't authorize a model call, so caching them doesn't weaken the subscription guarantee (§7).
- Runtime-wide improvements, in the [runtime proposal §8](provider-runtime-extraction-proposal.md#8-speed-and-cold-start).

**Switching over.**
- During the port, the Rust server runs alongside the TypeScript server on another port.
- It becomes the default once three things hold:
  - it passes the whole contract;
  - it serves all four providers;
  - it loads real `~/.conclave` data.
- The TypeScript server stays available for one release as a fallback, and is then removed.

## 6. The sidecar, if no-go

- The runtime proposal's service API is wrapped in a sidecar executable. It speaks JSON-RPC over stdio and takes its namespace as a launch argument.
- The protocol follows the runtime's turn-lifecycle rules. Every method gets a response, and a lost runtime process fails the turns it was running, marked `maybe_started`.
- A TypeScript client replaces each adapter behind a flag, such as `CONCLAVE_RUNTIME_PROVIDERS=google`. The existing adapter stays the default until its parity checks pass.
- The runtime has its own version and release tags. Per-platform npm packages ship the binary, and Conclave pins an exact version.

## 7. Provider decisions

**Sign-in checks and the subscription-only guarantee.**
- **Where a check is needed.**
  - For Claude and Codex, the guarantee needs a check before every model call, because either CLI may be signed in with an API-key, Console, or cloud account. The runtime classifies the sign-in and Conclave refuses anything but a subscription.
  - For Gemini and Grok, the launch itself enforces it. API-key variables are never passed, and Grok also runs with `GROK_DISABLE_API_KEY_AUTH`.
- **The check never runs alongside a model call.** It finishes before the provider process starts.
- **Caching the check is a Conclave setting, off by default.**
  - When on, a successful check is reused for a short set time, and any authentication or provider failure invalidates it immediately.
  - The setting's description states the tradeoff: within that time, a CLI that has switched to API-key or Console sign-in goes unnoticed, and a call could be billed.
- **If the check turns out to dominate latency,** look for evidence the CLI reports before any model request, provider by provider, rather than starting billable work early.

**OpenAI.**
- **`codex-exec` through the runtime** is simpler, but loses:
  - token-by-token streaming;
  - the live model list (`model/list`), so the model picker needs a list kept by hand;
  - the ChatGPT usage panel (`/provider-limits`, fed by `account/rateLimits/read`), which would show as unavailable.

  The subscription check moves to classifying `codex login status`.
- **A Conclave-only app-server client, built on `runtime-core`,** keeps those features. Conclave's turns never change tools, so the runtime's one-process-per-turn reasons weigh less there.

**Grok.**
- **The runtime's one-shot adapter** brings:
  - a private `GROK_HOME`;
  - denial of Grok's MCP umbrellas;
  - a verified `init` boundary.

  It delivers answers a message at a time and doesn't support web search. Conclave doesn't use Grok's search.
- **A Conclave-only ACP client, built on `runtime-core`,** keeps token streaming. It relies on refusing ACP permission requests instead of the one-shot boundary. Pervue chose one-shot mode for its stricter, verifiable boundary.

**Parity checks.** Beyond the HTTP contract, each provider must match Conclave's current behavior. These are the checks for adopting the runtime's adapters:
- **Gemini (the pilot):**
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
  - a turn fails when Antigravity reports a step type it doesn't document.
- **Grok:**
  - the model choices Conclave offers today;
  - OAuth-only sign-in;
  - no tools;
  - the absolute turn limit;
  - cancellation;
  - the same failures in the run inspector.

  It also changes behavior: answers arrive a message at a time.
- **Claude:**
  - refusing sign-ins that aren't subscriptions;
  - the `sonnet`, `opus`, and `haiku` aliases;
  - usage events;
  - no tools;
  - no session persistence;
  - the system prompt;
  - the absolute turn limit;
  - cancellation;
  - the same failures in the run inspector.
- **Codex:** the same list as Claude, with the §7 OpenAI decision applied.

## 8. Phases

**Now.**
1. Patch the security gaps (§3).
2. Record the HTTP contract (§3).

**After runtime Phase 1.** Run the spike and decide go or no-go (§4).

**If go:**
1. Port storage and the HTTP API. Generate the web app's types.
2. Port the orchestrator with its tests.
3. Connect the providers in the runtime's order (Gemini, Grok, Claude, Codex), plus any Conclave-only adapters (§7).
4. Switch over (§5).

**If no-go:**
1. Build the sidecar and its TypeScript client.
2. Replace the adapters one at a time behind the flag (§6).

**Afterwards:** speed work, as in runtime Phase 5.

## 9. Risks

- **The port stalls or drifts.**
  - The contract recording, running both servers side by side, and the TypeScript fallback limit the damage.
  - The sidecar stays available as the no-go path.
- **Stored data compatibility.** Existing files must load unchanged. Test with real data before switching.
- **Feature work during the port.** Either freeze Conclave's features, or port in parallel and re-sync (§10).
- **Contributors need Rust** for everyday server work if the answer is go. The web app stays TypeScript.
- **Stricter provider behavior.**
  - The environment allowlist may drop a variable someone relies on, so each provider can extend it.
  - Private workspaces mean project `CLAUDE.md` files no longer apply.
  - The stricter Gemini and Grok checks can fail turns that pass today when a CLI release changes its output.

## 10. Open decisions

1. After the spike: go with the Rust server, or no-go and take the sidecar?
2. OpenAI: `codex-exec` through the runtime, or a Conclave-only app-server client?
3. Grok: the runtime's one-shot adapter, or a Conclave-only ACP client that keeps streaming?
4. Should Rust become the source of truth for the web app's types (recommended, if go)?
5. Freeze Conclave's features during a port, or port in parallel and re-sync?
6. Caching the sign-in check: keep it off by default (recommended), and what time limit when it's on?
7. Does Conclave accept the runtime's stricter defaults, the environment allowlist and private workspaces, as they are?

[c-index]: https://github.com/davletovb/conclave/blob/1379a37ccb13d484247122ee215257e642b166fc/apps/server/src/index.ts
[c-claude-auth]: https://github.com/davletovb/conclave/blob/1379a37ccb13d484247122ee215257e642b166fc/apps/server/src/providers/anthropic-claude.ts#L235
