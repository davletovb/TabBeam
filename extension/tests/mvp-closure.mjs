import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import {
  THEME_STORAGE_KEY,
  applyTheme,
  bindThemeSelect,
  loadTheme,
  normalizeThemePreference
} from "../src/shared/theme.js";
import { createDiagnosticsState } from "../src/background/diagnostics.js";
import { renderDiagnostics } from "../src/popup/diagnostics.js";

class FakeElement {
  constructor() {
    this.textContent = "";
    this.hidden = false;
    this.disabled = false;
    this.open = false;
    this.value = "";
    /** @type {Map<string, string>} */
    this.attributes = new Map();
    /** @type {Map<string, ((event?: any) => void)[]>} */
    this.listeners = new Map();
  }
  /** @param {string} name @param {string} value */
  setAttribute(name, value) { this.attributes.set(name, String(value)); }
  /** @param {string} name */
  removeAttribute(name) { this.attributes.delete(name); }
  /** @param {string} name */
  getAttribute(name) { return this.attributes.get(name) ?? null; }
  /** @param {string} type @param {(event?: any) => void} listener */
  addEventListener(type, listener) {
    this.listeners.set(type, [...(this.listeners.get(type) ?? []), listener]);
  }
  /** @param {string} type */
  dispatch(type) {
    for (const listener of this.listeners.get(type) ?? []) listener();
  }
}

{
  assert.equal(normalizeThemePreference("system"), "system");
  assert.equal(normalizeThemePreference("light"), "light");
  assert.equal(normalizeThemePreference("dark"), "dark");
  assert.equal(normalizeThemePreference("neon"), "system");

  /** @type {Record<string, any>} */
  const values = {};
  /** @type {Record<string, string>} */
  const cacheValues = {};
  const cache = {
    /** @param {string} key */
    getItem(key) { return cacheValues[key] ?? null; },
    /** @param {string} key @param {string} value */
    setItem(key, value) { cacheValues[key] = value; }
  };
  const storage = {
    /** @param {string} key */
    async get(key) { return { [key]: values[key] }; },
    /** @param {Record<string, any>} next */
    async set(next) { Object.assign(values, next); }
  };
  /** @type {((changes: Record<string, any>, area: string) => void)[]} */
  const changeListeners = [];
  const storageChanges = {
    /** @param {(changes: Record<string, any>, area: string) => void} callback */
    addListener(callback) { changeListeners.push(callback); }
  };
  const root = new FakeElement();
  const select = new FakeElement();

  assert.equal(applyTheme(/** @type {any} */ (root), "system"), "system");
  assert.equal(root.getAttribute("data-theme"), null);
  await bindThemeSelect(
    /** @type {any} */ (select),
    storage,
    /** @type {any} */ (root),
    storageChanges,
    /** @type {any} */ (cache)
  );
  assert.equal(select.value, "system");

  select.value = "dark";
  select.dispatch("change");
  await new Promise((resolve) => setTimeout(resolve, 0));
  assert.equal(values[THEME_STORAGE_KEY], "dark");
  assert.equal(root.getAttribute("data-theme"), "dark");

  const secondRoot = new FakeElement();
  assert.equal(
    await loadTheme(storage, /** @type {any} */ (secondRoot), /** @type {any} */ (cache)),
    "dark"
  );
  assert.equal(secondRoot.getAttribute("data-theme"), "dark");

  changeListeners[0]({ [THEME_STORAGE_KEY]: { newValue: "light" } }, "local");
  assert.equal(select.value, "light");
  assert.equal(root.getAttribute("data-theme"), "light");

  // Cached theme applies synchronously, and a user change made while the
  // authoritative storage read is pending is not overwritten by the late read.
  cacheValues[THEME_STORAGE_KEY] = "dark";
  let resolveGet = () => {};
  const slowStorage = {
    get() { return new Promise((resolve) => { resolveGet = () => resolve({ [THEME_STORAGE_KEY]: "light" }); }); },
    async set() {}
  };
  const slowRoot = new FakeElement();
  const slowSelect = new FakeElement();
  const pendingBind = bindThemeSelect(
    /** @type {any} */ (slowSelect),
    slowStorage,
    /** @type {any} */ (slowRoot),
    undefined,
    /** @type {any} */ (cache)
  );
  assert.equal(slowRoot.getAttribute("data-theme"), "dark");
  slowSelect.value = "dark";
  slowSelect.dispatch("change");
  resolveGet();
  await pendingBind;
  assert.equal(slowSelect.value, "dark");
}

