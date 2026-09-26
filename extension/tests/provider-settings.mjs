import assert from "node:assert/strict";
import { bindProviderSettings } from "../src/setup/provider-settings.js";
import { PROVIDER_STORAGE_KEY, bindProviderSelector } from "../src/shared/provider-selector.js";

/** @param {string} availability @param {string} authentication @param {any} [modelSelection] */
function status(availability, authentication, modelSelection = false) {
  return {
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
  /** @type {Record<string, any>} */
  const saved = { [PROVIDER_STORAGE_KEY]: "claude" };
  /** @type {any[]} */
  const writes = [];
  /** @type {((changes: any, area: string) => void)[]} */
  const listeners = [];
  /** @type {any[]} */
  const checks = [];
  const settings = bindProviderSettings(
    /** @type {any} */ ({ options, model, modelNote }),
    {
      async sendMessage(message) {
        checks.push(message);
        return message.provider_id === "codex"
          ? { provider_id: "codex", status: status("available", "authenticated") }
          : { provider_id: "claude", status: status("not_found", "unknown", true) };
      }
    },
    {
      async get() { return { ...saved }; },
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

  // Model choice follows the capability, and v1 can't pass a model: the
  // field stays on the provider's default either way.
  assert.equal(model.disabled, true);
  assert.equal(model.children[0].textContent, "Claude default");
  assert.ok(/can switch models/.test(modelNote.textContent));

  // Choosing saves the preference and updates the model field.
  radio(0).checked = true;
  radio(0).fire("change");
  assert.deepEqual(writes, [{ [PROVIDER_STORAGE_KEY]: "codex" }]);
  assert.equal(radio(1).checked, false);
  assert.equal(model.children[0].textContent, "Codex default");
  assert.ok(/uses the model set in its own configuration/.test(modelNote.textContent));

  // A change saved elsewhere shows here without saving again.
  for (const listener of listeners) listener({ [PROVIDER_STORAGE_KEY]: { newValue: "claude" } }, "local");
  assert.equal(radio(1).checked, true);
  assert.equal(writes.length, 1);
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
