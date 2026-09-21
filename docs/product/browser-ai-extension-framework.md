# Browser AI Extension — Product & Engineering Framework

**Status:** Framework v0.1  
**Purpose:** Define the product, architecture, engineering principles, reusable native library boundaries, and acceptance criteria that will later be converted into an implementation plan and tracker.

---

## 1. Product Thesis

Build a lightweight browser AI interface that lets a user ask or search for something immediately from the browser without opening a new tab, while preserving a seamless path into a richer full-page conversation when deeper work is needed.

The product should feel like an **AI command palette for the browser**, not a miniature website inside a popup.

The core interaction is:

> **Invoke → Ask → Read → Continue browsing**
>
> and, when needed:
>
> **Invoke → Ask → Deepen → Continue in full view**

The initial implementation will use a **thin Chrome extension frontend** and a **native C companion/host** that can communicate with subscription-authenticated AI runtimes such as Codex/OpenAI and Claude Code, with additional providers added behind a common interface.

---

## 2. Primary User Value

The extension exists to remove the context-switching cost of AI search and assistance.

A user should be able to:

- ask a quick question without navigating away from the current page;
- search the web and receive a concise cited answer;
- ask about the current page or selected text;
- ask follow-up questions in the popup;
- expand the same conversation into a full-page experience without losing context;
- use already-authenticated provider subscriptions where supported by their local provider tooling;
- avoid dealing with terminals, daemons, ports, tokens, or provider-specific implementation details during normal use.

---

## 3. Product Principles

### 3.1 Instant first, deep second

The popup exists for fast interactions. Long research workflows belong in the full-page experience.

### 3.2 Preserve continuity

Opening the full-page view must continue the same conversation rather than starting over.

### 3.3 Browser-native context

The extension should take advantage of information already available in the browser:

- current page title;
- current URL;
- selected text;
- readable page content when explicitly requested or needed;
- tab metadata;
- browser context-menu actions.

### 3.4 Hide infrastructure

Users should not need to understand:

- Native Messaging;
- local IPC;
- provider CLIs;
- OAuth token storage internals;
- process management;
- provider adapter protocols.

The user-facing model is simply:

> **Install extension → Install companion → Connect provider → Ask**

### 3.5 Provider independence

The UI and conversation model must not be coupled to a single AI provider.

### 3.6 Reusable native core

Low-level functionality should be written so that it can later be extracted into a reusable C library for other products.

### 3.7 Product before abstraction

Do not build a generalized AI framework before the extension needs it. Reusable APIs should emerge from at least two real provider integrations or two real consumers.

### 3.8 Secure by default

The extension must not rely on scraping browser cookies, replaying web-session tokens, or automating consumer chat websites when an authenticated local runtime is available.

---

## 4. Scope

### 4.1 MVP scope

The first usable release should include:

- Chrome Manifest V3 extension;
- popup chat/search interface;
- streaming responses;
- at least one working subscription-backed provider;
- provider status detection;
- current-page context;
- selected-text context;
- conversation persistence;
- follow-up questions;
- full-page continuation view;
- keyboard shortcut;
- right-click/context-menu action;
- light/dark theme;
- native companion installation and health/status check;
- clear failure states when the companion or provider is unavailable.

### 4.2 Near-term scope

After the MVP is stable:

- multiple providers;
- web-search mode with citations;
- provider/model selection;
- recent conversation history;
- configurable page-context permissions;
- richer source cards;
- cancellation and retry;
- provider fallback;
- automatic update mechanism for the native companion;
- macOS and Windows packaging;
- Linux packaging if demand justifies it.

### 4.3 Explicit non-goals for the initial implementation

- building a full autonomous research agent in the popup;
- reproducing every feature of ChatGPT, Claude, or Perplexity;
- a cloud account system;
- team collaboration;
- mobile browser support;
- a generalized plugin marketplace;
- arbitrary browser automation;
- storing provider credentials inside extension local storage;
- using undocumented browser-cookie/session-token interception as the primary provider integration.

