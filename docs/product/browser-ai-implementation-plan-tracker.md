# Browser AI Extension — Implementation Plan & Tracker

**Status:** Active implementation tracker  
**Derived from:** `browser-ai-extension-framework.md` (Framework v0.2)  
**Tracker version:** v0.2  
**Scope rule:** This tracker converts the framework into implementation sequence. It intentionally does not add cloud accounts, browser-cookie scraping, autonomous browser automation, or other capabilities outside the framework.

---

## 1. Delivery Strategy

Build the product as a sequence of capability gates. Each milestone must leave behind a runnable, testable vertical slice.

The critical path is:

```text
Foundation
  ↓
A — Native round trip
  ↓
B — First provider (Codex/OpenAI)
  ↓
C — Conversation continuity
  ↓
D — Browser context
  ↓
MVP closure
  ↓
E — Second provider (Claude)
  ↓
F — Reusable native core
  ↓
G — Installable product
  ↓
H — Search + citations
```

Milestones E–H are ordered to protect the framework's "product before abstraction" rule:

- do not stabilize the reusable provider interface before two real providers work;
- do not extract a generalized native library before real reuse exists;
- do not let web search delay the first useful provider-backed browser experience;
- do not treat packaging as finished until a non-developer can install and use the product without a terminal.

### Recommended PR sizing

Prefer one tracker item per PR. Combine items only when they cannot be tested independently or the split would create unusable intermediate code.

### Status lifecycle

```text
BACKLOG
  ↓
READY
  ↓
IN PROGRESS
  ↓
IMPLEMENTED — VERIFY
  ↓
VERIFIED
```

Use `BLOCKED` only when an external dependency prevents progress. Use `DEFERRED` only for an explicit product decision, not as a substitute for unfinished work.

---

## 2. Release Cut Lines

### Developer vertical slice

Reached after **Milestone B**:

- popup can ask a real provider-backed question;
- response streams from the native host;
- provider/host failures are normalized;
- cancellation and process cleanup work.

This is useful for engineering validation, but is not yet the MVP described by the framework.

### MVP feature-complete cut

Reached after **Milestone D + MVP closure items**:

- popup ask/follow-up flow;
- one real subscription-backed provider;
- provider status detection;
- conversation persistence;
- full-page continuation;
- selected-text and current-page context;
- keyboard shortcut;
- context-menu actions;
- light/dark theme;
- companion/host health detection;
- clear failure states.

Development installation may still require manual native-host registration.

### Early-user release cut

Reached after **Milestone G**:

- extension install;
- companion install;
- native-host registration;
- provider detection/connection guidance;
- no terminal required during normal setup or usage;
- macOS supported first, Windows next.

### Search release cut

Reached after **Milestone H**:

- provider-independent web search;
- normalized sources;
- cited answers;
- consistent source rendering in popup and full-page view.

---

## 3. Milestone Exit Gates

| Milestone | Exit gate |
|---|---|
| Foundation | Repository builds, CI runs, protocol/error decisions are documented, and extension/native skeletons launch. |
| A — Native round trip | Extension sends a validated Native Messaging request and receives streamed fake-provider events. |
| B — First provider | Codex/OpenAI can be discovered, status-checked, invoked, streamed, cancelled, timed out, and cleaned up. |
| C — Conversation continuity | Follow-ups persist and the exact same conversation opens in full-page view. |
| D — Browser context | Selection/current-page context is intentionally captured, bounded, and attached to requests. |
| MVP closure | All MVP entry points, theme, health/failure UX, accessibility baseline, and end-to-end regression suite are green. |
| E — Second provider | Claude works through the same normalized adapter contract and exposes capabilities without UI hard-coding. |
| F — Reusable native core | Reused process/messaging/stream/provider primitives are extracted behind documented library APIs. |
| G — Installable product | Clean-machine macOS installation works without terminal; Windows path is implemented and verified next. |
| H — Search/citations | Search is provider-independent and returns normalized, grounded, cited responses. |

---

## 4. Master Tracker

| ID | Title | Milestone | Area | Dependencies | Status |
|---|---|---|---|---|---|
| DOC-01 | Freeze protocol v1 envelope and event contract | Foundation | Documentation / Protocol | — | IMPLEMENTED — VERIFY |
| DOC-02 | Freeze normalized error taxonomy and capability vocabulary | Foundation | Documentation / Protocol | DOC-01 | IMPLEMENTED — VERIFY |
| EXT-01 | Scaffold Manifest V3 extension surfaces | Foundation | Extension | — | IMPLEMENTED — VERIFY |
| NAT-01 | Scaffold native host and build system | Foundation | Native | — | IMPLEMENTED — VERIFY |
| TST-01 | Establish CI/build/test baseline | Foundation | Testing | EXT-01, NAT-01 | IMPLEMENTED — VERIFY |
| NAT-02 | Implement bounded Native Messaging frame reader/writer | A | Native | NAT-01, DOC-01 | IMPLEMENTED — VERIFY |
| NAT-03 | Implement JSON validation and request router | A | Native | NAT-02, DOC-01, DOC-02 | IMPLEMENTED — VERIFY |
| TST-02 | Build deterministic fake streaming provider | A | Testing | NAT-01 | IMPLEMENTED — VERIFY |
| EXT-02 | Implement service-worker Native Messaging connection manager | A | Extension | EXT-01, DOC-01 | IMPLEMENTED — VERIFY |
| EXT-03 | Implement minimal popup ask/stream UI | A | Extension | EXT-01, EXT-02 | IMPLEMENTED — VERIFY |
| SEC-01 | Enforce browser/native trust-boundary limits | A | Security | NAT-02, NAT-03 | IMPLEMENTED — VERIFY |
| OBS-01 | Add structured native lifecycle diagnostics | A | Observability | NAT-03, DOC-02 | IMPLEMENTED — VERIFY |
| TST-03 | Add extension ↔ host streamed round-trip integration test | A | Testing | TST-02, EXT-02, EXT-03, NAT-03 | IMPLEMENTED — VERIFY |
| NAT-04 | Implement provider process manager | B | Native | NAT-03 | IMPLEMENTED — VERIFY |
| NAT-05 | Implement native stream manager | B | Native | NAT-04 | IMPLEMENTED — VERIFY |
| PRO-01 | Implement provisional provider adapter contract | B | Provider | DOC-02, NAT-04, NAT-05 | IMPLEMENTED — VERIFY |
| PRO-02 | Implement Codex/OpenAI discovery and authentication status | B | Provider | PRO-01 | IMPLEMENTED — VERIFY |
| PRO-03 | Implement Codex/OpenAI request + streaming adapter | B | Provider | PRO-02 | IMPLEMENTED — VERIFY |
| PRO-04 | Implement provider cancellation, timeout, and crash mapping | B | Provider | PRO-03, NAT-04, NAT-05 | IMPLEMENTED — VERIFY |
| EXT-04 | Surface host/provider state and normalized failures | B | Extension | EXT-03, PRO-02, DOC-02 | BACKLOG |
| SEC-02 | Harden provider process invocation and log redaction | B | Security | NAT-04, PRO-03, OBS-01 | BACKLOG |
| TST-04 | Add hostile fake-process integration matrix | B | Testing | NAT-04, NAT-05, PRO-04 | BACKLOG |
| TST-05 | Add opt-in real Codex/OpenAI smoke test | B | Testing | PRO-03, PRO-04 | BACKLOG |
| CON-01 | Define provider-neutral conversation/message/source model | C | Conversation | DOC-01, PRO-03 | BACKLOG |
| CON-02 | Implement conversation persistence and recent index | C | Conversation | CON-01 | BACKLOG |
| CON-03 | Implement native provider-session bridge | C | Conversation | CON-01, PRO-03 | BACKLOG |
| EXT-05 | Implement popup follow-up flow | C | Extension | CON-02, CON-03 | BACKLOG |
| EXT-06 | Implement full-page conversation UI | C | Extension | CON-01, CON-02 | BACKLOG |
| EXT-07 | Implement popup → full-page continuation handoff | C | Extension | EXT-05, EXT-06 | BACKLOG |
| TST-06 | Add conversation continuity end-to-end tests | C | Testing | EXT-07, CON-03 | BACKLOG |
| CTX-01 | Capture selected text safely | D | Context | EXT-01 | IMPLEMENTED — VERIFY |
| CTX-02 | Capture current-tab title/URL metadata | D | Context | EXT-01 | IMPLEMENTED — VERIFY |
| CTX-03 | Implement bounded readable-page extraction | D | Context | CTX-02 | IMPLEMENTED — VERIFY |
| CTX-04 | Implement explicit page-context permission/intent policy | D | Context / Security | CTX-01, CTX-03 | IMPLEMENTED — VERIFY |
| EXT-08 | Add context mode/control to popup request flow | D | Extension | CTX-01, CTX-03, CTX-04, EXT-05 | BACKLOG |
| EXT-09 | Add selection/current-page context-menu actions | D | Extension | CTX-01, CTX-03 | BACKLOG |
| EXT-10 | Add keyboard command entry path | D | Extension | EXT-03 | BACKLOG |
| SEC-03 | Add context size limits, minimization, and safe UI/log handling | D | Security | CTX-03, CTX-04 | BACKLOG |
| TST-07 | Add browser-context extraction and permission tests | D | Testing | CTX-01, CTX-03, CTX-04 | BACKLOG |
| EXT-11 | Add light/dark/system theme support | MVP closure | Extension | EXT-03, EXT-06 | BACKLOG |
| EXT-12 | Add response cancellation and retry UX | MVP closure | Extension | PRO-04, EXT-05 | BACKLOG |
| EXT-13 | Add companion health/install state UX | MVP closure | Extension | EXT-04 | BACKLOG |
| OBS-02 | Add sanitized diagnostics summary | MVP closure | Observability | OBS-01, EXT-13 | BACKLOG |
| EXT-14 | Complete keyboard/accessibility baseline | MVP closure | Extension | EXT-03, EXT-06, EXT-11 | BACKLOG |
| TST-08 | Add MVP critical-journey E2E suite | MVP closure | Testing | TST-06, TST-07, EXT-09, EXT-10, EXT-13 | BACKLOG |
| TST-09 | Add startup/first-chunk performance budgets | MVP closure | Testing / Performance | TST-08 | BACKLOG |
| PRO-05 | Implement Claude discovery and authentication status | E | Provider | PRO-01, PRO-02 | BACKLOG |
| PRO-06 | Implement Claude request + streaming adapter | E | Provider | PRO-05, NAT-04, NAT-05 | BACKLOG |
| PRO-07 | Reconcile provider contract from Codex + Claude evidence | E | Provider | PRO-03, PRO-06 | BACKLOG |
| EXT-15 | Add capability-aware provider selector | E | Extension | PRO-07, EXT-04 | BACKLOG |
| TST-10 | Add cross-provider contract test suite | E | Testing | PRO-07 | BACKLOG |
| LIB-01 | Extract reusable process primitives | F | Native Library | PRO-07, TST-10 | BACKLOG |
| LIB-02 | Extract reusable Native Messaging primitives | F | Native Library | NAT-02, TST-03 | BACKLOG |
| LIB-03 | Extract reusable stream primitives | F | Native Library | NAT-05, TST-10 | BACKLOG |
| LIB-04 | Extract reusable provider/protocol primitives | F | Native Library | PRO-07, LIB-01, LIB-03 | BACKLOG |
| LIB-05 | Extract reusable platform/diagnostics primitives where justified | F | Native Library | OBS-01, PRO-05 | BACKLOG |
| DOC-03 | Document reusable library ownership/API boundaries | F | Documentation | LIB-01, LIB-02, LIB-03, LIB-04 | BACKLOG |
| TST-11 | Add standalone native-library unit/ABI tests | F | Testing | LIB-01, LIB-02, LIB-03, LIB-04 | BACKLOG |
| PKG-01 | Build macOS companion package and host registration | G | Packaging | TST-08, EXT-13, LIB-02, LIB-05 | BACKLOG |
| PKG-02 | Add macOS provider discovery/setup guidance | G | Packaging | PKG-01, PRO-07 | BACKLOG |
| PKG-03 | Add extension/host protocol compatibility check | G | Packaging / Protocol | DOC-01, PKG-01 | BACKLOG |
| PKG-04 | Add signing/notarization-ready macOS release pipeline | G | Packaging | PKG-01 | BACKLOG |
| TST-12 | Verify macOS clean-machine install/use/uninstall journey | G | Testing | PKG-01, PKG-02, PKG-03 | BACKLOG |
| PKG-05 | Implement Windows native host registration/build | G | Packaging | LIB-02, LIB-05 | BACKLOG |
| PKG-06 | Build Windows companion installer | G | Packaging | PKG-05, PRO-07 | BACKLOG |
| TST-13 | Verify Windows clean-machine install/use/uninstall journey | G | Testing | PKG-06, PKG-03 | BACKLOG |
| SEC-04 | Security review of packaged trust boundaries and permissions | G | Security | PKG-04, PKG-06 | BACKLOG |
| SRCH-01 | Define provider-independent search adapter contract | H | Search | CON-01, PRO-07 | BACKLOG |
| SRCH-02 | Implement first search backend adapter | H | Search | SRCH-01 | BACKLOG |
| SRCH-03 | Normalize search results into source model | H | Search | SRCH-02, CON-01 | BACKLOG |
| SRCH-04 | Implement search → synthesis pipeline | H | Search | SRCH-03, PRO-07 | BACKLOG |
| EXT-16 | Add Search mode and compact citations to popup | H | Extension | SRCH-04, EXT-08 | BACKLOG |
| EXT-17 | Add rich sources/citations to full-page view | H | Extension | SRCH-04, EXT-06 | BACKLOG |
| SEC-05 | Sanitize/limit untrusted search-result content | H | Security | SRCH-02, SRCH-03 | BACKLOG |
| TST-14 | Add citation/source grounding regression suite | H | Testing | SRCH-04, EXT-16, EXT-17 | BACKLOG |
| OBS-03 | Add sanitized diagnostics export | Post-G | Observability | OBS-02, PKG-01 | BACKLOG |
| PRO-08 | Add Gemini adapter | Post-E | Provider | PRO-07 | BACKLOG |
| PRO-09 | Add Grok adapter | Post-E | Provider | PRO-07 | BACKLOG |
| CON-04 | Add SQLite native persistence only when justified | Post-MVP | Conversation | CON-02 | DEFERRED |
| PKG-07 | Add automatic companion updater | Post-G | Packaging | PKG-03, PKG-04, PKG-06 | BACKLOG |
| PKG-08 | Add Linux packaging if demand justifies it | Post-G | Packaging | LIB-05 | DEFERRED |

