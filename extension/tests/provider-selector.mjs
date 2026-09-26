import assert from "node:assert/strict";
import { bindProviderSelector, PROVIDER_STORAGE_KEY } from "../src/shared/provider-selector.js";
import { bindContextControls } from "../src/popup/context-controls.js";

class FakeOption {
  constructor() {
    this.value = "";
    this.textContent = "";
    this.disabled = false;
  }
}

class FakeSelect {
  constructor() {
    this.options = [];
    this.value = "";
    this.disabled = false;
    this.listeners = new Map();
    this.ownerDocument = { createElement: () => new FakeOption() };
  }
  /** @param {...FakeOption} children */
  replaceChildren(...children) {
    this.options = children;
    if (children[0]) this.value = children[0].value;
  }
  /** @param {string} type @param {(event: any) => void} callback */
  addEventListener(type, callback) {
    this.listeners.set(type, callback);
  }
  /** @param {string} value */
  change(value) {
    this.value = value;
    this.listeners.get("change")?.({});
  }
}

class FakeButton {
  constructor() {
    this.disabled = false;
    this.attributes = new Map();
    this.listeners = new Map();
  }
  /** @param {string} name @param {string} value */
  setAttribute(name, value) { this.attributes.set(name, String(value)); }
  /** @param {string} type @param {(event: any) => void} callback */
  addEventListener(type, callback) { this.listeners.set(type, callback); }
}

class FakeText {
  constructor() {
    this.textContent = "";
    this.hidden = false;
  }
}

function ready(pageContext) {
  return {
    availability: "available",
    authentication: "authenticated",
    capabilities: {
      streaming: true,
      continuation: true,
      web_search: "unknown",
      page_context: pageContext,
      attachments: false,
      model_selection: false,
      cancellation: true
    }
  };
}

{
  const select = new FakeSelect();
  const saved = { [PROVIDER_STORAGE_KEY]: "claude" };
  /** @type {any[]} */
  const writes = [];
  /** @type {any[]} */
  const changes = [];
  const selector = bindProviderSelector(
    /** @type {any} */ (select),
    {
      async sendMessage(message) {
        return {
          provider_id: message.provider_id,
          status: ready(message.provider_id === "codex")
        };
      }
    },
    {
      async get() { return saved; },
      async set(value) { writes.push(value); }
    },
    { onChange(selection) { changes.push(selection); } }
  );
  await selector.ready;

  assert.deepEqual(select.options.map((option) => option.value), ["codex", "claude"]);
  assert.equal(selector.getProviderId(), "claude");
  assert.equal(changes.at(-1)?.status?.capabilities?.page_context, false);

  select.change("codex");
  assert.equal(selector.getProviderId(), "codex");
  assert.deepEqual(writes.at(-1), { [PROVIDER_STORAGE_KEY]: "codex" });

  selector.lock("claude");
  assert.equal(select.disabled, true);
  assert.equal(selector.getProviderId(), "claude");
  select.change("codex");
  assert.equal(selector.getProviderId(), "claude", "locked conversations cannot switch provider");

  selector.unlock();
  assert.equal(select.disabled, false);
}

{
  const select = new FakeSelect();
  const selector = bindProviderSelector(
    /** @type {any} */ (select),
    {
      async sendMessage(message) {
        return message.provider_id === "claude"
          ? {
              provider_id: "claude",
              status: {
                ...ready(false),
                availability: "not_found",
                authentication: "unknown"
              }
            }
          : { provider_id: "codex", status: ready(true) };
      }
    },
    {
      async get() { return { [PROVIDER_STORAGE_KEY]: "claude" }; },
      async set() {}
    }
  );
  await selector.ready;
  assert.equal(selector.getProviderId(), "codex");
  const claudeOption = select.options.find((option) => option.value === "claude");
  assert.ok(claudeOption);
  assert.equal(claudeOption.disabled, true);
}

{
  const elements = {
    none: new FakeButton(),
    selection: new FakeButton(),
    page: new FakeButton(),
    status: new FakeText(),
    preview: new FakeText()
  };
  const controls = bindContextControls(
    /** @type {any} */ (elements),
    { async sendMessage() { throw new Error("should not capture"); } }
  );
  controls.setSupported(false, "Claude");
  assert.equal(elements.selection.disabled, true);
  assert.equal(elements.page.disabled, true);
  assert.equal(elements.status.textContent, "Claude doesn't support browser context yet.");

  controls.setSupported(true, "Codex");
  assert.equal(elements.selection.disabled, false);
  assert.equal(elements.page.disabled, false);
}

console.log("EXT-15 provider selector tests passed");