---

## 5. User Experience Framework

### 5.1 Primary entry points

The product should support three fast entry paths:

1. **Toolbar icon** — open popup.
2. **Keyboard shortcut** — invoke the popup/command surface quickly.
3. **Context menu** — ask about selected text or the current page.

### 5.2 Popup layout

The popup should remain intentionally small and focused.

Suggested structure:

```text
┌──────────────────────────────────────┐
│ Ask                              ⋮   │
│                                      │
│ ┌──────────────────────────────────┐ │
│ │ Ask anything…                    │ │
│ └──────────────────────────────────┘ │
│                                      │
│ Provider ▾     Page context ▾        │
│                                      │
│ Streaming answer...                  │
│ Sources / citations                   │
│                                      │
│ Ask a follow-up…                     │
│                                      │
│ ↗ Continue in full view              │
└──────────────────────────────────────┘
```

### 5.3 Full-page view

The full-page view is not a different product. It is an expanded form of the same conversation.

It should support:

- full conversation history;
- longer responses;
- richer citations;
- larger page context;
- provider switching when safe;
- deeper search/research modes later;
- conversation management.

### 5.4 Interaction modes

The product should eventually distinguish intent without overloading the interface.

Initial conceptual modes:

- **Ask** — direct model response;
- **Search** — web-grounded response;
- **This Page** — current page is primary context;
- **Selection** — highlighted text is primary context;
- **Research** — deep workflow that should generally expand into full view.

These modes may initially be implicit rather than exposed as five separate buttons.

---

## 6. System Architecture

### 6.1 High-level architecture

```text
Chrome Extension
  ├─ Popup UI
  ├─ Full-page UI
  ├─ Service Worker
  ├─ Content Script
  └─ Context Menu / Commands
          │
          │ Chrome Native Messaging
          ▼
Native C Host / Companion
  ├─ Message framing
  ├─ Request router
  ├─ Provider registry
  ├─ Process manager
  ├─ Stream manager
  ├─ Conversation/session bridge
  ├─ Configuration
  ├─ Logging
  └─ Platform abstraction
          │
          ├─ Codex / OpenAI adapter
          ├─ Claude adapter
          ├─ Gemini adapter
          ├─ Grok adapter
          └─ Future local/API adapters
```

### 6.2 Why Native Messaging

Native Messaging is the preferred local transport for the first architecture because it:

- avoids requiring a permanently listening localhost port;
- lets Chrome invoke the native host when needed;
- provides a natural boundary between browser code and native provider tooling;
- reduces exposure compared with an unauthenticated local HTTP server;
- keeps the provider runtime outside the browser sandbox while maintaining a simple message-based interface.

A localhost transport can remain an optional future adapter if other clients need it.

---

## 7. Chrome Extension Responsibilities

The extension layer should remain thin.

### 7.1 Popup

Responsibilities:

- capture query;
- display streaming answer;
- show compact source/citation information;
- show provider state;
- submit follow-ups;
- expand conversation to full-page view;
- expose simple controls without duplicating the entire settings application.

### 7.2 Service worker

Responsibilities:

- Native Messaging connection lifecycle;
- request routing between UI/content scripts and native host;
- context-menu registration;
- keyboard-command handling;
- tab/page metadata coordination;
- extension-level storage where appropriate;
- reconnection/error recovery.

### 7.3 Content script

Responsibilities:

- capture user selection;
- extract explicitly requested page context;
- retrieve readable page text through a bounded extraction pipeline;
- avoid sending page content unless required by the interaction;
- enforce size limits before passing context to the native layer.

### 7.4 Full-page extension UI

Responsibilities:

- display complete conversation;
- support longer responses and sources;
- expose conversation history;
- provide richer configuration when necessary;
- remain linked to the same native host and conversation identifiers as the popup.

---

## 8. Native C Host Responsibilities

The native host is the systems core of the project.

It should own:

- Native Messaging framing;
- schema validation;
- provider discovery;
- provider capability reporting;
- provider process spawning;
- stdin/stdout/stderr handling;
- streaming output;
- cancellation;
- timeout handling;
- process cleanup;
- provider-specific protocol normalization;
- conversation/session identifiers;
- configuration discovery;
- platform-specific executable lookup;
- structured logging;
- native error normalization.

It should **not** own browser UI logic.

---

## 9. Reusable C Library Strategy

The project should be structured so that reusable native functionality can later be extracted without forcing premature generalization.

Conceptual library layers:

```text
libbrowserai/
  ├─ process/
  ├─ messaging/
  ├─ stream/
  ├─ provider/
  ├─ protocol/
  ├─ platform/
  └─ diagnostics/
```

### 9.1 Process module

Potential API responsibilities:

```c
proc_spawn();
proc_write();
proc_read_stream();
proc_cancel();
proc_wait();
proc_destroy();
```

Requirements:

- stdout and stderr separation;
- asynchronous/non-blocking reads where required;
- cancellation;
- timeout support;
- deterministic cleanup;
- platform abstraction.

### 9.2 Native Messaging module

Potential API responsibilities:

```c
nm_read_message();
nm_write_message();
nm_validate_frame();
```

Requirements:

- bounded message size;
- strict framing validation;
- malformed-input handling;
- no unbounded allocation based solely on untrusted frame length.

### 9.3 Provider module

Conceptual interface:

```c
ai_provider_t *ai_provider_open(const char *id);
ai_request_id_t ai_provider_send(ai_provider_t *, const ai_request_t *);
int ai_provider_cancel(ai_provider_t *, ai_request_id_t);
void ai_provider_close(ai_provider_t *);
```

The interface should normalize:

- provider availability;
- authentication state;
- send/request semantics;
- streaming chunks;
- completion;
- errors;
- cancellation;
- optional conversation continuation.

### 9.4 Stream module

Responsibilities:

- incremental buffering;
- bounded memory use;
- UTF-8 boundary handling;
- chunk callbacks;
- finalization/error state.

### 9.5 Platform module

Responsibilities:

- executable discovery;
- configuration paths;
- Native Messaging host registration;
- OS-specific process primitives;
- environment detection;
- future installer hooks.

### 9.6 Extraction rule

A component should be promoted into the reusable library only when one of these is true:

1. at least two provider adapters need it; or
2. at least two applications need it; or
3. it represents a clearly isolated transport/process primitive with stable semantics.

---

## 10. Provider Adapter Framework

Each provider adapter should expose capabilities rather than forcing all providers into identical behavior.

Example capability model:

```text
ProviderCapabilities
  streaming: yes/no
  continuation: yes/no
  web_search: yes/no
  page_context: yes/no
  attachments: yes/no
  model_selection: yes/no
  cancellation: yes/no
```

The extension UI should respond to capability information rather than hard-code provider-specific assumptions.

### 10.1 Initial provider order

Recommended order:

1. **OpenAI/Codex** — first end-to-end adapter.
2. **Claude** — second adapter, used to validate the reusable provider abstraction.
3. **Gemini** — later.
4. **Grok** — later.

The first reusable provider interface should not be declared stable until at least OpenAI/Codex and Claude both work.

---

## 11. Conversation Model

The extension should use a provider-neutral conversation model.

Conceptual structure:

```text
Conversation
  id
  created_at
  updated_at
  title
  provider_id
  provider_session_id?
  messages[]
  sources[]
  page_context_metadata?
```

Each message should support:

```text
Message
  id
  role
  text
  timestamp
  status
  provider_metadata?
  sources[]?
```

Provider-specific continuation IDs should be treated as implementation metadata, not primary user-facing identifiers.

---

## 12. Search and Retrieval Framework

Search should remain separate from model execution.

Conceptually:

```text
User query
    ↓
Intent / mode decision
    ↓
Need web search?
  ├─ No → provider
  └─ Yes
       ↓
     search adapter
       ↓
     result normalization
       ↓
     provider synthesis
       ↓
     cited response
```