---

# 5. Detailed Work Items

## Foundation

### DOC-01 — Freeze protocol v1 envelope and event contract
**Milestone:** Foundation  
**Area:** Documentation / Protocol  
**Goal:** Turn the framework's example protocol into the minimum normative v1 contract used by extension and native code.  
**Dependencies:** None

**Implementation notes**
- Define request/event envelopes and required fields.
- Define request ID semantics and uniqueness expectations.
- Define event ordering for:
  - `host.ready`
  - `provider.status`
  - `conversation.created`
  - `response.started`
  - `response.delta`
  - `response.source`
  - `response.completed`
  - `response.failed`
  - `request.cancelled`
- Define unknown-version and unknown-method behavior.
- Keep payloads extensible without making all fields optional.

**Acceptance criteria**
- Extension and native host can be implemented without guessing envelope semantics.
- Protocol version mismatch has a deterministic response.
- Event ordering is explicit enough to test independently of a real provider.

**Tests required**
- Protocol fixture validation tests.
- Golden request/event examples.

**Status:** IMPLEMENTED — VERIFY

**Implementation evidence**
- Normative contract: `docs/protocol/v1.md`
- Request envelope schema: `docs/protocol/schemas/request-envelope.schema.json`
- Event envelope schema: `docs/protocol/schemas/event-envelope.schema.json`
- Golden protocol fixtures: `docs/protocol/fixtures/v1-golden.json`
- Automated fixture validation is intentionally wired by TST-01 when the CI/test baseline exists.

### DOC-02 — Freeze normalized error taxonomy and capability vocabulary
**Milestone:** Foundation  
**Area:** Documentation / Protocol  
**Goal:** Convert framework error/capability concepts into stable application-level names.  
**Dependencies:** DOC-01

**Implementation notes**
- Start with framework errors:
  - `HOST_NOT_INSTALLED`
  - `HOST_UNAVAILABLE`
  - `PROVIDER_NOT_FOUND`
  - `PROVIDER_NOT_AUTHENTICATED`
  - `PROVIDER_FAILED`
  - `REQUEST_CANCELLED`
  - `REQUEST_TIMEOUT`
  - `CONTEXT_UNAVAILABLE`
  - `INVALID_REQUEST`
  - `INTERNAL_ERROR`
- Define capability fields for streaming, continuation, web search, page context, attachments, model selection, and cancellation.
- Provider-specific raw errors remain metadata, never primary UI logic.

**Acceptance criteria**
- Every known host/provider failure maps to one normalized category.
- Capability fields have explicit true/false/unknown semantics.

**Tests required**
- Error mapping fixture tests.
- Capability serialization tests.

**Status:** IMPLEMENTED — VERIFY

**Implementation evidence**
- Normative taxonomy: `docs/protocol/errors-and-capabilities-v1.md`
- Error schema: `docs/protocol/schemas/error.schema.json`
- Provider-status schema: `docs/protocol/schemas/provider-status.schema.json`
- Golden mapping/status fixtures: `docs/protocol/fixtures/v1-errors-capabilities.json`
- Automated schema/fixture validation is wired by TST-01 with the CI/test baseline.

### EXT-01 — Scaffold Manifest V3 extension surfaces
**Milestone:** Foundation  
**Area:** Extension  
**Goal:** Create the extension skeleton with popup, full-page, service worker, content script, command, and context-menu entry points.  
**Dependencies:** None

**Acceptance criteria**
- Extension loads unpacked without warnings caused by project code.
- Popup opens.
- Full-page route opens.
- Service worker starts.
- Content script can be injected only on allowed pages.
- Placeholder command/context-menu handlers fire.

**Tests required**
- Manifest validation.
- Minimal extension smoke test.

**Status:** IMPLEMENTED — VERIFY

**Implementation evidence**
- MV3 manifest: `extension/manifest.json`
- Popup surface: `extension/src/popup/`
- Full-page surface: `extension/src/fullpage/`
- Service worker: `extension/src/background/service-worker.js`
- Content-script scaffold: `extension/src/content/content-script.js`
- Manifest/service-worker smoke test: `extension/tests/manifest-smoke.mjs`
- Development loading instructions: `extension/README.md`

### NAT-01 — Scaffold native host and build system
**Milestone:** Foundation  
**Area:** Native  
**Goal:** Establish a portable native project that can read stdin/write stdout and compile with strict diagnostics.  
**Dependencies:** None

**Implementation notes**
- Treat compiler and Clippy warnings as errors for project code in CI.
- Forbid `unsafe` code in project crates.
- Create module boundaries aligned with the framework, without prematurely publishing a reusable library API.

**Acceptance criteria**
- Native host builds locally and in CI.
- Project crates forbid `unsafe` code.
- Host starts and exits cleanly.

**Tests required**
- Build matrix smoke test.
- Host startup test.

**Status:** IMPLEMENTED — VERIFY