{
  /** @type {Record<string, any>} */
  const sessionValues = {};
  const sessionStorage = {
    /** @param {string} key */
    async get(key) { return { [key]: sessionValues[key] }; },
    /** @param {Record<string, any>} values */
    async set(values) { Object.assign(sessionValues, structuredClone(values)); }
  };
  const diagnostics = createDiagnosticsState("0.1.0", sessionStorage, () => 1234);
  diagnostics.noteLifecycle({
    event: "host.ready",
    payload: { host_version: "0.1.0-dev", protocol_versions: [1] }
  });
  diagnostics.noteProvider({
    provider_id: "codex",
    status: {
      availability: "available",
      authentication: "authenticated",
      capabilities: {}
    }
  });
  diagnostics.noteFailure({
    code: "PROVIDER_FAILED",
    reason: "PROCESS_EXITED",
    message: "secret prompt text must never appear",
    retryable: true,
    prompt: "also-secret"
  });
  const summary = await diagnostics.summary();
  assert.equal(summary.extension_version, "0.1.0");
  assert.deepEqual(summary.host, {
    state: "available",
    version: "0.1.0-dev",
    protocol_versions: [1]
  });
  assert.deepEqual(summary.provider, {
    provider_id: "codex",
    availability: "available",
    authentication: "authenticated"
  });
  assert.deepEqual(summary.recent_failure, {
    code: "PROVIDER_FAILED",
    reason: "PROCESS_EXITED",
    retryable: true,
    at: 1234
  });
  assert.ok(!JSON.stringify(summary).includes("secret"));

  diagnostics.noteFailure({
    code: "INTERNAL_ERROR",
    reason: "token=definitely-not-a-safe-reason",
    message: "credential-looking material",
    retryable: false
  });
  assert.equal((await diagnostics.summary()).recent_failure?.reason, "UNKNOWN");

  // Host loss clears stale provider readiness, and the snapshot survives a
  // simulated MV3 worker restart through storage.session.
  diagnostics.noteDisconnect();
  const disconnected = await diagnostics.summary();
  assert.equal(disconnected.host.state, "unavailable");
  assert.equal(disconnected.provider.availability, "unknown");
  const restored = createDiagnosticsState("0.1.0", sessionStorage, () => 9999);
  assert.deepEqual(await restored.summary(), disconnected);

  const elements = {
    host: new FakeElement(),
    protocol: new FakeElement(),
    provider: new FakeElement(),
    failure: new FakeElement()
  };
  renderDiagnostics(/** @type {any} */ (elements), summary);
  assert.equal(elements.host.textContent, "Companion 0.1.0-dev");
  assert.equal(elements.protocol.textContent, "Extension v1; companion: 1");
  assert.equal(elements.provider.textContent, "Codex: available, authenticated");
  assert.equal(elements.failure.textContent, "PROVIDER_FAILED / PROCESS_EXITED");
}

{
  const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
  const popup = fs.readFileSync(path.join(root, "src/popup/index.html"), "utf8");
  const fullpage = fs.readFileSync(path.join(root, "src/fullpage/index.html"), "utf8");
  const setup = fs.readFileSync(path.join(root, "src/setup/index.html"), "utf8");
  const popupCss = fs.readFileSync(path.join(root, "src/popup/popup.css"), "utf8");
  const fullpageCss = fs.readFileSync(path.join(root, "src/fullpage/fullpage.css"), "utf8");
  const setupCss = fs.readFileSync(path.join(root, "src/setup/setup.css"), "utf8");

  for (const markup of [popup, fullpage]) {
    assert.ok(markup.includes('id="theme-select"'));
    assert.ok(markup.includes('value="system"'));
    assert.ok(markup.includes('value="light"'));
    assert.ok(markup.includes('value="dark"'));
    assert.ok(markup.includes('id="ask-cancel"'));
    assert.ok(markup.includes('id="ask-retry"'));
    assert.ok(markup.includes('aria-live="polite"'));
    assert.ok(markup.includes('aria-atomic="true"'));
  }
  assert.ok(popup.includes('id="diagnostics"'));
  assert.ok(popup.includes('id="companion-setup"'));
  assert.ok(popup.includes("../setup/index.html"));
  for (const markup of [popup, fullpage, setup]) {
    assert.ok(markup.includes("../shared/theme-bootstrap.js"));
  }
  assert.ok(setup.includes("./setup.js"));

  for (const css of [popupCss, fullpageCss, setupCss]) {
    assert.ok(css.includes(':root[data-theme="light"]'));
    assert.ok(css.includes(':root[data-theme="dark"]'));
    assert.ok(css.includes(":focus-visible"));
  }
  for (const css of [popupCss, fullpageCss]) {
    assert.ok(css.includes("prefers-reduced-motion"));
  }
}

console.log("EXT-11/EXT-13/OBS-02/EXT-14 MVP closure tests passed");