This separation allows:

- provider-independent search;
- multiple search engines later;
- local/private providers to still use web search;
- consistent citations regardless of model.

Search is not required to block the first provider integration if it slows MVP delivery.

---

## 13. Data & Storage

Use the minimum necessary storage at each layer.

### 13.1 Extension storage

Appropriate for:

- UI preferences;
- selected default provider;
- theme;
- lightweight recent conversation index;
- permission preferences.

### 13.2 Native storage

Appropriate for:

- provider discovery cache;
- native diagnostics;
- optional conversation store;
- provider session metadata;
- installation metadata.

SQLite is preferred once structured persistent native storage is necessary. Do not add it before there is a real persistence requirement.

### 13.3 Credential rule

Do not duplicate provider credentials into our own storage when provider tooling can own authentication securely.

---

## 14. Security Framework

Security must be a first-class design constraint because the native host crosses the browser/native boundary.

### 14.1 Trust boundaries

```text
Web page
   ↓ untrusted
Content script
   ↓
Extension service worker
   ↓ validated protocol
Native host
   ↓
Provider process
```

Treat all of the following as untrusted input:

- page text;
- selected text;
- URLs;
- titles;
- Native Messaging payloads;
- provider stdout/stderr;
- search results.

### 14.2 Required controls

- strict JSON schema validation;
- maximum request sizes;
- maximum page-context size;
- maximum Native Messaging frame size;
- no shell-string command construction;
- spawn provider executables using argument arrays;
- deterministic process cleanup;
- no arbitrary executable path from web-page input;
- no secrets in logs;
- no provider credentials stored in extension storage;
- origin/extension identity restrictions in Native Messaging registration;
- explicit permission model for page-content access;
- safe truncation and escaping for logs/UI.

### 14.3 C-specific engineering requirements

Development builds should use:

- AddressSanitizer where supported;
- UndefinedBehaviorSanitizer where supported;
- compiler warnings treated as errors for project code;
- static analysis;
- fuzzing for message/framing parsers when practical;
- centralized ownership and cleanup conventions;
- bounded buffers or explicit dynamic-length checks;
- no unsafe string APIs where safer alternatives exist.

---

## 15. Reliability Framework

The extension should degrade gracefully.

Expected failure states:

- companion not installed;
- Native Messaging host not registered;
- provider executable missing;
- provider not authenticated;
- provider process crashes;
- malformed provider output;
- timeout;
- user cancellation;
- browser closes popup;
- content script cannot access page;
- page content exceeds limit;
- provider unavailable or rate-limited.

Every failure should map to a stable application-level error code plus a human-readable message.

Example categories:

```text
HOST_NOT_INSTALLED
HOST_UNAVAILABLE
PROVIDER_NOT_FOUND
PROVIDER_NOT_AUTHENTICATED
PROVIDER_FAILED
REQUEST_CANCELLED
REQUEST_TIMEOUT
CONTEXT_UNAVAILABLE
INVALID_REQUEST
INTERNAL_ERROR
```

---

## 16. Performance Framework

The main performance objective is perceived responsiveness, not micro-benchmark superiority.

Priority order:

1. popup opens immediately;
2. input is usable immediately;
3. native-host connection is established quickly;
4. provider process is reused where safe and useful;
5. first response chunk is displayed as early as possible;
6. page extraction is bounded and lazy;
7. long operations are cancellable;
8. UI never blocks on native work.

C should be used to keep the native layer lean, but the product should not optimize insignificant sub-millisecond differences while provider/network latency dominates.

---

## 17. Installation & Distribution Framework

### 17.1 Development setup

During development it is acceptable to require:

- loading the unpacked Chrome extension;
- compiling/installing the C native host;
- manually registering the Native Messaging manifest;
- installing provider tools;
- authenticating providers through their supported flows.

### 17.2 Early-user setup