**Implementation evidence**
- Cargo workspace (Rust 1.85+, `unsafe_code = "forbid"`): `native/Cargo.toml` + `native/host/Cargo.toml`
- Host entry point: `native/host/src/main.rs`
- Foundation host core: `native/host/src/host.rs`
- Startup, stream-lifecycle, and exit-status tests: `native/host/src/host.rs` + `native/host/tests/cli.rs`
- Accepts Chrome's launch shapes, including the Windows `--parent-window=<handle>` argument after the origin: `native/host/src/main.rs` + `native/host/tests/cli.rs`
- Local validation: `cargo fmt --check`, `cargo clippy -- -D warnings` (Linux, plus Windows and macOS target checks), and `cargo test --workspace` on stable and Rust 1.85 all pass.
- Ported from C to Rust (ADR-0001). Before the C sources were removed, the Rust host matched the C host byte-for-byte, including exit statuses, on about 2.3 million differential inputs. Moves to VERIFIED once the port merges.

### TST-01 — Establish CI/build/test baseline
**Milestone:** Foundation  
**Area:** Testing  
**Goal:** Make every later tracker item land behind automatic checks.  
**Dependencies:** EXT-01, NAT-01

**Acceptance criteria**
- Extension lint/type/build checks run automatically.
- Native build/unit tests run automatically.
- Native parsers are fuzzed in CI under AddressSanitizer.
- Project fails CI on compiler or Clippy warnings in project code.

**Status:** IMPLEMENTED — VERIFY

**Implementation evidence**
- Repository CI workflow: `.github/workflows/ci.yml`
- Extension lint/type/build/smoke commands: `extension/package.json`
- Extension lint config: `extension/eslint.config.js`
- Extension JS type checking: `extension/jsconfig.json` + `extension/types/chrome.d.ts`
- Deterministic unpacked build: `extension/scripts/build.mjs`
- Protocol schema/fixture validation: `scripts/validate_protocol.py`
- Native CI runs `cargo fmt`, `cargo clippy`, and `cargo test --workspace` on Linux, macOS, and Windows with stable Rust, plus a Rust 1.85 minimum-version job and a cargo-fuzz smoke job.
- Warnings fail CI through `RUSTFLAGS=-D warnings` and `cargo clippy -- -D warnings`.
- Moves to VERIFIED once the Rust port merges.

---

## Milestone A — Native Round Trip

### NAT-02 — Implement bounded Native Messaging frame reader/writer
**Area:** Native  
**Dependencies:** NAT-01, DOC-01

**Implementation notes**
- Implement Chrome Native Messaging length-prefix framing.
- Reject oversized, truncated, impossible, or malformed frames before allocation/use.
- Do not allocate directly from untrusted length without a configured upper bound.
- Keep read/write logic independent of provider behavior.

**Acceptance criteria**
- Valid frames round-trip exactly.
- Oversized/truncated frames fail cleanly.
- EOF and partial reads are handled deterministically.
- No unbounded memory growth from frame length.

**Tests required**
- Unit tests for normal/empty/max/oversized/truncated frames.
- Fuzz target for frame parser when practical.

**Status:** IMPLEMENTED — VERIFY

**Implementation evidence**
- Bounded Native Messaging reader/writer: `native/host/src/framing.rs`
- Host framing integration and deterministic exit statuses: `native/host/src/host.rs`
- Unit coverage for normal, empty, maximum-size, oversized, truncated, EOF, I/O-error, interrupted-read, literal native-byte-order, and forced short-read/short-write cases: `native/host/src/framing.rs`
- Host-level oversized/truncated framing checks: `native/host/src/host.rs` + `native/host/tests/cli.rs`
- cargo-fuzz harness and structured corpus generator: `native/fuzz/fuzz_targets/frame_reader.rs` + `native/fuzz/create_corpus.py`
- Project frame cap: 1 MiB, enforced before payload allocation and before writes.
- Ported from C to Rust (ADR-0001). Before the C sources were removed, the Rust host matched the C host byte-for-byte, including exit statuses, on about 2.3 million differential inputs. Moves to VERIFIED once the port merges.

### NAT-03 — Implement JSON validation and request router
**Area:** Native  
**Dependencies:** NAT-02, DOC-01, DOC-02

**Acceptance criteria**
- Unknown versions/methods produce normalized errors.
- Missing required fields are rejected.
- Request IDs are preserved in every emitted response/event.
- Router can dispatch a fake conversation request.

**Tests required**
- Malformed JSON fixtures.
- Missing/extra/unknown field cases.
- Unsupported version/method cases.

**Status:** IMPLEMENTED — VERIFY

**Implementation evidence**
- Strict bounded JSON syntax reader: `native/host/src/protocol/json.rs`
- Protocol request model and strict top-level/method-payload validation: `native/host/src/protocol/request.rs`
- Duplicate member names are rejected in every object of a method payload, compared after decoding escapes (v1 §1 rule 9): `native/host/src/protocol/request.rs` + `native/host/src/protocol/json.rs`
- Method router: `native/host/src/protocol/router.rs`
- Protocol event/error emission with the typed DOC-02 error and capability vocabulary: `native/host/src/protocol/events.rs`
- Host emits exactly one `host.ready`, keeps running after malformed requests, and routes the local fake conversation provider: `native/host/src/host.rs`
- Parser/router/integration tests cover malformed JSON, duplicate/missing/extra/wrong fields, 128/129-character request-ID boundaries, escaped identifiers, unsupported versions, unknown methods, depth limits, recovered-ID malformed failures, invalid method payloads, and fake conversation dispatch: `native/host/src/protocol/` + `native/host/src/host.rs`
- Protocol fuzz target runs each input through the whole host and asserts every emitted frame is a JSON object; golden-derived corpus: `native/fuzz/fuzz_targets/protocol.rs` + `native/fuzz/create_protocol_corpus.py`
- Built-host contract harness validates emitted frames against the frozen event/error/provider-status schemas and golden event sequences: `scripts/validate_host_protocol.py`
- Ported from C to Rust (ADR-0001). Before the C sources were removed, the Rust host matched the C host byte-for-byte, including exit statuses, on about 2.3 million differential inputs. Moves to VERIFIED once the port merges.

### TST-02 — Build deterministic fake streaming provider
**Area:** Testing  
**Dependencies:** NAT-01

**Goal:** Provide a provider-independent test executable/harness for streaming behavior before a real provider exists.

**Acceptance criteria**
- Fake provider can:
  - stream normally;
  - stream slowly;
  - write to stderr;
  - exit non-zero;
  - hang;
  - ignore cancellation;
  - emit malformed output;
  - emit large output.

**Status:** IMPLEMENTED — VERIFY

**Implementation evidence**
- Standalone fake provider executable: `native/test_provider/src/main.rs`
- Test-only mode contract/documentation: `native/test_provider/README.md`
- Cross-platform behavior tests: `native/test_provider/tests/modes.rs`
- Tests verify normal streaming, deliberately slow streaming with deterministic timeout/flush probes, stderr output, exit 42 with empty stdout, bounded timeout for hanging behavior, cancellation-ignore behavior, malformed output, exactly 2 MiB of large stdout, and invalid CLI shapes.
- Rust's standard streams pass bytes through unchanged on Windows pipes, so fixture bytes are deterministic across platforms without a binary-mode switch.
- POSIX signal tests prove `hang` terminates on SIGTERM while `ignore-cancel` survives SIGTERM until SIGKILL cleanup: `native/test_provider/tests/signals.rs`
- The fake provider is a separate workspace crate outside the default build, so `cargo build` produces only the host while `cargo test --workspace` builds and tests the fixture.
- Ported from C to Rust (ADR-0001). Before they were removed, the original CMake mode harness and C signal test both passed against the Rust binary. Moves to VERIFIED once the port merges.

### EXT-02 — Implement service-worker Native Messaging connection manager
**Area:** Extension  
**Dependencies:** EXT-01, DOC-01

**Acceptance criteria**
- Opens native port on demand.
- Routes events to the correct requester by request ID.
- Detects disconnect.
- Can reconnect after host termination.
- Does not require the popup to remain open to maintain extension state where MV3 permits.

**Tests required**
- Mock-port lifecycle tests.
- Disconnect/reconnect tests.
- Request multiplexing tests.

**Status:** IMPLEMENTED — VERIFY

**Implementation evidence**
- Lazy Native Messaging port manager: `extension/src/background/native-connection.js`
- Service-worker-owned singleton, independent of popup lifetime: `extension/src/background/service-worker.js`
- Required MV3 permission: `extension/manifest.json`
- Mock-port lifecycle, multiplexing, disconnect/reconnect, stale-port, manual-disconnect, post-failure, connect-failure, `request.cancelled`, request-ID grammar, request-ID-first serialization, throwing-callback isolation, and terminal-route ordering tests: `extension/tests/native-connection-manager.mjs`
- Service-worker smoke test proves import/health handling does not eagerly call `connectNative`: `extension/tests/manifest-smoke.mjs`
- Canonical host name centralized as `com.pervue.host`; later packaging/registration must use the same identifier.
- Moves to VERIFIED once merged.

### EXT-03 — Implement minimal popup ask/stream UI
**Area:** Extension  
**Dependencies:** EXT-01, EXT-02

**Acceptance criteria**
- Input is immediately usable.
- Submit sends one request.
- Streaming deltas render incrementally.
- Completion/failure states render without page reload.
- UI does not block while native work is active.

**Tests required**
- Popup lifecycle tests.
- Incremental rendering test.
- Duplicate-submit guard.

**Status:** IMPLEMENTED — VERIFY

