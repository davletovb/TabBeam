import assert from "node:assert/strict";
import { serveConversationAskPort } from "../src/background/conversation-bridge.js";
import { normalizedStatus } from "../src/background/status-bridge.js";
import {
  MODEL_PREFERENCES_KEY,
  followModelPreferences,
  isModelId,
  readModelPreferences,
  suggestedModels
} from "../src/shared/models.js";
import { ASK_PORT_NAME } from "../src/shared/ask-port.js";
import { MockPort } from "./support/mock-port.mjs";

// ---------- Model IDs: the host's rule, checked in the extension too ----------
for (const model of ["sonnet", "gpt-5-codex", "claude-opus-4-1@20250805", "org/model:v1.2", "o3", "m".repeat(128)]) {
  assert.equal(isModelId(model), true, model);
}
for (const model of ["", "-o", "--config=x", " sonnet", "son net", "sonnet\n", "/bin/sh", "../m", "a;id", "$(id)", "sónnet", "m".repeat(129), 7, null, undefined]) {
  assert.equal(isModelId(model), false, JSON.stringify(model));
}

assert.deepEqual(
  suggestedModels([
    { id: "sonnet", label: "Sonnet (latest)" },
    { id: "-evil", label: "Evil" },
    { id: "opus", label: "" },
    { id: "haiku", label: "x".repeat(65) },
    { id: "opus", label: "Opus", extra: "dropped" },
    "sonnet"
  ]),
  [{ id: "sonnet", label: "Sonnet (latest)" }, { id: "opus", label: "Opus" }]
);
assert.equal(suggestedModels(Array.from({ length: 40 }, (_, i) => ({ id: `m${i}`, label: `M${i}` }))).length, 32);
assert.deepEqual(suggestedModels("sonnet"), []);

// ---------- Provider status: suggestions pass only where a model can be chosen ----------
/** @param {any} modelSelection @param {any} models */
function status(modelSelection, models) {
  return {
    availability: "available",
    authentication: "authenticated",
    capabilities: {
      streaming: true, continuation: true, web_search: "unknown", page_context: false,
      attachments: false, model_selection: modelSelection, cancellation: true
    },
    models
  };
}
assert.deepEqual(
  normalizedStatus(status(true, [{ id: "sonnet", label: "Sonnet" }, { id: "--x", label: "X" }]))?.models,
  [{ id: "sonnet", label: "Sonnet" }]
);
assert.equal("models" in /** @type {any} */ (normalizedStatus(status(false, [{ id: "sonnet", label: "Sonnet" }]))), false);
assert.equal("models" in /** @type {any} */ (normalizedStatus(status(true, []))), false);
assert.equal("models" in /** @type {any} */ (normalizedStatus(status("unknown", [{ id: "sonnet", label: "S" }]))), false);

// ---------- Saved models, followed by the popup and full view ----------
{
  /** @type {((changes: any, area: string) => void)[]} */
  const listeners = [];
  const models = followModelPreferences(
    { async get() { return { [MODEL_PREFERENCES_KEY]: { claude: "sonnet", codex: "-bad" } }; } },
    { addListener: (listener) => { listeners.push(listener); } }
  );
  await new Promise((resolve) => setTimeout(resolve, 0));
  assert.equal(models.modelFor("claude", true), "sonnet");
  assert.equal(models.modelFor("claude", false), undefined, "never sent to a provider that can't switch");
  assert.equal(models.modelFor("codex", true), undefined, "an invalid saved model is ignored");
  assert.equal(models.modelFor(undefined, true), undefined);
  for (const listener of listeners) listener({ [MODEL_PREFERENCES_KEY]: { newValue: { codex: "o3" } } }, "local");
  assert.equal(models.modelFor("codex", true), "o3", "a change on the setup page applies at once");
  assert.equal(models.modelFor("claude", true), undefined);
  assert.deepEqual(await readModelPreferences({ async get() { throw new Error("no storage"); } }), {});
}

// ---------- The worker refuses a malformed model before anything runs ----------
{
  /** @type {any[]} */
  const sent = [];
  const port = new MockPort(ASK_PORT_NAME);
  serveConversationAskPort(/** @type {any} */ (port), {
    manager: { send(/** @type {any} */ request) { sent.push(request); } },
    store: /** @type {any} */ ({}),
    inFlight: new Set()
  });
  port.emitMessage({ type: "ask", text: "Hello", provider_id: "claude", model: "--config=evil" });
  await new Promise((resolve) => setTimeout(resolve, 0));
  assert.equal(sent.length, 0, "nothing reached the host");
  const failure = port.messages.at(-1);
  assert.equal(failure.event, "response.failed");
  assert.equal(failure.payload.error.reason, "INVALID_PAYLOAD");
  assert.ok(/Provider & setup/.test(failure.payload.error.message));
}

console.log("EXT-15 model choice tests passed");
