import assert from "node:assert/strict";
import { bindProviderSettings } from "../src/setup/provider-settings.js";
import { MODEL_PREFERENCES_KEY } from "../src/shared/models.js";
import { PROVIDER_STORAGE_KEY, bindProviderSelector } from "../src/shared/provider-selector.js";

/**
 * @param {string} availability @param {string} authentication
 * @param {any} [modelSelection] @param {{id: string, label: string}[]} [models]
 */
function status(availability, authentication, modelSelection = false, models = undefined) {
  return {
    ...(models ? { models } : {}),
    availability,
    authentication,
    capabilities: {
      streaming: true,
      continuation: true,
      web_search: "unknown",
      page_context: true,
      attachments: false,
      model_selection: modelSelection,
      cancellation: true
    }
  };
}

class Node {
  /** @param {string} tag */
  constructor(tag) {
    this.tag = tag;
    /** @type {any[]} */
    this.children = [];
    /** @type {Map<string, string>} */
    this.attributes = new Map();
    /** @type {Map<string, (() => void)[]>} */
    this.listeners = new Map();
    this.className = "";
    this.textContent = "";
    this.type = "";
    this.name = "";
    this.value = "";
    this.checked = false;
    this.disabled = false;
    this.hidden = false;
    this.focused = false;
    this.ownerDocument = doc;
  }
  /** @param {...any} nodes */
  append(...nodes) { this.children.push(...nodes); }
  /** @param {...any} nodes */
  replaceChildren(...nodes) { this.children = nodes; }
  /** @param {string} name @param {string} value */
  setAttribute(name, value) { this.attributes.set(name, value); }
  /** @param {string} name */
  getAttribute(name) { return this.attributes.get(name) ?? null; }
  /** @param {string} name */
  removeAttribute(name) { this.attributes.delete(name); }
  /** @param {string} type @param {() => void} listener */
  addEventListener(type, listener) { this.listeners.set(type, [...this.listeners.get(type) ?? [], listener]); }
  /** @param {string} type */
  fire(type) { for (const listener of this.listeners.get(type) ?? []) listener(); }
  focus() { this.focused = true; }
}
const doc = {
  /** @param {string} tag */
  createElement(tag) { return new Node(tag); }
};