**Implementation evidence**
- Popup ask form with streaming render, completion/failure states, and duplicate-submit guard: `extension/src/popup/ask-form.js`, `extension/src/popup/index.html`
- Popup ↔ service-worker contract, one runtime port per question: `extension/src/shared/ask-port.js`
- Service-worker bridge sending one `conversation.send` per question, forwarding its events in order to one terminal event, reporting a missing or unregistered host as `HOST_NOT_INSTALLED` and a host that can't start or disconnects as `HOST_UNAVAILABLE`, and serving only the extension's own pages: `extension/src/background/ask-bridge.js`, `extension/src/background/service-worker.js`
- Popup lifecycle, incremental rendering, duplicate-submit, text-only rendering, failure, and lost-worker tests: `extension/tests/popup-ask.mjs`
- Bridge tests for request shape, event order, concurrent pages, native disconnect and its host-error classification, connect and post failures, closed pages, empty questions, and sender checks: `extension/tests/ask-bridge.mjs`
- Service-worker wiring smoke test: `extension/tests/manifest-smoke.mjs`
- Milestone A asks the host's deterministic `fake` provider; provider selection arrives with Milestone B.
- Development host registration for trying the popup: `extension/README.md`
- Moves to VERIFIED once merged.

### SEC-01 — Enforce browser/native trust-boundary limits
**Area:** Security  
**Dependencies:** NAT-02, NAT-03

**Acceptance criteria**
- Native host manifest restricts allowed extension origin/identity.
- Request/frame size limits are centralized and tested.
- Untrusted values are never used as executable paths or shell command strings.
- Invalid payloads fail before provider/process work begins.

**Status:** IMPLEMENTED — VERIFY

**Implementation evidence**
- The host generates its Native Messaging manifest for exact extension IDs only (32 characters `a`–`p`; wildcards, patterns, and full origins are refused) and starts only with an exact `chrome-extension://<id>/` caller origin, never without one: `native/host/src/manifest.rs` + `native/host/src/main.rs`, tested in `native/host/src/manifest.rs` + `native/host/tests/cli.rs`
- Frame, nesting, and request-ID limits defined once per side (`native/host/src/limits.rs`, `extension/src/shared/limits.js`) and recorded in `docs/protocol/native-messaging-v1.json`, which contract tests on both sides and the fuzz-corpus generator read
- The extension refuses a request over the 1 MiB frame limit, or one JSON can't represent, before the native port opens or is used, so one bad request can't end the host for every request in flight. The popup reports `INVALID_REQUEST` / `REQUEST_TOO_LARGE` and pre-checks the question: `extension/src/background/native-connection.js`, `extension/src/background/ask-bridge.js`, `extension/src/popup/ask-form.js`, with manager, bridge, and popup tests
- Provider IDs are compared exactly with the registry; path- and shell-like IDs are unknown providers (`native/host/src/host.rs` tests), and `native/host/clippy.toml` makes any `std::process::Command::new` in the host a CI error until NAT-04's process manager allows it in one place
- A test feeds every validation failure kind through the host loop and shows none reaches a handler: `invalid_requests_never_reach_a_handler` in `native/host/src/host.rs`
- Trust-boundary summary: `docs/security/trust-boundaries.md`
- Checked in Chromium: the generated manifest admits the unpacked extension and one generated for another ID locks it out; a 2 MiB question is refused while the host keeps running, where on `main` it ended the host
- Moves to VERIFIED once merged.

### OBS-01 — Add structured native lifecycle diagnostics
**Area:** Observability  
**Dependencies:** NAT-03, DOC-02

**Acceptance criteria**
- Logs can include timestamp, request ID, conversation ID, provider ID, lifecycle event, duration, exit code, normalized error.
- Full prompts, page content, credentials, and provider auth files are excluded by default.
- stderr diagnostics do not corrupt stdout Native Messaging frames.

**Status:** IMPLEMENTED — VERIFY

**Implementation evidence**
- JSON-lines diagnostics with timestamp, lifecycle event, request ID, method, provider ID, conversation ID, duration, exit code, and normalized error (code and reason): `native/host/src/diagnostics.rs`
- The host records `host.started`, `request.completed`, `request.failed`, `request.aborted`, `request.rejected`, and `host.stopped`, with one `request.*` record for every request it reads. The request loop records each request's outcome, and any conversation it created: `native/host/src/host.rs` (the router it used at first became the provider adapter contract in PRO-01)
- Records never copy prompt text, page context, other payload members, raw frames, or error messages. Identifiers are cut to 128 characters and JSON-escaped: `diagnostics_never_copy_request_content` and the `diagnostics.rs` unit tests
- Diagnostics go only to stderr, and a failing stderr is ignored: `diagnostics_go_to_stderr_and_never_into_the_frames` and `the_logged_exit_code_matches_the_process` in `native/host/tests/cli.rs`, and `diagnostics_do_not_change_the_frames` in `native/host/src/host.rs`
- The protocol fuzz target checks that every input yields three well-formed records (`native/fuzz/fuzz_targets/protocol.rs`), and `scripts/validate_host_protocol.py` checks the built host's stderr on the golden requests
- Format and fields documented in `native/README.md` (Diagnostics)
- Moves to VERIFIED once merged.

### TST-03 — Add extension ↔ host streamed round-trip integration test
**Area:** Testing  
**Dependencies:** TST-02, EXT-02, EXT-03, NAT-03

**Exit proof for Milestone A**
- A test request travels from extension → host → fake provider and streamed deltas return to the popup.
- The test verifies request IDs, ordering, completion, and one failure path.

**Notes**
- Also pin SEC-01's size boundary end to end with the built host: a request whose frame is exactly 1 MiB reaches the host and is answered, and one byte more is refused by the extension without reaching it (suggested in the SEC-01 review).

**Status:** IMPLEMENTED — VERIFY

**Implementation evidence**
- `extension/tests/native-roundtrip.mjs` launches the built host with a Chrome origin and sends real Native Messaging frames through the extension connection manager, ask bridge, and popup renderer.
- The test checks the host's deterministic fake-provider event ordering, request ID correlation, visible incremental rendering, a host-generated provider failure, and recovery.
- It sends one serialized request at exactly 1 MiB, rejects one byte over before the host sees it, and confirms the same host handles a subsequent question.
- `npm --prefix extension run test:roundtrip` runs in the dedicated `native-roundtrip` CI job after building the host. The in-memory runtime/DOM ports stand in for Chrome; the native host binary and protocol are real.

---

## Milestone B — First Provider

### NAT-04 — Implement provider process manager
**Area:** Native  
**Dependencies:** NAT-03

**Implementation notes**
- Spawn executable with argument arrays, not shell strings.
- Separate stdin/stdout/stderr.
- Support write, wait, cancel, timeout, kill escalation, and deterministic cleanup.
- Design for macOS first without embedding assumptions that block Windows.

**Acceptance criteria**
- Child process lifecycle is deterministic across success, crash, timeout, and cancellation.
- No zombie/orphan process remains after tests.

**Tests required**
- Process lifecycle tests using fake executables.
- Repeated spawn/cancel stress test.

**Status:** IMPLEMENTED — VERIFY

**Implementation evidence**
- `native/host/src/process.rs`: `Process::spawn` starts an absolute executable path with an argument array, with no shell and no `PATH` search, on three separate pipes. `write` queues input for a helper thread, `next_event(deadline)` returns bounded stdout/stderr chunks and then the exit, `terminate(grace)` escalates from a stop request to a kill, `kill` kills at once, and dropping a `Process` kills and reaps it. It is the host's only process spawn (`native/host/clippy.toml`), and the Codex adapter (PRO-03) is its first caller, through the stream manager (NAT-05), which added `request_stop()`: a stop request that doesn't wait.
- POSIX: the provider leads its own process group. A stop reaches everything it started, and whatever it leaves in the group when it exits is killed. Windows: only the provider itself is stopped so far, and closing stdin is the only stop request; stopping the whole tree needs a Job Object (ADR-0001).
- Lifecycle tests with the fake provider (`native/test_provider/tests/process_manager.rs`): success, nonzero exit, crash (SIGABRT), a deadline passing, a graceful stop, an ignored stop escalated to SIGKILL, kill, a drop that reaps, 1 MiB of input written while its echo is read, 2 MiB of output in chunks of at most 8 KiB, process-group leadership, and descendants that stay in the group, outlive the provider, or leave the group.
- Stress test (`native/test_provider/tests/process_stress.rs`): 120 spawn/stop rounds across modes, start points, and ways of stopping. Each process is reaped (no zombie), and no pipe or thread is left behind.
- New fake provider modes: `crash`, `echo`, `tree`, `orphan`, `escape`, `detached` (`native/test_provider/README.md`).
- Documented in `native/README.md` (Provider processes) and `docs/security/trust-boundaries.md` §3.
- Moves to VERIFIED once merged.

### NAT-05 — Implement native stream manager
**Area:** Native  
**Dependencies:** NAT-04

**Acceptance criteria**
- Handles incremental buffering.
- Preserves UTF-8 boundaries.
- Applies bounded memory policy.
- Separates chunk/final/error states.
- Can stop promptly on cancellation.

**Tests required**
- Split UTF-8 boundary fixtures.
- Large-output/backpressure tests.
- Cancellation mid-chunk.

**Status:** IMPLEMENTED — VERIFY

**Implementation evidence**
- `native/host/src/stream.rs`: `LineStream` delivers a provider's stdout as complete lines, then exactly one terminal state: `Final` after a clean end, `Error` when a line passes the limit or isn't UTF-8 (the process is killed), or `Stopped` after `cancel`. stderr is counted and discarded. `split_text` cuts outgoing text between characters.
- Incremental buffering and UTF-8 boundaries: lines are reassembled across chunks cut anywhere, even inside a character, and checked as UTF-8 only once complete. The `stream.rs` unit tests cut `é✓😀` at every byte, and `characters_split_between_chunks_are_reassembled` streams 2,000 lines of multi-byte characters through a real process (`native/test_provider/tests/stream_manager.rs`).
- Bounded memory: the stream holds at most one line, up to its limit, plus the lines of one chunk, and the process manager reads at most 16 chunks of 8 KiB ahead, so a flood waits on the provider's own writes. A line past the limit ends the stream (`a_line_past_the_limit_ends_the_stream_without_growing_memory`), and a 2 MiB line reaches a slow reader whole (`a_long_line_within_the_limit_arrives_whole_even_to_a_slow_reader`).
- Prompt cancellation: `cancel(grace)` drops undelivered output at once, sends SIGTERM (`Process::request_stop`), and kills the process after the grace period. Tests cancel mid-line, using the new `partial` fake provider mode, between lines, and against a provider that ignores SIGTERM.
- The host splits long answers into `response.delta` events of at most 64 KiB, so each fits in a frame (`a_long_delta_is_split_into_frames_that_fit` in `native/host/src/host.rs`).
- Documented in `native/README.md` (Stream manager).
- Moves to VERIFIED once merged.

