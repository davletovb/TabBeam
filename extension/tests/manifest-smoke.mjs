import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import { ASK_PORT_NAME } from "../src/shared/ask-port.js";
import { MAX_HISTORY_BYTES, MAX_HISTORY_MESSAGES } from "../src/shared/limits.js";
import { PROVIDER_STATUS_MESSAGE } from "../src/shared/provider-status.js";
import { DIAGNOSTICS_MESSAGE } from "../src/shared/diagnostics.js";
import { MockPort } from "./support/mock-port.mjs";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const manifestPath = path.join(root, "manifest.json");
const contract = JSON.parse(fs.readFileSync(path.join(root, "../docs/protocol/native-messaging-v1.json"), "utf8"));
assert.equal(contract.max_history_messages, MAX_HISTORY_MESSAGES);
assert.equal(contract.max_history_bytes, MAX_HISTORY_BYTES);
/** @type {any} */
const manifest = JSON.parse(fs.readFileSync(manifestPath, "utf8"));

assert.equal(manifest.manifest_version, 3);
assert.equal(manifest.action.default_popup, "src/popup/index.html");
assert.equal(manifest.background.service_worker, "src/background/service-worker.js");
assert.equal(manifest.background.type, "module");
assert.ok(manifest.permissions.includes("contextMenus"));
assert.ok(manifest.permissions.includes("nativeMessaging"));
assert.ok(manifest.permissions.includes("storage"));
assert.ok(manifest.permissions.includes("activeTab"));
assert.ok(manifest.commands._execute_action?.suggested_key);
assert.equal(manifest.commands["open-pervue-full-page"], undefined);
const popupMarkup = fs.readFileSync(path.join(root, manifest.action.default_popup), "utf8");
for (const id of ["context-none", "context-selection", "context-page", "context-preview"]) {
  assert.ok(popupMarkup.includes(`id="${id}"`), `missing popup context control: ${id}`);
}

const contentScript = manifest.content_scripts[0];
assert.deepEqual(contentScript.matches, ["http://*/*", "https://*/*"]);
assert.ok(!contentScript.matches.includes("<all_urls>"));

const referencedFiles = [
  manifest.action.default_popup,
  manifest.background.service_worker,
  ...contentScript.js,
  "src/background/native-connection.js",
  "src/background/ask-bridge.js",
  "src/background/selection-capture.js",
  "src/background/entry-actions.js",
  "src/shared/ask-port.js",
  "src/shared/limits.js",
  "src/shared/theme.js",
  "src/shared/diagnostics.js",
  "src/shared/performance.js",
  "src/background/diagnostics.js",
  "src/popup/diagnostics.js",
  "src/popup/context-controls.js",
  "src/popup/menu-preload.js",
  "src/popup/ask-form.js",
  "src/fullpage/index.html",
  "src/popup/popup.js",
  "src/popup/popup.css",
  "src/fullpage/fullpage.js",
  "src/fullpage/fullpage.css",
  "src/setup/index.html",
  "src/setup/setup.css"
];

for (const file of referencedFiles) {
  assert.ok(fs.existsSync(path.join(root, file)), `missing extension file: ${file}`);
}

/** @type {any} */
const listeners = {
  installed: [],
  connects: [],
  messages: [],
  commands: [],
  contextMenuClicks: []
};

let nativeConnectCalls = 0;
/** @type {any[]} */
const menuItems = [];

globalThis.chrome = /** @type {any} */ ({
  runtime: {
    onInstalled: {
      addListener: (/** @type {any} */ fn) => listeners.installed.push(fn)
    },
    onConnect: {
      addListener: (/** @type {any} */ fn) => listeners.connects.push(fn)
    },
    onMessage: {
      addListener: (/** @type {any} */ fn) => listeners.messages.push(fn)
    },
    getURL: (/** @type {string} */ relative) =>
      `chrome-extension://test/${relative}`,
    getManifest: () => manifest,
    connectNative: () => {
      nativeConnectCalls += 1;
      throw new Error("the smoke test has no native host");
    }
  },
  commands: {
    onCommand: {
      addListener: (/** @type {any} */ fn) => listeners.commands.push(fn)
    }
  },
  contextMenus: {
    onClicked: {
      addListener: (/** @type {any} */ fn) =>
        listeners.contextMenuClicks.push(fn)
    },
    removeAll: async () => {},
    create: (/** @type {any} */ properties) => { menuItems.push(properties); return properties.id; }
  },
  action: { openPopup: async () => {} },
  storage: { local: { async get() { return {}; }, async set() {} } },
  tabs: {
    create: async () => ({ id: 1 })
  }
});

