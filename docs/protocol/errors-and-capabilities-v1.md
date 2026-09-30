# TabBeam Protocol v1 — Errors and Provider Capabilities

**Status:** Frozen for implementation (DOC-02)  
**Protocol:** TabBeam Native Protocol v1  
**Scope:** Normalized application error taxonomy, provider-status vocabulary, and provider capability semantics.

This document defines the provider-neutral values consumed by the extension and emitted by the native host. Raw provider errors MAY be retained as diagnostic metadata, but browser UI logic MUST branch only on the normalized fields defined here.

## 1. Normalized error object

Every `response.failed` event MUST contain:

```json
{
  "error": {
    "code": "PROVIDER_FAILED",
    "reason": "PROCESS_EXITED",
    "message": "The provider process exited unexpectedly.",
    "retryable": true
  }
}
```

### Required fields

| Field | Type | Meaning |
|---|---|---|
| `code` | string enum | Stable application-level error category. |
| `reason` | string | Stable machine-readable subreason. |
| `message` | string | Human-readable explanation; never used for program branching. |
| `retryable` | boolean | Whether retrying the same logical operation may succeed without user reconfiguration. |

Optional provider/native details belong under:

```json
{
  "metadata": {}
}
```

Metadata MUST NOT contain credentials, tokens, authentication files, full page content, or full prompts.

## 2. Error code taxonomy

### `HOST_NOT_INSTALLED`
The browser cannot invoke the native companion because the Native Messaging host is not installed/registered.

Typical reasons:
- `NATIVE_HOST_NOT_FOUND`
- `NATIVE_HOST_NOT_REGISTERED`

Default retryability: **false** until installation/registration changes.

### `HOST_UNAVAILABLE`
The host is installed/known but cannot currently complete the connection or startup handshake.

Typical reasons:
- `HOST_START_FAILED`
- `HOST_DISCONNECTED`
- `HOST_PROTOCOL_MISMATCH`
- `HOST_INITIALIZATION_FAILED`

Retryability depends on reason. Transient disconnect/start failures MAY be retryable; protocol mismatch is not.

### `PROVIDER_NOT_FOUND`
The requested provider runtime/executable is unavailable.

Typical reasons:
- `EXECUTABLE_NOT_FOUND`
- `PROVIDER_NOT_INSTALLED`
- `PROVIDER_DISABLED`

Default retryability: **false** until provider installation/configuration changes.

### `PROVIDER_NOT_AUTHENTICATED`
The provider runtime exists but has no usable authenticated session.

Typical reasons:
- `LOGIN_REQUIRED`
- `AUTH_EXPIRED`
- `AUTH_REJECTED`

Default retryability: **false** until authentication state changes.

### `PROVIDER_FAILED`
The provider was available and authenticated but failed while servicing the request.

Typical reasons:
- `PROCESS_EXITED`
- `MALFORMED_PROVIDER_OUTPUT`
- `PROVIDER_RATE_LIMITED`
- `PROVIDER_UNAVAILABLE`
- `PROVIDER_REJECTED_REQUEST`
- `PROVIDER_INTERNAL_ERROR`
- `QUEUE_FULL` — the shared Seatline companion has too many requests queued (retryable).
- `PROVIDER_TIMEOUT` — the turn hit the shared companion's time limit (retryable).
- `COMPANION_DISCONNECTED` — the connection to the shared companion ended before the request did (retryable).

Retryability is reason-dependent:
- rate limiting/unavailability/process crash MAY be retryable;
- rejected request is normally not retryable without modifying the request.

The Codex adapter reports `WORKSPACE_UNAVAILABLE` when the directory it runs Codex in can't be created, or other users could change it (SEC-02). It isn't retryable until the directory's owner or permissions change.

### `SEARCH_FAILED`
The user requested web search, but TabBeam could not start a supported native-search turn.

Current reasons:
- `NATIVE_SEARCH_UNSUPPORTED` — the selected AI provider does not expose authenticated native web search.
- `SEARCH_WITH_CONTEXT_UNSUPPORTED` — search was requested together with browser context. TabBeam currently refuses that combination so attacker-controlled page text never gains live network access.
- `NATIVE_SEARCH_CONFIGURATION_UNSAFE` — provider-local configuration would enable additional tool surfaces TabBeam cannot safely bound for a native-search turn.
- `NATIVE_SEARCH_NO_SOURCES` — the provider completed a search turn without any usable normalized sources. TabBeam fails the turn instead of silently presenting an ungrounded answer.

Provider-native search runs inside the selected provider turn. Authentication, rate-limit, timeout, cancellation, and provider-service failures therefore keep their ordinary provider/request error categories rather than being reclassified as search-backend failures.