### PRO-01 — Implement provisional provider adapter contract
**Area:** Provider  
**Dependencies:** DOC-02, NAT-04, NAT-05

**Goal:** Create the smallest provider interface needed by the first real adapter. It is explicitly provisional until Claude is implemented.

**Acceptance criteria**
- Adapter can report availability/authentication/capabilities.
- Adapter can start a request, stream events, cancel, and close.
- Provider-specific protocol details do not leak into popup code.

**Status:** IMPLEMENTED — VERIFY

**Implementation evidence**
- `native/host/src/providers/mod.rs`: a `Provider` reports its status (availability, authentication, capabilities) and starts each request as an `Exchange`, a state machine the host loop drives with deadlines. `cancel(grace)` stops an exchange, and dropping one closes it and kills its processes. Updates are in protocol terms (`ConversationCreated`, `Started`, `Delta`, `Status`, `Activity`, then `Completed`, `Failed`, or `Stopped`), so command lines, output formats, and provider session IDs stay inside the adapter.
- The host serves requests side by side through the contract (`native/host/src/host.rs`). `request.cancel` stops its target, which ends with `REQUEST_CANCELLED` before `request.cancelled` confirms each cancellation (protocol v1 §7.6). Request IDs are unique among requests in flight (`DUPLICATE_REQUEST_ID`), start and idle timeouts end a request with `REQUEST_TIMEOUT`, and closing the input stops every running request (`INPUT_CLOSED`).
- Tests in `host.rs` drive a scripted provider: requests side by side, a cancel before the response starts, nothing after a cancel, repeated cancellations, unknown and finished targets, duplicate and reused IDs, both timeouts, the end of input, an adapter that stops unasked or never stops, and requests aborted when stdout closes.
- The deterministic `fake` scaffold is an adapter too (`native/host/src/providers/fake.rs`), so the golden fixtures and host conformance run through the contract. In the extension, only the default provider ID changed (`extension/src/background/ask-bridge.js`): popup code still sees protocol events alone.
- Documented in `native/README.md` (Requests in flight, Adapter contract). Provisional until Claude works (PRO-07).
- Moves to VERIFIED once merged.

### PRO-02 — Implement Codex/OpenAI discovery and authentication status
**Area:** Provider  
**Dependencies:** PRO-01

**Acceptance criteria**
- Supported executable/runtime is discovered using platform-controlled lookup rules.
- Missing executable maps to `PROVIDER_NOT_FOUND`.
- Unauthenticated state maps to `PROVIDER_NOT_AUTHENTICATED`.
- No credential material is copied into extension storage.

**Tests required**
- Missing executable.
- Present but unauthenticated.
- Authenticated status fixture.

**Status:** IMPLEMENTED — VERIFY