// ---------- The setup page's provider and model choice ----------
{
  const options = new Node("fieldset");
  const model = new Node("select");
  const modelNote = new Node("p");
  const customField = new Node("div");
  const customInput = new Node("input");
  const customSave = new Node("button");
  /** @type {Record<string, any>} */
  const saved = { [PROVIDER_STORAGE_KEY]: "claude" };
  /** @type {any[]} */
  const writes = [];
  /** @type {((changes: any, area: string) => void)[]} */
  const listeners = [];
  /** @type {any[]} */
  const checks = [];
  const settings = bindProviderSettings(
    /** @type {any} */ ({ options, model, modelNote, customField, customInput, customSave }),
    {
      async sendMessage(message) {
        checks.push(message);
        return message.provider_id === "codex"
          ? { provider_id: "codex", status: status("available", "authenticated") }
          : {
            provider_id: "claude",
            status: status("not_found", "unknown", true, [{ id: "sonnet", label: "Sonnet (latest)" }, { id: "opus", label: "Opus (latest)" }])
          };
      }
    },
    {
      async get(/** @type {string} */ key) { return key === PROVIDER_STORAGE_KEY ? { ...saved } : {}; },
      async set(values) { writes.push(values); }
    },
    { addListener: (listener) => { listeners.push(listener); } }
  );
  await settings.ready;

  const rows = options.children;
  const radio = (/** @type {number} */ i) => rows[i].children[0];
  const line = (/** @type {number} */ i) => rows[i].children[1].children[1];
  assert.deepEqual(rows.map((row) => row.children[1].children[0].textContent), ["Codex", "Claude"]);
  assert.equal(radio(1).checked, true, "the saved choice is selected");
  assert.equal(radio(0).checked, false);
  assert.equal(settings.getProviderId(), "claude");
  // Every provider is checked, and the page never overwrites the popup's
  // diagnostics.
  assert.deepEqual(checks.map((check) => [check.provider_id, check.record_diagnostics]), [["codex", false], ["claude", false]]);
  assert.equal(line(0).textContent, "Codex is ready.");
  assert.equal(line(0).getAttribute("data-state"), "ready");
  assert.equal(line(1).getAttribute("data-kind"), "provider-missing", "what's left to set up shows per provider");

  // Claude can switch models and suggests some: the field lists them, then
  // "Other model…".
  const values = () => model.children.map((/** @type {any} */ node) => node.value);
  assert.equal(model.disabled, false);
  assert.deepEqual(values(), ["", "sonnet", "opus", "other"]);
  assert.equal(model.children[0].textContent, "Claude default");
  assert.equal(model.value, "");
  assert.ok(/its default model/.test(modelNote.textContent));

  // Choosing a suggestion saves it for Claude only.
  model.value = "opus";
  model.fire("change");
  assert.deepEqual(writes.at(-1), { [MODEL_PREFERENCES_KEY]: { claude: "opus" } });
  assert.equal(model.value, "opus");
  assert.ok(/use opus/.test(modelNote.textContent));

  // Another model ID: validated before it's saved.
  model.value = "other";
  model.fire("change");
  assert.equal(customField.hidden, false);
  customInput.value = "--config=evil";
  customSave.fire("click");
  assert.equal(customInput.getAttribute("aria-invalid"), "true");
  assert.equal(modelNote.getAttribute("data-state"), "error");
  assert.deepEqual(writes.at(-1), { [MODEL_PREFERENCES_KEY]: { claude: "opus" } }, "an invalid ID isn't saved");
  customInput.value = " claude-opus-4-1@20250805 ";
  customSave.fire("click");
  assert.deepEqual(writes.at(-1), { [MODEL_PREFERENCES_KEY]: { claude: "claude-opus-4-1@20250805" } });
  assert.equal(model.value, "other", "an unlisted model shows as Other");
  assert.equal(customInput.value, "claude-opus-4-1@20250805");

  // Back to the default removes the entry.
  model.value = "";
  model.fire("change");
  assert.deepEqual(writes.at(-1), { [MODEL_PREFERENCES_KEY]: {} });
  assert.equal(customField.hidden, true);

  // Choosing a provider saves the preference; Codex can't switch here, so
  // its field is locked on the default.
  const before = writes.length;
  radio(0).checked = true;
  radio(0).fire("change");
  assert.deepEqual(writes.slice(before), [{ [PROVIDER_STORAGE_KEY]: "codex" }]);
  assert.equal(radio(1).checked, false);
  assert.equal(model.disabled, true);
  assert.deepEqual(values(), [""]);
  assert.equal(model.children[0].textContent, "Codex default");
  assert.ok(/can't switch its model/.test(modelNote.textContent));

  // Changes saved elsewhere show here without saving again.
  const count = writes.length;
  for (const listener of listeners) {
    listener({ [PROVIDER_STORAGE_KEY]: { newValue: "claude" }, [MODEL_PREFERENCES_KEY]: { newValue: { claude: "sonnet" } } }, "local");
  }
  assert.equal(radio(1).checked, true);
  assert.equal(model.value, "sonnet");
  assert.equal(writes.length, count);
}

{
  // A saved model is shown when the page opens; junk in storage is ignored.
  const model = new Node("select");
  const settings = bindProviderSettings(
    /** @type {any} */ ({ options: new Node("fieldset"), model, modelNote: new Node("p") }),
    {
      async sendMessage(message) {
        return { provider_id: message.provider_id, status: status("available", "authenticated", true, [{ id: "sonnet", label: "Sonnet" }]) };
      }
    },
    {
      async get(/** @type {string} */ key) {
        return key === MODEL_PREFERENCES_KEY
          ? { [key]: { codex: "gpt-5-codex", claude: "-rm" } }
          : { [PROVIDER_STORAGE_KEY]: "codex" };
      },
      async set() {}
    }
  );
  await settings.ready;
  assert.equal(model.value, "other");
  assert.equal(model.disabled, false);
}

{
  // An unknown saved value or a storage failure falls back to the default.
  const settings = bindProviderSettings(
    /** @type {any} */ ({ options: new Node("fieldset"), model: new Node("select"), modelNote: new Node("p") }),
    { async sendMessage() { throw new Error("worker gone"); } },
    { async get() { throw new Error("no storage"); }, async set() {} }
  );
  await settings.ready;
  assert.equal(settings.getProviderId(), "codex");
}

// ---------- The popup and full view follow the saved choice ----------
{
  /** @type {((changes: any, area: string) => void)[]} */
  const listeners = [];
  /** @type {any[]} */
  const selections = [];
  const selector = bindProviderSelector(
    null,
    {
      async sendMessage(message) {
        return { provider_id: message.provider_id, status: status("available", "authenticated") };
      }
    },
    { async get() { return { [PROVIDER_STORAGE_KEY]: "codex" }; }, async set() { throw new Error("views don't save"); } },
    {
      storageChanges: { addListener: (listener) => { listeners.push(listener); } },
      onChange: (selection) => selections.push(selection.providerId)
    }
  );
  await selector.ready;
  assert.equal(selector.getProviderId(), "codex");

  /** @param {string} id */
  const setupChooses = (id) => {
    for (const listener of listeners) listener({ [PROVIDER_STORAGE_KEY]: { newValue: id } }, "local");
  };
  setupChooses("claude");
  assert.equal(selector.getProviderId(), "claude", "a choice made on the setup page applies at once");
  assert.equal(selections.at(-1), "claude");

  // An open conversation keeps its provider; the new choice waits for the next one.
  selector.lock("claude");
  setupChooses("codex");
  assert.equal(selector.getProviderId(), "claude");
  selector.unlock();
  assert.equal(selector.getProviderId(), "codex");

  // Same while a first question is in flight.
  selector.hold(true);
  setupChooses("claude");
  assert.equal(selector.getProviderId(), "codex");
  selector.hold(false);
}

{
  // A preferred provider that isn't installed falls back to one that is.
  const selector = bindProviderSelector(
    null,
    {
      async sendMessage(message) {
        return {
          provider_id: message.provider_id,
          status: message.provider_id === "claude" ? status("not_found", "unknown") : status("available", "authenticated")
        };
      }
    },
    { async get() { return { [PROVIDER_STORAGE_KEY]: "claude" }; }, async set() {} }
  );
  assert.equal(await selector.ready, "codex");
}

console.log("EXT-15 provider settings page and headless selector tests passed");