### `REQUEST_CANCELLED`
The target request was cancelled intentionally.

Typical reasons:
- `USER_CANCELLED`
- `INPUT_CLOSED`: the extension closed the native connection while the request was in flight, so the host stopped it (PRO-04).

Default retryability: **true** as a new request, but the cancelled request itself is terminal.

### `REQUEST_TIMEOUT`
A configured request/provider timeout expired.

Typical reasons:
- `PROVIDER_START_TIMEOUT`
- `PROVIDER_RESPONSE_TIMEOUT`
- `REQUEST_DEADLINE_EXCEEDED`

Default retryability: **true**, subject to provider state.

### `CONTEXT_UNAVAILABLE`
Requested browser context could not be supplied safely or at all.

Typical reasons:
- `PAGE_ACCESS_DENIED`
- `PAGE_NOT_SCRIPTABLE`
- `SELECTION_UNAVAILABLE`
- `PAGE_EXTRACTION_FAILED`
- `CONTEXT_TOO_LARGE`

Retryability is reason-dependent.

### `INVALID_REQUEST`
The browser/native request is malformed, unsupported, or violates protocol constraints.

Required v1 protocol reasons include:
- `MALFORMED_MESSAGE`
- `INVALID_ENVELOPE`
- `INVALID_PAYLOAD`
- `UNKNOWN_METHOD`
- `UNSUPPORTED_PROTOCOL_VERSION`
- `DUPLICATE_REQUEST_ID`
- `UNKNOWN_TARGET_REQUEST`
- `PAGE_CONTEXT_UNSUPPORTED`
- `PAGE_CONTEXT_TOOLS_ENABLED`
- `MODEL_SELECTION_UNSUPPORTED`