**Implementation evidence**
- Platform-controlled lookup (`native/host/src/providers/discovery.rs`): the host's `PATH`, then the usual install locations Chrome's minimal `PATH` can miss (Homebrew, `/usr/local/bin`, user bin directories, npm, Volta, Bun, and nvm's Node versions, newest first; `%APPDATA%\npm` on Windows), or `PERVUE_PROVIDER_PATH` instead. Relative directories are skipped, and nothing in a request affects the lookup.
- `provider.status` runs `codex login status` and reads only its exit status: 0 is `authenticated`, 1 `unauthenticated`, and anything else, or no answer within 10 seconds, `unknown`. A missing executable is `not_found` (`native/host/src/providers/codex/mod.rs`).
- A request finds a missing executable as `PROVIDER_NOT_FOUND` / `EXECUTABLE_NOT_FOUND` and a signed-out Codex as `PROVIDER_NOT_AUTHENTICATED` / `LOGIN_REQUIRED`, before `codex exec` runs, because a signed-out `codex exec` retries instead of failing.
- No credential material leaves Codex: the sign-in check's output, which names a masked key, is never read, and the extension stores nothing.
- Tests against a fake `codex` (`native/test_provider/tests/codex_adapter.rs`): missing (`status_reports_a_missing_codex_as_not_found`, `a_missing_codex_fails_the_request_as_not_found`), present but unauthenticated, and authenticated (`status_reports_the_sign_in_from_the_exit_status_alone`, `a_signed_out_codex_fails_the_request_before_it_runs`), a check that hangs, and a Codex that can't start (`unavailable`). `the_installed_host_reports_a_missing_codex` in `native/host/tests/cli.rs` runs the built host. Discovery has unit tests for the lookup order, the override, relative directories, non-executable files, and nvm versions.
- Checked with Codex CLI 0.156.1 from npm: `codex login status` exits 0 once signed in with an API key and 1 when signed out, and a real-browser check shows a signed-out Codex as "Codex isn't signed in" and a missing one as "Codex isn't installed" in the popup.
- Documented in `native/README.md` (Codex).
- Moves to VERIFIED once merged.

### PRO-03 — Implement Codex/OpenAI request + streaming adapter
**Area:** Provider  
**Dependencies:** PRO-02

**Acceptance criteria**
- One real prompt streams into popup.
- stdout/stderr are handled separately.
- Provider output is normalized into protocol events.
- Provider-specific session metadata stays behind the adapter boundary.

**Tests required**
- Parser fixtures from representative provider outputs.
- Opt-in live smoke test.

**Status:** IMPLEMENTED — VERIFY

**Implementation evidence**
- `native/host/src/providers/codex/mod.rs` runs `codex exec --json --skip-git-repo-check --sandbox read-only -C <empty dir> [resume <thread id>] -` with the question on stdin, never in an argument. `output.rs` turns Codex's JSON events into protocol events: `turn.started` becomes `conversation.created` and `response.started`, each agent message a `response.delta`, and `turn.completed` `response.completed`.
- stdout and stderr are separate pipes. stdout is read line by line through the stream manager (NAT-05). stderr, which can hold secrets, is counted and discarded, and so are the messages of failed turns.
- Codex thread IDs stay behind the adapter: each conversation gets a random `conv_…` ID mapped to its thread inside the host, and continuing the conversation resumes that thread. A thread ID that could read as an option is refused as malformed output.
- Parser fixtures: `native/host/src/providers/codex/fixtures/*.jsonl`, captured from Codex CLI 0.156.1 (a success, a resumed turn, 401, 429, and 500 failures, and retry notices), parsed by the `output.rs` tests. Adapter tests against the fake `codex` check the exact command line, the question on stdin, streaming, continuing a conversation, and several messages in one answer.
- Opt-in live smoke test: `native/host/tests/live_codex.rs` (`PERVUE_LIVE_CODEX=1`) asks the installed Codex one question through the built host.
- Checked with the real Codex CLI 0.156.1 against a local stand-in for the OpenAI Responses API, as no OpenAI credentials are available here. The live smoke test passes, and a continued conversation carries its history. In Chromium, the unpacked extension's popup asked Codex through the built host and showed the streamed answer, with no Codex or host process left afterwards.
- The extension's default provider is now `codex`.
- Documented in `native/README.md` (Codex).
- Moves to VERIFIED once merged.

### PRO-04 — Implement provider cancellation, timeout, and crash mapping
**Area:** Provider  
**Dependencies:** PRO-03, NAT-04, NAT-05

**Acceptance criteria**
- User cancellation ends native/provider work.
- Timeout produces `REQUEST_TIMEOUT`.
- Non-zero exit/malformed provider output maps to `PROVIDER_FAILED`.
- Cancellation produces `REQUEST_CANCELLED`.
- Cleanup remains deterministic even if child process ignores graceful cancellation.

**Status:** IMPLEMENTED — VERIFY

**Implementation evidence**
- User cancellation: `request.cancel` sends SIGTERM to Codex's process group and SIGKILL after 2 seconds. The request ends with `REQUEST_CANCELLED` / `USER_CANCELLED`, then the cancellation with `request.cancelled` (`a_cancel_request_stops_codex_mid_turn`, `cancelling_mid_turn_stops_codex_promptly`). Cancelling during the sign-in check never starts `codex exec` (`cancelling_during_the_sign_in_check_runs_nothing`).
- Timeouts: `REQUEST_TIMEOUT` / `PROVIDER_START_TIMEOUT` when Codex doesn't start its turn within 60 seconds, and `REQUEST_TIMEOUT` / `PROVIDER_RESPONSE_TIMEOUT` after 5 minutes without progress (`a_codex_that_never_starts_its_turn_times_out`, `a_codex_that_goes_quiet_times_out`).
- Failures map to `PROVIDER_FAILED`: a nonzero exit before the turn ends to `PROCESS_EXITED`, and malformed or oversized output, or a clean exit before the turn ends, to `MALFORMED_PROVIDER_OUTPUT`. Failed turns map to `AUTH_REJECTED` (as `PROVIDER_NOT_AUTHENTICATED`), `PROVIDER_RATE_LIMITED`, or `PROVIDER_UNAVAILABLE`, never with Codex's own message (`failed_turns_map_to_normalized_errors`, `a_lost_codex_session_fails_as_a_process_exit`).
- Deterministic cleanup: a Codex that ignores SIGTERM is killed after the grace period (`a_codex_that_ignores_cancellation_is_killed_after_the_grace_period`), and one that lingers after its turn is stopped with its answer kept. The host ends a request whose adapter never stops one second past the grace period, and closing the input stops every running request (`closing_the_input_stops_a_running_codex`). The tests check that no Codex process is left.
- Checked with the real Codex CLI 0.156.1: cancelling mid-answer ended the request in under 0.1 seconds with no Codex process left. 401, 429, and 500 responses mapped to `AUTH_REJECTED`, `PROVIDER_RATE_LIMITED`, and `PROVIDER_UNAVAILABLE`. A signed-out Codex failed with `LOGIN_REQUIRED` in under 0.1 seconds, and closing the input mid-answer left no Codex process, and the host exited 0.
- Mutation check: 24 deliberate faults across the request loop, stream manager, process manager, discovery, and the Codex adapter, all caught by the tests.
- Moves to VERIFIED once merged.

### EXT-04 — Surface host/provider state and normalized failures
**Area:** Extension  
**Dependencies:** EXT-03, PRO-02, DOC-02

**Acceptance criteria**
- Popup distinguishes:
  - companion/host unavailable;
  - provider missing;
  - provider unauthenticated;
  - provider failure;
  - timeout;
  - cancellation.
- UI instructions describe the user action, not internal architecture.

**Status:** BACKLOG

### SEC-02 — Harden provider process invocation and log redaction
**Area:** Security  
**Dependencies:** NAT-04, PRO-03, OBS-01

**Acceptance criteria**
- No shell command concatenation.
- Web/page content cannot select arbitrary executable paths.
- Environment forwarding is minimal and documented.
- Secrets/tokens are redacted from logs.

**Status:** BACKLOG

### TST-04 — Add hostile fake-process integration matrix
**Area:** Testing  
**Dependencies:** NAT-04, NAT-05, PRO-04

**Acceptance criteria**
- CI covers slow stream, stderr flood, non-zero exit, hang, ignored cancellation, malformed output, and large output.
- All cases finish with bounded resources and normalized outcomes.

**Status:** BACKLOG

### TST-05 — Add opt-in real Codex/OpenAI smoke test
**Area:** Testing  
**Dependencies:** PRO-03, PRO-04

**Acceptance criteria**
- Test is skipped safely when provider tooling/auth is absent.
- When enabled, it proves discovery → send → stream → completion.
- It does not expose credentials in CI logs.

**Notes**
- PRO-03 added `native/host/tests/live_codex.rs`, which runs only with `PERVUE_LIVE_CODEX=1` and proves discovery → send → stream → completion through the built host. It still fails, rather than skips, when enabled without Codex or its sign-in, and no CI job runs it yet.

**Status:** BACKLOG

---

## Milestone C — Conversation Continuity

### CON-01 — Define provider-neutral conversation/message/source model
**Area:** Conversation  
**Dependencies:** DOC-01, PRO-03

**Acceptance criteria**
- Conversation includes framework fields: ID, timestamps, title, provider ID, optional provider session ID, messages, sources, optional page-context metadata.
- Message supports ID, role, text, timestamp, status, optional provider metadata, optional sources.
- Provider session IDs are implementation metadata, not user-facing primary IDs.

**Status:** BACKLOG

### CON-02 — Implement conversation persistence and recent index
**Area:** Conversation  
**Dependencies:** CON-01

**Implementation notes**
- Start with extension storage for conversation metadata/content sufficient for MVP continuity.
- Do not add SQLite yet unless a concrete persistence requirement exceeds extension storage.

**Acceptance criteria**
- Closing/reopening popup preserves recent conversation.
- Recent index can open a stored conversation.
- Schema versioning exists for persisted data.

**Status:** BACKLOG

### CON-03 — Implement native provider-session bridge
**Area:** Conversation  
**Dependencies:** CON-01, PRO-03

**Acceptance criteria**
- Follow-up request can reuse provider continuation/session information when supported.
- Providers without native continuation can still receive normalized conversation context.
- Native session metadata is recoverable enough for the chosen persistence model.

**Status:** BACKLOG

### EXT-05 — Implement popup follow-up flow
**Area:** Extension  
**Dependencies:** CON-02, CON-03

**Acceptance criteria**
- Follow-up reuses the same conversation ID.
- Prior messages render correctly.
- New response streams into the existing thread.

**Status:** BACKLOG

### EXT-06 — Implement full-page conversation UI
**Area:** Extension  
**Dependencies:** CON-01, CON-02

**Acceptance criteria**
- Opens a conversation by stable conversation ID.
- Displays complete history and longer responses.
- Uses the same request protocol/native host as popup.

**Status:** BACKLOG

### EXT-07 — Implement popup → full-page continuation handoff
**Area:** Extension  
**Dependencies:** EXT-05, EXT-06

**Acceptance criteria**
- "Continue in full view" opens the identical conversation.
- No prompt or answer duplication occurs.
- A follow-up from full view continues the same conversation.

**Status:** BACKLOG

### TST-06 — Add conversation continuity end-to-end tests
**Area:** Testing  
**Dependencies:** EXT-07, CON-03

**Exit proof for Milestone C**
- Ask in popup → follow up → open full page → follow up again.
- Conversation ID and message ordering remain stable throughout.

**Status:** BACKLOG

---

## Milestone D — Browser Context

### CTX-01 — Capture selected text safely
**Area:** Context  
**Dependencies:** EXT-01

**Acceptance criteria**
- Selection is captured only after explicit user action.
- Selection length is bounded.
- Empty/unavailable selection has a deterministic outcome.

**Status:** IMPLEMENTED — VERIFY

**Implementation evidence**
- The popup's explicit **Use selection** action requests capture only on click, shows a preview, and attaches it only on a later Ask; **No context** clears it: `extension/src/popup/context-controls.js`, `extension/src/popup/ask-form.js`.
- The service worker accepts capture only from the popup, requests the active tab's top-frame content script, normalizes unavailable pages and empty selections as `CONTEXT_UNAVAILABLE`, and checks the size of the response: `extension/src/background/selection-capture.js`.
- The content script reads regular selections and focused text-field selections only upon the capture message, limits UTF-8 text to 16 KiB without splitting Unicode characters, and reports truncation: `extension/src/content/content-script.js`.
- `extension/tests/selection-capture.mjs` covers explicit gating, bounds, Unicode, input selection, restricted pages, unauthorized callers, and clear-during-capture behavior; `extension/tests/ask-bridge.mjs` covers request attachment.

### CTX-02 — Capture current-tab title/URL metadata
**Area:** Context  
**Dependencies:** EXT-01

**Acceptance criteria**
- Current tab title/URL are available to request construction where permissions allow.
- Unsupported/internal pages fail as `CONTEXT_UNAVAILABLE`, not as generic internal errors.

**Status:** IMPLEMENTED — VERIFY

**Implementation evidence**
- Active-tab metadata is read only after a popup capture action, under the temporary `activeTab` grant; HTTP/HTTPS titles and URLs are bounded, credentials/query/fragment removed, and unsupported or inaccessible tabs return `CONTEXT_UNAVAILABLE`: `extension/src/background/selection-capture.js`, `extension/manifest.json`.
- `extension/tests/selection-capture.mjs` covers missing access, internal pages, sanitized metadata, and bounds.

### CTX-03 — Implement bounded readable-page extraction
**Area:** Context  
**Dependencies:** CTX-02

**Implementation notes**
- Extraction is lazy: do not scrape every page in advance.
- Use a bounded readable-text pipeline rather than raw full DOM serialization.
- Truncation policy must be deterministic and visible in metadata where useful.

**Acceptance criteria**
- Page content is not sent unless the interaction requires it.
- Extracted text respects a configured size ceiling.
- Extraction failure does not break normal Ask mode.

**Status:** IMPLEMENTED — VERIFY

**Implementation evidence**
- The content script lazily walks readable page text, prefers `main`/`article`, skips hidden/navigation/form/editable content, caps output at 64 KiB and traversal at 5,000 text nodes, and reports truncation: `extension/src/content/content-script.js`.
- Empty or inaccessible extraction is normalized as `CONTEXT_UNAVAILABLE`; `extension/tests/selection-capture.mjs` covers page extraction and limits.

### CTX-04 — Implement explicit page-context permission/intent policy
**Area:** Context / Security  
**Dependencies:** CTX-01, CTX-03

**Acceptance criteria**
- User can distinguish no context vs selection vs current page.
- Permission state is explicit and persists only where appropriate.
- Context is never silently attached simply because a content script is present.

**Status:** IMPLEMENTED — VERIFY

**Implementation evidence**
- The popup begins with No context, shows explicit selection/page choices, permission outcome and preview, permits clearing while capture is pending, and keeps context only in popup memory: `extension/src/popup/context-controls.js`.
- The service worker accepts capture requests only from the popup with explicit click intent, uses the active tab's top frame, and never stores access decisions; the ask bridge validates and attaches only chosen structured context: `extension/src/background/selection-capture.js`, `extension/src/background/ask-bridge.js`.
- `extension/tests/selection-capture.mjs`, `extension/tests/popup-ask.mjs`, and `extension/tests/ask-bridge.mjs` cover intent, failure, no-context Ask, and attached context.

### EXT-08 — Add context mode/control to popup request flow
**Area:** Extension  
**Dependencies:** CTX-01, CTX-03, CTX-04, EXT-05

**Acceptance criteria**
- Popup can initiate Ask, Selection, and This Page behavior without cluttering the command surface.
- Attached context is visible enough that the user understands what will be sent.

**Status:** BACKLOG

**Existing foundation**
- CTX-01–04 already provide the popup's No context, Use selection, and Use this page controls, capture preview, and structured context on the first Ask. This item remains for the EXT-05 follow-up flow and its context behavior; reuse those controls rather than rebuilding them.

### EXT-09 — Add selection/current-page context-menu actions
**Area:** Extension  
**Dependencies:** CTX-01, CTX-03

**Acceptance criteria**
- Selected-text action preloads/starts a Selection interaction.
- Current-page action preloads/starts a This Page interaction.
- No accidental duplicate popup/full-page conversations are created.

**Status:** BACKLOG

### EXT-10 — Add keyboard command entry path
**Area:** Extension  
**Dependencies:** EXT-03

**Acceptance criteria**
- Configured browser shortcut opens/focuses the command surface.
- Focus lands in the input.
- Workflow is usable without mouse.

**Status:** BACKLOG

### SEC-03 — Add context size limits, minimization, and safe UI/log handling
**Area:** Security  
**Dependencies:** CTX-03, CTX-04

**Acceptance criteria**
- Page/selection size limits are centralized.
- Full page/selection content is excluded from logs by default.
- UI rendering escapes untrusted page/provider text safely.
- URL/title/text are treated as untrusted data end to end.

**Status:** BACKLOG

### TST-07 — Add browser-context extraction and permission tests
**Area:** Testing  
**Dependencies:** CTX-01, CTX-03, CTX-04

**Exit proof for Milestone D**
- Selected text path works.
- Current-page path works.
- Unsupported page fails cleanly.
- Oversized content truncates safely.
- Ask mode does not send page text when context is off.

**Status:** BACKLOG

---

## MVP Closure

### EXT-11 — Add light/dark/system theme support
**Area:** Extension  
**Dependencies:** EXT-03, EXT-06

**Acceptance criteria**
- System default is supported.
- Explicit light/dark preference persists.
- Popup and full-page remain visually consistent.

**Status:** BACKLOG

### EXT-12 — Add response cancellation and retry UX
**Area:** Extension  
**Dependencies:** PRO-04, EXT-05

**Acceptance criteria**
- Active response exposes cancel.
- Cancel updates UI immediately and native work terminates.
- Retry creates a clear new request without duplicating persisted assistant messages.

**Status:** BACKLOG

### EXT-13 — Add companion health/install state UX
**Area:** Extension  
**Dependencies:** EXT-04

**Acceptance criteria**
- Extension can distinguish "host not installed/registered" from "host present but provider unavailable."
- Development builds can point to setup instructions without pretending packaging already exists.
- Normal errors avoid exposing Native Messaging jargon unless diagnostics are expanded.

**Status:** BACKLOG

### OBS-02 — Add sanitized diagnostics summary
**Area:** Observability  
**Dependencies:** OBS-01, EXT-13

**Acceptance criteria**
- User can inspect host version, protocol version, provider availability, and recent normalized failure.
- No full prompt/page content or secrets are included.

**Status:** BACKLOG

### EXT-14 — Complete keyboard/accessibility baseline
**Area:** Extension  
**Dependencies:** EXT-03, EXT-06, EXT-11

**Acceptance criteria**
- Visible focus states.
- Semantic form/buttons.
- Status updates are screen-reader-friendly where practical.
- Scalable text remains usable.
- Reduced-motion preference is respected where motion exists.

**Status:** BACKLOG

### TST-08 — Add MVP critical-journey E2E suite
**Area:** Testing  
**Dependencies:** TST-06, TST-07, EXT-09, EXT-10, EXT-13

**Acceptance criteria**
Automated or reproducible tests cover:
1. host health check;
2. simple question;
3. streaming response;
4. follow-up;
5. continue in full view;
6. selected text;
7. current page;
8. provider missing;
9. provider unauthenticated;
10. host crash/restart;
11. user cancellation.

**Status:** BACKLOG

### TST-09 — Add startup/first-chunk performance budgets
**Area:** Testing / Performance  
**Dependencies:** TST-08

**Implementation notes**
Measure product-level responsiveness, not microbenchmarks.

**Acceptance criteria**
- Popup open/input-ready timing is measured.
- Native connection timing is measured.
- Time-to-first-response-chunk is measured separately from provider total latency.
- Regressions are visible in CI or a repeatable benchmark report.

**Status:** BACKLOG

---

## Milestone E — Second Provider

### PRO-05 — Implement Claude discovery and authentication status
**Area:** Provider  
**Dependencies:** PRO-01, PRO-02

**Acceptance criteria**
- Claude tooling is discovered using the same platform rules.
- Availability/auth status maps to the same normalized app model.
- No Claude-specific conditions are added to popup code.

**Status:** BACKLOG

### PRO-06 — Implement Claude request + streaming adapter
**Area:** Provider  
**Dependencies:** PRO-05, NAT-04, NAT-05

**Acceptance criteria**
- Real authenticated Claude request streams through the native host.
- Cancellation/errors are normalized.
- Continuation behavior is exposed as a capability rather than assumed.

**Status:** BACKLOG

### PRO-07 — Reconcile provider contract from Codex + Claude evidence
**Area:** Provider  
**Dependencies:** PRO-03, PRO-06

**Goal:** This is the point where the provider interface may become stable.

**Acceptance criteria**
- Shared concepts are in the common interface.
- Provider-specific concepts remain adapter metadata/extensions.
- No interface method exists solely for hypothetical future providers.
- Capability model reflects observed differences between the two real adapters.

**Status:** BACKLOG

### EXT-15 — Add capability-aware provider selector
**Area:** Extension  
**Dependencies:** PRO-07, EXT-04

**Acceptance criteria**
- User can choose available provider.
- Unsupported controls are hidden/disabled based on capability data.
- Provider switching does not silently claim continuation when unsafe.

**Status:** BACKLOG

### TST-10 — Add cross-provider contract test suite
**Area:** Testing  
**Dependencies:** PRO-07

**Exit proof for Milestone E**
- The same provider-neutral test cases run against Codex and Claude adapters.
- Differences are expressed through capabilities, not test exceptions scattered through UI code.

**Status:** BACKLOG

---

## Milestone F — Reusable Native Core

### LIB-01 — Extract reusable process primitives
**Area:** Native Library  
**Dependencies:** PRO-07, TST-10

**Acceptance criteria**
- Process API covers spawn/write/read/cancel/wait/destroy semantics actually used by at least two provider adapters.
- Host-specific policy remains outside the library.
- Existing tests pass through the extracted API.

**Status:** BACKLOG

### LIB-02 — Extract reusable Native Messaging primitives
**Area:** Native Library  
**Dependencies:** NAT-02, TST-03

**Acceptance criteria**
- Frame read/write/validation has a documented bounded API.
- No extension-product UI assumptions exist in the module.

**Status:** BACKLOG

### LIB-03 — Extract reusable stream primitives
**Area:** Native Library  
**Dependencies:** NAT-05, TST-10

**Acceptance criteria**
- Bounded buffering, UTF-8 handling, chunk delivery, final/error states are reusable outside one provider.

**Status:** BACKLOG

### LIB-04 — Extract reusable provider/protocol primitives
**Area:** Native Library  
**Dependencies:** PRO-07, LIB-01, LIB-03

**Acceptance criteria**
- Public API reflects only behaviors proven by Codex + Claude.
- Capability and normalized event/error types are documented.
- Provider-specific session details remain opaque.

**Status:** BACKLOG

### LIB-05 — Extract reusable platform/diagnostics primitives where justified
**Area:** Native Library  
**Dependencies:** OBS-01, PRO-05

**Acceptance criteria**
- Only functionality with demonstrated reuse is extracted.
- Executable discovery/config-path/diagnostic helpers remain separable from provider adapters.
- Windows implementation points are represented without forcing POSIX-only public semantics.

**Status:** BACKLOG

### DOC-03 — Document reusable library ownership/API boundaries
**Area:** Documentation  
**Dependencies:** LIB-01, LIB-02, LIB-03, LIB-04

**Acceptance criteria**
- Ownership/lifetime rules are explicit.
- Error ownership and cleanup conventions are explicit.
- Public vs internal items are separated.
- Extraction rule from the framework is documented beside the library.

**Status:** BACKLOG

### TST-11 — Add standalone native-library unit/ABI tests
**Area:** Testing  
**Dependencies:** LIB-01, LIB-02, LIB-03, LIB-04

**Exit proof for Milestone F**
- Reusable library modules can be tested without launching the extension.
- Library fuzz targets are green.
- API/ABI compatibility policy is documented for packaged host releases.

**Status:** BACKLOG

---

## Milestone G — Installable Product

### PKG-01 — Build macOS companion package and host registration
**Area:** Packaging  
**Dependencies:** TST-08, EXT-13, LIB-02, LIB-05

**Acceptance criteria**
- Installer places native host in a stable location.
- Native Messaging manifest is registered automatically.
- Uninstall removes owned files/registration cleanly.
- Normal user does not edit JSON paths manually.

**Status:** BACKLOG

### PKG-02 — Add macOS provider discovery/setup guidance
**Area:** Packaging  
**Dependencies:** PKG-01, PRO-07

**Acceptance criteria**
- Companion detects supported provider tools.
- Missing/unauthenticated providers produce actionable setup guidance.
- Credentials remain owned by provider tooling.

**Status:** BACKLOG

### PKG-03 — Add extension/host protocol compatibility check
**Area:** Packaging / Protocol  
**Dependencies:** DOC-01, PKG-01

**Acceptance criteria**
- Extension detects incompatible host protocol versions before starting a request.
- User receives an update/setup action instead of undefined behavior.

**Status:** BACKLOG

### PKG-04 — Add signing/notarization-ready macOS release pipeline
**Area:** Packaging  
**Dependencies:** PKG-01

**Acceptance criteria**
- Release artifacts are reproducible enough for signing/notarization.
- Build provenance/version is embedded.
- No secrets are bundled in release artifacts.

**Status:** BACKLOG

### TST-12 — Verify macOS clean-machine install/use/uninstall journey
**Area:** Testing  
**Dependencies:** PKG-01, PKG-02, PKG-03

**Acceptance criteria**
A non-development macOS environment can:
1. install extension;
2. install companion;
3. detect/connect a provider;
4. ask and stream;
5. restart browser and continue normal operation;
6. uninstall cleanly;
without terminal commands during the user journey.

**Status:** BACKLOG

### PKG-05 — Implement Windows native host registration/build
**Area:** Packaging  
**Dependencies:** LIB-02, LIB-05

**Acceptance criteria**
- Windows process/path/registration implementation follows the same host protocol.
- No POSIX-only assumption leaks into the public reusable interfaces.

**Status:** BACKLOG

### PKG-06 — Build Windows companion installer
**Area:** Packaging  
**Dependencies:** PKG-05, PRO-07

**Acceptance criteria**
- Installer registers the host automatically.
- Provider discovery works for supported Windows provider tooling.
- Uninstall is clean.

**Status:** BACKLOG

### TST-13 — Verify Windows clean-machine install/use/uninstall journey
**Area:** Testing  
**Dependencies:** PKG-06, PKG-03

**Acceptance criteria**
- Same user-level journey as TST-12 passes on supported Windows.
- No terminal is required for normal setup/use.

**Status:** BACKLOG

### SEC-04 — Security review of packaged trust boundaries and permissions
**Area:** Security  
**Dependencies:** PKG-04, PKG-06

**Acceptance criteria**
- Native host allowed-origins/identity config is verified in packaged artifacts.
- Installer cannot be influenced by webpage-supplied paths/arguments.
- Logs and update/install metadata contain no provider credentials.
- Extension permissions are reviewed against minimum required capability.

**Status:** BACKLOG

---

## Milestone H — Search & Citations

### SRCH-01 — Define provider-independent search adapter contract
**Area:** Search  
**Dependencies:** CON-01, PRO-07

**Acceptance criteria**
- Search request/result interface is independent of model provider.
- Normalized result includes enough data for title, URL, snippet/content excerpt, and source identity.
- Search errors do not masquerade as provider errors.

**Status:** BACKLOG

### SRCH-02 — Implement first search backend adapter
**Area:** Search  
**Dependencies:** SRCH-01

**Implementation notes**
- Choose one supported search mechanism during implementation.
- Keep search execution separate from model execution.
- Do not couple the adapter to Codex/Claude.

**Acceptance criteria**
- Query returns normalized raw search results.
- Timeout/error behavior is bounded and normalized.

**Status:** BACKLOG

### SRCH-03 — Normalize search results into source model
**Area:** Search  
**Dependencies:** SRCH-02, CON-01

**Acceptance criteria**
- Search results map into the conversation/source model used by popup/full-page UI.
- Duplicate sources can be collapsed deterministically.
- Source URL/title remain untrusted data and are safely rendered.

**Status:** BACKLOG

### SRCH-04 — Implement search → synthesis pipeline
**Area:** Search  
**Dependencies:** SRCH-03, PRO-07

**Acceptance criteria**
- Search results are passed to selected provider for synthesis.
- Response can emit source references independently of provider-specific citation formats.
- Search can work with either supported provider.

**Status:** BACKLOG

### EXT-16 — Add Search mode and compact citations to popup
**Area:** Extension  
**Dependencies:** SRCH-04, EXT-08

**Acceptance criteria**
- User can explicitly invoke search behavior.
- Streaming answer shows compact source references without turning popup into a full research UI.
- Long/deep workflow can hand off to full-page view.

**Status:** BACKLOG

### EXT-17 — Add rich sources/citations to full-page view
**Area:** Extension  
**Dependencies:** SRCH-04, EXT-06

**Acceptance criteria**
- Full-page view can show richer source metadata.
- Source identity remains consistent with popup.
- Clicking/opening source uses safe URL handling.

**Status:** BACKLOG

### SEC-05 — Sanitize/limit untrusted search-result content
**Area:** Security  
**Dependencies:** SRCH-02, SRCH-03

**Acceptance criteria**
- Search snippets/content obey size limits.
- HTML/script content is never trusted as UI.
- Search result text cannot inject executable instructions into local process invocation.

**Status:** BACKLOG

### TST-14 — Add citation/source grounding regression suite
**Area:** Testing  
**Dependencies:** SRCH-04, EXT-16, EXT-17

**Exit proof for Milestone H**
- Search results normalize consistently.
- Sources attached to answer exist in retrieved source set.
- Popup/full-page render the same source identities.
- Search failure degrades to a clear error without corrupting the conversation.

**Status:** BACKLOG

---

## 6. Post-Milestone Backlog

### OBS-03 — Add sanitized diagnostics export
**Dependencies:** OBS-02, PKG-01  
**Acceptance criteria:** User can export support diagnostics without prompts, full page text, credentials, or auth files.  
**Status:** BACKLOG

### PRO-08 — Add Gemini adapter
**Dependencies:** PRO-07  
**Rule:** Must conform to the proven provider abstraction; do not expand the interface unless Gemini exposes a real missing capability.  
**Status:** BACKLOG

### PRO-09 — Add Grok adapter
**Dependencies:** PRO-07  
**Rule:** Same abstraction discipline as PRO-08.  
**Status:** BACKLOG

### CON-04 — Add SQLite native persistence only when justified
**Dependencies:** CON-02  
**Trigger condition:** Extension storage or provider-session durability becomes demonstrably insufficient.  
**Status:** DEFERRED

### PKG-07 — Add automatic companion updater
**Dependencies:** PKG-03, PKG-04, PKG-06  
**Status:** BACKLOG

### PKG-08 — Add Linux packaging if demand justifies it
**Dependencies:** LIB-05  
**Status:** DEFERRED

---

## 7. Cross-Cutting Definition of Done

An implementation item is not `VERIFIED` unless all applicable conditions are true:

- code is merged;
- acceptance criteria are demonstrated;
- required tests exist and pass;
- error paths are covered, not only happy path;
- no provider credentials are copied into extension storage;
- no shell-string command construction is introduced;
- page/provider/search text is treated as untrusted input;
- new protocol behavior has fixtures or contract tests;
- new user-facing failure modes map to normalized errors;
- native code has deterministic ownership/cleanup;
- native fuzz smoke runs remain green;
- documentation is updated when a public protocol/API/installer behavior changes.

Documentation-only completion does not count as implementation completion unless the tracker item is explicitly documentation work.

---

## 8. Security Checklist Applied to Every Relevant PR

- [ ] Native Messaging/request size is bounded.
- [ ] Page/selection/search/provider output is treated as untrusted.
- [ ] No arbitrary executable path originates from webpage input.
- [ ] Child processes are spawned with argument arrays, not shell command strings.
- [ ] No provider credentials/tokens are persisted in extension storage.
- [ ] Logs exclude full prompt/page content by default.
- [ ] Native host registration restricts the extension identity/origin.
- [ ] UI escapes untrusted text/URLs safely.
- [ ] Cancellation/timeout leaves no leaked native process/resource.
- [ ] New native code adds no `unsafe` (any exception documents its safety argument and has targeted tests).

---

## 9. Verification Matrix

| Capability | Primary proof |
|---|---|
| Native protocol | TST-03 |
| Process reliability | TST-04 |
| First real provider | TST-05 |
| Conversation continuity | TST-06 |
| Browser context | TST-07 |
| Complete MVP journey | TST-08 |
| Performance regression visibility | TST-09 |
| Provider independence | TST-10 |
| Reusable native core | TST-11 |
| macOS non-developer install | TST-12 |
| Windows non-developer install | TST-13 |
| Search/citation grounding | TST-14 |

---

## 10. Recommended First Execution Queue

Start with this exact order unless a blocker is discovered:

```text
DOC-01
DOC-02
EXT-01
NAT-01
TST-01
NAT-02
NAT-03
TST-02
EXT-02
EXT-03
SEC-01
OBS-01
TST-03
NAT-04
NAT-05
PRO-01
PRO-02
PRO-03
PRO-04
EXT-04
SEC-02
TST-04
TST-05
```

After **TST-05** is green, move directly into Milestone C rather than adding more providers or generalized library abstractions.

---

## 11. Progress Dashboard

Update this section whenever item statuses change.

| Stage | Total | Verified | Implemented — Verify | In Progress | Ready | Backlog | Blocked | Deferred |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| Foundation | 5 | 0 | 5 | 0 | 0 | 0 | 0 | 0 |
| A — Native round trip | 8 | 0 | 8 | 0 | 0 | 0 | 0 | 0 |
| B — First provider | 10 | 0 | 6 | 0 | 0 | 4 | 0 | 0 |
| C — Conversation continuity | 7 | 0 | 0 | 0 | 0 | 7 | 0 | 0 |
| D — Browser context | 9 | 0 | 4 | 0 | 0 | 5 | 0 | 0 |
| MVP closure | 7 | 0 | 0 | 0 | 0 | 7 | 0 | 0 |
| E — Second provider | 5 | 0 | 0 | 0 | 0 | 5 | 0 | 0 |
| F — Reusable native core | 7 | 0 | 0 | 0 | 0 | 7 | 0 | 0 |
| G — Installable product | 9 | 0 | 0 | 0 | 0 | 9 | 0 | 0 |
| H — Search/citations | 8 | 0 | 0 | 0 | 0 | 8 | 0 | 0 |
| Post-milestone | 6 | 0 | 0 | 0 | 0 | 4 | 0 | 2 |
| **Total** | **81** | **0** | **23** | **0** | **0** | **56** | **0** | **2** |

### Milestone completion rule

A milestone is complete only when its exit-proof test is `VERIFIED` and every security/reliability dependency required by that proof is also `VERIFIED`.

### Progress reporting rule

When reporting project completion, report both:

1. **item completion:** `VERIFIED / total non-deferred items`; and
2. **capability completion:** which milestone exit gates are fully satisfied.

Do not count `IMPLEMENTED — VERIFY` as verified completion.
