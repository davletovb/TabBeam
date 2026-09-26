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
    /** @type {FakeOption[]} */
    this.options = [];
    this.value = "";
    this.disabled = false;
    /** @type {Map<string, (event: any) => void>} */
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
    /** @type {Map<string, string>} */
    this.attributes = new Map();
    /** @type {Map<string, (event: any) => void>} */
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

/** @param {boolean} pageContext */
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

/** @returns {{promise: Promise<any>, resolve(value: any): void, reject(error: any): void}} */
function deferred() {
  /** @type {(value: any) => void} */
  let resolve = () => {};
  /** @type {(error: any) => void} */
  let reject = () => {};
  const promise = new Promise((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}

/** Let async selector initialization reach its provider probes. */
async function tick() {
  await Promise.resolve();
  await Promise.resolve();
}

{
  const select = new FakeSelect();
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
      async get() { return { [PROVIDER_STORAGE_KEY]: "claude" }; },
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

  const beforeSameLock = changes.length;
  selector.lock("codex");
  assert.equal(select.disabled, true);
  assert.equal(changes.length, beforeSameLock, "locking the same provider does not replace fresh outcome UI");

  selector.unlock();
  assert.equal(select.disabled, false);
  assert.equal(selector.getProviderId(), "codex");
}

{
  // A user choice made while the two discovery calls are pending wins.
  const select = new FakeSelect();
  const codex = deferred();
  const claude = deferred();
  /** @type {any[]} */
  const changes = [];
  const selector = bindProviderSelector(
    /** @type {any} */ (select),
    {
      /** @param {any} message */
      sendMessage(message) {
        return message.provider_id === "codex" ? codex.promise : claude.promise;
      }
    },
    {
      async get() { return { [PROVIDER_STORAGE_KEY]: "codex" }; },
      async set() {}
    },
    { onChange(selection) { changes.push(selection); } }
  );
  await tick();
  select.change("claude");
  codex.resolve({ provider_id: "codex", status: ready(true) });
  claude.resolve({ provider_id: "claude", status: ready(false) });
  await selector.ready;

  assert.equal(selector.getProviderId(), "claude");
  assert.equal(select.value, "claude");
  assert.equal(changes.at(-1)?.providerId, "claude");
  assert.equal(changes.at(-1)?.status?.capabilities?.page_context, false);
  assert.equal(changes.at(-1)?.statusUpdated, true);
}

{
  // A conversation lock made before discovery completes also wins, then gets
  // its capability data when the probes finish.
  const select = new FakeSelect();
  const codex = deferred();
  const claude = deferred();
  /** @type {any[]} */
  const changes = [];
  const selector = bindProviderSelector(
    /** @type {any} */ (select),
    {
      /** @param {any} message */
      sendMessage(message) {
        return message.provider_id === "codex" ? codex.promise : claude.promise;
      }
    },
    {
      async get() { return { [PROVIDER_STORAGE_KEY]: "codex" }; },
      async set() {}
    },
    { onChange(selection) { changes.push(selection); } }
  );
  await tick();
  selector.lock("claude");
  codex.resolve({ provider_id: "codex", status: ready(true) });
  claude.resolve({ provider_id: "claude", status: ready(false) });
  await selector.ready;

  assert.equal(selector.getProviderId(), "claude");
  assert.equal(select.disabled, true);
  assert.equal(changes.at(-1)?.providerId, "claude");
  assert.equal(changes.at(-1)?.status?.capabilities?.page_context, false);
}

{
  // Leaving a stored conversation restores the person's saved provider.
  const select = new FakeSelect();
  const selector = bindProviderSelector(
    /** @type {any} */ (select),
    {
      async sendMessage(message) {
        return { provider_id: message.provider_id, status: ready(message.provider_id === "codex") };
      }
    },
    {
      async get() { return { [PROVIDER_STORAGE_KEY]: "claude" }; },
      async set() {}
    }
  );
  await selector.ready;
  selector.lock("codex");
  assert.equal(selector.getProviderId(), "codex");
  selector.unlock();
  assert.equal(selector.getProviderId(), "claude");
}

{
  // An unavailable saved preference falls back to a usable provider.
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
  if (!claudeOption) throw new Error("Claude option is missing");
  assert.equal(claudeOption.disabled, true);
}

{
  // Capability refreshes must not throw away a context-menu handoff or retry
  // context. Only a real supported -> unsupported transition clears it.
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
  const finishHandoff = controls.beginMenuHandoff();
  const captured = {
    mode: "selection",
    text: "selected text",
    page: { title: "Example", url: "https://example.com/" },
    truncated: false
  };
  finishHandoff?.({ available: true, result: { ok: true, context: captured } });
  assert.equal(controls.getContext(), captured);

  controls.setSupported(true, "Codex");
  controls.setSupported(true, "Codex");
  assert.equal(controls.getContext(), captured, "same-support refresh preserves context");

  controls.setSupported(false, "Claude");
  assert.equal(controls.getContext(), null);
  assert.equal(elements.selection.disabled, true);
  assert.equal(elements.page.disabled, true);
  assert.equal(elements.status.textContent, "Claude doesn't support browser context yet.");

  controls.setSupported(true, "Codex");
  assert.equal(elements.selection.disabled, false);
  assert.equal(elements.page.disabled, false);
}

console.log("EXT-15 provider selector tests passed");