The default build, which runs providers through the shared Seatline companion, adds two reasons under `INVALID_REQUEST`: `APP_NOT_AUTHORIZED` (TabBeam has no grant in the companion) and `PROVIDER_DEFAULT_TOOLS_DENIED` (the grant does not include `--allow-provider-default`, which a plain, context-free question needs because it uses the provider's own tool settings).

The extension also reports `REQUEST_TOO_LARGE` when it refuses to send a request that would exceed the Native Messaging frame limit (SEC-01, `docs/protocol/native-messaging-v1.json`). The host never receives such a request.

A provider adapter reports `UNKNOWN_CONVERSATION` when `conversation_id` has no recoverable session and no usable dialogue history. Codex recovers its native session mapping after host restarts, or starts a new provider session from bounded `input.history` if the mapping has been lost or the resumed thread fails before a turn starts. The host rejects attached browser context with `PAGE_CONTEXT_UNSUPPORTED` when the selected provider does not report `page_context: true`, rather than silently answering without it (§6). Codex reports `page_context: true`; for a context turn it consumes validated browser context only with shell/image/apps/plugins/hooks/web-search/orchestrator-MCP/subagent surfaces disabled. Plugin/cache artifacts do not block the turn. It fails with `PAGE_CONTEXT_TOOLS_ENABLED` only when user-level standalone `mcp_servers` configuration is present and cannot yet be disabled deterministically.

The host likewise rejects a `conversation.send` `model` with `MODEL_SELECTION_UNSUPPORTED` when the selected provider does not report `model_selection: true`, rather than answering with the provider's default. Codex and Claude report `model_selection: true` and pass the model to their CLIs as a single `--model=<id>` argument. Claude suggests its CLI's aliases (`sonnet`, `opus`, `haiku`), which track the latest model of each family; Codex has no stable way to list its models, so it suggests none and passes any valid model ID on. A model a provider doesn't recognize fails the turn as a provider failure.

Default retryability: **false** unless the caller changes the request.

### `INTERNAL_ERROR`
An unexpected host-side condition occurred that cannot be classified more specifically.

Typical reasons:
- `ALLOCATION_FAILED`
- `INTERNAL_STATE_ERROR`
- `UNEXPECTED_FAILURE`
- `SESSION_STORE_FAILED` — a provider session could not be recorded for later continuation.
- `SESSION_FORGET_FAILED` — a forgotten conversation's session mapping or provider transcript could not be removed (retryable).
- `SESSION_LIMIT_REACHED` — the shared companion has reached its stored-session limit for TabBeam.

Default retryability: **false** unless the implementation explicitly knows the condition is transient.

## 3. Error mapping rules

1. Implementations MUST choose the most specific normalized `code` available.
2. Provider-specific exit codes, stderr text, HTTP status, or CLI messages MUST NOT become new top-level error codes.
3. Raw details MAY appear under sanitized `metadata`.
4. The extension MUST NOT branch on `message` or raw provider metadata.
5. Unknown provider failures map to `PROVIDER_FAILED`, not `INTERNAL_ERROR`.
6. `SEARCH_FAILED` is reserved for search-mode capability/safety/grounding failures such as `NATIVE_SEARCH_UNSUPPORTED`, `SEARCH_WITH_CONTEXT_UNSUPPORTED`, `NATIVE_SEARCH_CONFIGURATION_UNSAFE`, or `NATIVE_SEARCH_NO_SOURCES`; provider service/authentication/rate-limit failures keep their ordinary provider/request categories.
7. `INTERNAL_ERROR` is reserved for failures inside TabBeam's own host/runtime where no more specific category applies.
8. Cancellation of a target request MUST terminate that target with `REQUEST_CANCELLED`.
9. Unsupported versions, methods, fields, or payload shape errors MUST map to `INVALID_REQUEST`.

## 4. Provider status vocabulary

A successful `provider.status` event MUST use:

```json
{
  "provider_id": "codex",
  "status": {
    "availability": "available",
    "authentication": "authenticated",
    "capabilities": {
      "streaming": true,
      "continuation": true,
      "web_search": "unknown",
      "page_context": true,
      "attachments": false,
      "model_selection": true,
      "cancellation": true
    }
  }
}
```

### Availability

`availability` MUST be one of:

- `available` — runtime is discovered and invocable.
- `unavailable` — runtime is known but cannot currently be invoked.
- `not_found` — runtime/executable is not installed/discovered.
- `unknown` — host cannot determine availability safely.

### Authentication

`authentication` MUST be one of:

- `authenticated` — provider tooling reports a usable authenticated session.
- `unauthenticated` — provider tooling is available but requires login.
- `unknown` — the host cannot determine authentication without attempting a request.

Authentication state MUST NOT be inferred from browser cookies or scraped consumer web sessions.

## 5. Capability vocabulary

Each capability is either:

- `true` — supported by the current provider/runtime path;
- `false` — explicitly unsupported; or
- `"unknown"` — cannot be determined reliably before use.

The v1 capability keys are:

| Capability | Meaning |
|---|---|
| `streaming` | Provider can emit incremental response chunks through the adapter. |
| `continuation` | Provider supports safe continuation of an existing provider-side/session conversation. |
| `web_search` | Provider can perform web-grounded retrieval through its authenticated native runtime. |
| `page_context` | Adapter can accept browser page/selection context supplied by TabBeam. |
| `attachments` | Adapter can accept supported non-text attachments. |
| `model_selection` | Adapter passes a chosen model (`conversation.send` `model`) to the provider. It MAY suggest models in `status.models`. |
| `cancellation` | In-flight provider work can be actively cancelled rather than merely ignored. |

Rules:

1. Capability keys are provider-runtime capabilities, not UI feature flags.
2. The extension MUST respond to capability values and MUST NOT hard-code provider names.
3. `unknown` MUST NOT be treated as `true`.
4. Absence of a required v1 capability key is invalid provider status.
5. A capability MAY change across provider versions, local installations, or authentication state.
6. Search mode currently requires `web_search: true`; providers without native search are not silently routed through a separate external backend.

## 6. Status/error consistency

Recommended deterministic mappings:

| Provider status | Request-time error |
|---|---|
| `availability = not_found` | `PROVIDER_NOT_FOUND` |
| `availability = unavailable` | `PROVIDER_FAILED` or `HOST_UNAVAILABLE`, depending on whether failure is provider-local or host-local |
| `authentication = unauthenticated` | `PROVIDER_NOT_AUTHENTICATED` |
| required capability = `false` | `INVALID_REQUEST` with a capability-specific reason |
| required capability = `unknown` | Adapter MAY probe; if unsupported, map deterministically rather than silently degrading |

## 7. UI guidance

The UI should render normalized state without exposing provider internals:

- `HOST_NOT_INSTALLED` → installation action
- `PROVIDER_NOT_FOUND` → provider setup guidance
- `PROVIDER_NOT_AUTHENTICATED` → authentication guidance
- retryable `PROVIDER_FAILED` / `SEARCH_FAILED` / `REQUEST_TIMEOUT` → retry action
- `REQUEST_CANCELLED` → neutral cancelled state
- `INVALID_REQUEST` / `INTERNAL_ERROR` → safe generic failure plus diagnostics identifier when available

Provider-specific raw stderr MUST NOT be shown directly as the primary user-facing message.

## 8. Machine-readable fixtures

The normative machine-readable companion files are:

- `docs/protocol/schemas/error.schema.json`
- `docs/protocol/schemas/provider-status.schema.json`
- `docs/protocol/fixtures/v1-errors-capabilities.json`

These fixtures are used by NAT-03, PRO-01, EXT-04, and TST-01/TST-10.