Target:

1. Install Chrome extension.
2. Install desktop companion package.
3. Connect provider accounts.
4. Use extension.

### 17.3 Mature setup

Target experience:

- extension detects whether companion is installed;
- one-click path to companion installer;
- companion registers native host automatically;
- companion detects supported provider tools;
- user gets guided provider connection flows;
- updates happen without terminal commands;
- normal usage never exposes a terminal window.

### 17.4 Packaging targets

Initial priority:

1. macOS;
2. Windows;
3. Linux.

Architecture must avoid unnecessary assumptions that make later Windows support difficult.

---

## 18. Testing Framework

Testing should be layered.

### 18.1 C unit tests

Cover:

- message framing;
- JSON validation;
- process lifecycle;
- stream buffering;
- cancellation;
- timeouts;
- error mapping;
- provider parsing.

### 18.2 Native integration tests

Use fake provider executables that intentionally:

- stream normally;
- stream slowly;
- produce malformed JSON/text;
- write heavily to stderr;
- exit non-zero;
- hang;
- ignore cancellation;
- produce large output.

### 18.3 Extension tests

Cover:

- popup lifecycle;
- service-worker reconnection;
- Native Messaging request routing;
- page-context extraction;
- full-page continuation;
- cancellation;
- settings persistence;
- provider availability UI.

### 18.4 End-to-end tests

Critical user journeys:

- install/health check;
- ask simple question;
- stream response;
- ask follow-up;
- continue in full page;
- ask about selected text;
- ask about current page;
- provider missing;
- provider unauthenticated;
- native host crashes/restarts;
- user cancels response.

---

## 19. Observability & Diagnostics

The native host should produce structured local diagnostics without leaking user content unnecessarily.

Useful fields:

- timestamp;
- request ID;
- conversation ID;
- provider ID;
- lifecycle event;
- duration;
- exit code;
- normalized error code.

Avoid logging:

- full prompts by default;
- full page content;
- credentials/tokens;
- provider authentication files;
- sensitive browser content.

The companion/settings UI should eventually expose a simple diagnostics view and a way to export sanitized logs.

---

## 20. Accessibility & UI Quality

The popup should support:

- keyboard-first operation;
- visible focus states;
- readable contrast;
- semantic controls;
- scalable text;
- screen-reader-friendly status changes where practical;
- reduced-motion preferences;
- light/dark themes.

The command surface should remain usable without a mouse.

---

## 21. Project Structure

Recommended repository shape:

```text
browser-ai/
├── extension/
│   ├── src/
│   │   ├── popup/
│   │   ├── fullpage/
│   │   ├── background/
│   │   ├── content/
│   │   ├── protocol/
│   │   └── shared/
│   ├── public/
│   ├── tests/
│   └── manifest.json
│
├── native/
│   ├── host/
│   ├── lib/
│   │   ├── process/
│   │   ├── messaging/
│   │   ├── stream/
│   │   ├── provider/
│   │   ├── protocol/
│   │   ├── platform/
│   │   └── diagnostics/
│   ├── providers/
│   │   ├── codex/
│   │   └── claude/
│   └── tests/
│
├── packaging/
│   ├── macos/
│   ├── windows/
│   └── linux/
│
├── docs/
│   ├── product/
│   ├── architecture/
│   ├── protocol/
│   └── security/
│
└── tools/
```

The exact folder structure may evolve, but browser code, reusable native code, provider-specific code, and packaging should remain clearly separated.

---

## 22. Protocol Framework

The extension/native protocol should be versioned from the beginning.

Example request envelope:

```json
{
  "version": 1,
  "type": "request",
  "request_id": "req_123",
  "method": "conversation.send",
  "payload": {}
}
```

Example event envelope:

```json
{
  "version": 1,
  "type": "event",
  "request_id": "req_123",
  "event": "response.delta",
  "payload": {
    "text": "partial response"
  }
}
```

Expected event families:

```text
host.ready
provider.status
conversation.created
response.started
response.delta
response.source
response.completed
response.failed
request.cancelled
```

Protocol compatibility must be testable independently of provider behavior.

---

## 23. Decision Rules

When implementation tradeoffs arise, use these rules in order:

1. **Does it improve the core invoke → ask → read workflow?**
2. **Does it preserve fast popup interaction?**
3. **Does it reduce user setup complexity?**
4. **Does it maintain provider independence?**
5. **Does it preserve security boundaries?**
6. **Can the native functionality become reusable without premature abstraction?**
7. **Can it be tested deterministically?**
8. **Does it keep future macOS/Windows portability viable?**

If a feature fails most of these checks, it should probably not be in the current phase.

---

## 24. Definition of Product Milestones

These milestones define capability states, not yet the implementation sequence.

### Milestone A — Native round trip

A Chrome extension can send a validated request through Native Messaging to the C host and receive a streamed fake response.

### Milestone B — First provider

The native host can invoke one real authenticated provider and stream a response into the popup.

### Milestone C — Conversation continuity

Follow-ups work, conversation identity persists, and the same conversation opens in the full-page UI.

### Milestone D — Browser context

Selected text and current-page context can be passed safely and intentionally into a request.

### Milestone E — Second provider

A second provider works through the same normalized provider interface, proving the abstraction is real.

### Milestone F — Reusable native core

Shared process, messaging, stream, and provider primitives are extracted into a reusable C library with documented API boundaries.

### Milestone G — Installable product

A non-developer can install the extension and native companion, connect a provider, and use the product without terminal interaction.

### Milestone H — Search/citations

Web search and source normalization produce grounded cited answers independent of the selected model provider.

---

## 25. Framework for the Future Implementation Tracker

The implementation tracker derived from this document should use work items that are:

- independently testable;
- small enough to review;
- tied to a milestone;
- assigned a stable ID;
- explicit about dependencies;
- explicit about acceptance criteria;
- explicit about whether work is extension, native, provider, packaging, security, or testing.

Suggested ID families:

```text
EXT-xx   Browser extension
NAT-xx   Native C host
LIB-xx   Reusable C library
PRO-xx   Provider adapters
CTX-xx   Page/selection context
CON-xx   Conversation/session model
SRCH-xx  Search and citations
SEC-xx   Security hardening
TST-xx   Testing and QA
PKG-xx   Packaging/installers
OBS-xx   Diagnostics/observability
DOC-xx   Documentation
```

Suggested tracker statuses:

```text
BACKLOG
READY
IN PROGRESS
BLOCKED
IMPLEMENTED — VERIFY
VERIFIED
DEFERRED
```

Every tracker item should include:

```text
ID
Title
Milestone
Area
Goal
Dependencies
Implementation notes
Acceptance criteria
Tests required
Status
```

The tracker should not treat documentation-only completion as implementation completion unless the item itself is documentation work.

---

## 26. Success Criteria

The project should be considered successful when a user can:

1. install the extension and companion;
2. connect at least one supported subscription-backed provider;
3. invoke the popup quickly;
4. ask a question and see a streaming response;
5. ask a follow-up;
6. ask about selected text or the current page;
7. continue the same conversation in a full-page view;
8. recover cleanly from provider or host failure;
9. use the product without understanding its native architecture.

Engineering success additionally requires:

- no dependence on browser-cookie/session scraping;
- deterministic tests for the native protocol and process layer;
- validated browser/native messages;
- clear provider abstraction proven by at least two providers;
- reusable C primitives extracted only where justified by real reuse;
- a path to packaged macOS and Windows installation.

---

## 27. Guiding Statement

The project should optimize for **low-friction access to AI from the browser**, with the popup serving as the fastest interaction surface and the native C layer serving as a secure, reusable bridge to authenticated AI runtimes.

The product is not merely a Perplexity-style popup. Its longer-term shape is a **provider-independent browser AI command layer**, backed by a compact native runtime that can also serve future applications.