await import(
  `${pathToFileURL(path.join(root, manifest.background.service_worker)).href}?smoke=1`
);

assert.equal(nativeConnectCalls, 0);
assert.equal(listeners.installed.length, 1);
assert.equal(listeners.connects.length, 1);
assert.equal(listeners.messages.length, 1);
assert.equal(listeners.commands.length, 0, "the reserved action command opens the popup in Chrome");
assert.equal(listeners.contextMenuClicks.length, 1);
await listeners.installed[0]();
assert.deepEqual(menuItems.map(({ id, contexts }) => ({ id, contexts })), [
  { id: "pervue-use-selection", contexts: ["selection"] },
  { id: "pervue-use-page", contexts: ["page"] }
]);

/** @type {any} */
let healthResponse;
listeners.messages[0](
  { type: "pervue.health" },
  {},
  (/** @type {any} */ response) => {
    healthResponse = response;
  }
);

assert.deepEqual(healthResponse, {
  ok: true,
  surface: "background",
  version: manifest.version
});

// Ask ports: a content script's port is refused, other port names are left
// to their own listeners, and a popup's question reaches connectNative.
const popupUrl = "chrome-extension://test/src/popup/index.html";
const [onConnect] = listeners.connects;

const contentScriptPort = new MockPort(ASK_PORT_NAME, { url: "https://example.com/" });
onConnect(contentScriptPort);
assert.equal(contentScriptPort.disconnectCalls, 1);

const otherPort = new MockPort("pervue.other", { url: popupUrl });
onConnect(otherPort);
assert.equal(otherPort.disconnectCalls, 0);
assert.equal(otherPort.onMessage.listeners.size, 0);

const popupPort = new MockPort(ASK_PORT_NAME, { url: popupUrl });
onConnect(popupPort);
assert.equal(nativeConnectCalls, 0);
popupPort.emitMessage({ type: "ask", text: "Hello" });
assert.equal(nativeConnectCalls, 1);
assert.equal(popupPort.messages.length, 1);
assert.equal(popupPort.messages[0].event, "response.failed");
assert.equal(popupPort.messages[0].payload.error.reason, "HOST_START_FAILED");
assert.equal(popupPort.disconnectCalls, 1);

// Provider status: only the extension's own pages may ask, and the answer
// comes asynchronously, here the failure of a host that can't start.
const [onMessage] = listeners.messages;
/** @type {any[]} */
const statusResponses = [];
assert.equal(
  onMessage({ type: PROVIDER_STATUS_MESSAGE }, { url: "https://example.com/" }, (/** @type {any} */ response) =>
    statusResponses.push(response)
  ),
  undefined
);
assert.equal(nativeConnectCalls, 1);
assert.equal(
  onMessage({ type: PROVIDER_STATUS_MESSAGE }, { url: popupUrl }, (/** @type {any} */ response) =>
    statusResponses.push(response)
  ),
  true
);
await new Promise((resolve) => setTimeout(resolve, 0));
assert.equal(nativeConnectCalls, 2);
assert.equal(statusResponses.length, 1);
assert.equal(statusResponses[0].provider_id, "codex");
assert.equal(statusResponses[0].error.reason, "HOST_START_FAILED");

// OBS-02: diagnostics are extension-page-only and expose only sanitized
// versions/provider state/recent normalized failure.
/** @type {any} */
let diagnosticsResponse = null;
assert.equal(
  onMessage({ type: DIAGNOSTICS_MESSAGE }, { url: popupUrl }, (/** @type {any} */ response) => {
    diagnosticsResponse = response;
  }),
  undefined
);
assert.equal(diagnosticsResponse.extension_version, manifest.version);
assert.equal(diagnosticsResponse.protocol_version, 1);
assert.equal(diagnosticsResponse.host.state, "unavailable");
assert.equal(diagnosticsResponse.provider.provider_id, "codex");
assert.equal(diagnosticsResponse.recent_failure.reason, "HOST_START_FAILED");
assert.equal(
  onMessage({ type: DIAGNOSTICS_MESSAGE }, { url: "https://example.com/" }, () => {}),
  undefined
);

console.log("EXT-01/EXT-02/EXT-03/EXT-04/OBS-02 manifest and service-worker smoke checks passed");
