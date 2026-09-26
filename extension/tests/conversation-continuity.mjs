import assert from "node:assert/strict";
import { bindAskForm } from "../src/popup/ask-form.js";
import { serveConversationAskPort } from "../src/background/conversation-bridge.js";
import { createConversationStore } from "../src/background/conversation-store.js";
import { dialogueHistory } from "../src/shared/conversation-model.js";
import { ASK_PORT_NAME } from "../src/shared/ask-port.js";
import { MAX_HISTORY_BYTES, MAX_HISTORY_MESSAGES, utf8ByteLength } from "../src/shared/limits.js";
import { MockPort } from "./support/mock-port.mjs";

const ownerDocument = { createElement: () => new Element() };

class Element {
  constructor() {
    /** @type {Map<string, ((event: any) => void)[]>} */
    this.listeners = new Map();
    /** @type {Element[]} */
    this.children = [];
    this.attributes = new Map();
    this.ownerDocument = ownerDocument;
    this.textContent = "";
    this.value = "";
    this.hidden = false;
    this.focused = false;
    this.className = "";
  }
  /** @param {string} name @param {(event: any) => void} listener */
  addEventListener(name, listener) {
    this.listeners.set(name, [...this.listeners.get(name) ?? [], listener]);
  }
  /** @param {string} name */
  fire(name) {
    for (const listener of this.listeners.get(name) ?? []) listener({ preventDefault() {}, key: name });
  }
  /** @param {string} name @param {string} value */
  setAttribute(name, value) { this.attributes.set(name, String(value)); }
  /** @param {string} name */
  removeAttribute(name) { this.attributes.delete(name); }
  /** @param {...(string | Element)} children */
  append(...children) {
    for (const child of children) {
      if (typeof child === "string") this.textContent += child;
      else this.children.push(child);
    }
  }
  /** @param {...Element} children */
  replaceChildren(...children) { this.children = children; this.textContent = ""; }
  focus() { this.focused = true; }
  requestSubmit() { this.fire("submit"); }
}

let nextUuid = 0;
const uuid = () => `00000000-0000-4000-8000-${String(++nextUuid).padStart(12, "0")}`;
/** @type {Record<string, any>} */
let saved = {};
/** @type {((changes: any, area: string) => void)[]} */
const storageListeners = [];
const storage = {
  async get() { return structuredClone(saved); },
  /** @param {any} update */
  async set(update) {
    saved = { ...saved, ...structuredClone(update) };
    for (const listener of storageListeners) {
      listener(Object.fromEntries(Object.keys(update).map((key) => [key, { newValue: update[key] }])), "local");
    }
  },
  /** @param {string} key */
  async remove(key) { delete saved[key]; }
};
let store = createConversationStore(storage, uuid);
let inFlight = new Set();
/** @type {{request: any, owner: any}[]} */
const native = [];
const manager = {
  /** @param {any} request @param {any} owner */
  send(request, owner) { native.push({ request, owner }); }
};

/** The popup and full page use the same controller and worker request path. */
/** @param {string | (() => string)} [providerId] @param {any} [extra] more ask-form options */
function openView(providerId = "codex", extra = {}) {
  const elements = {
    form: new Element(), input: new Element(), submit: new Element(),
    status: new Element(), answer: new Element(), history: new Element(),
    cancel: new Element(), retry: new Element()
  };
  /** @type {MockPort[]} */
  const ports = [];
  const runtime = {
    /** @param {{name: string}} info */
    connect(info) {
      assert.equal(info.name, ASK_PORT_NAME);
      const ui = new MockPort(info.name);
      const worker = new MockPort(info.name);
      const uiPost = ui.postMessage.bind(ui);
      const workerPost = worker.postMessage.bind(worker);
      const uiDisconnect = ui.disconnect.bind(ui);
      const workerDisconnect = worker.disconnect.bind(worker);
      ui.postMessage = (message) => { uiPost(message); worker.emitMessage(message); };
      worker.postMessage = (message) => { workerPost(message); ui.emitMessage(message); };
      ui.disconnect = () => { uiDisconnect(); worker.emitDisconnect(); };
      worker.disconnect = () => { workerDisconnect(); ui.emitDisconnect(); };
      serveConversationAskPort(worker, { manager, store, inFlight, createRequestId: () => `req_${native.length + 1}` });
      ports.push(ui);
      return ui;
    },
    /** @param {any} message */
    async sendMessage(message) {
      if (message.type === "pervue.conversations.list") return { ok: true, value: await store.list() };
      if (message.type === "pervue.conversations.get") return { ok: true, value: await store.get(message.conversation_id) };
      throw new Error("unexpected worker message");
    }
  };
  const view = bindAskForm(/** @type {any} */ (elements), runtime, undefined, {
    getProviderId: () => typeof providerId === "function" ? providerId() : providerId,
    storageChanges: { addListener(listener) { storageListeners.push(listener); } },
    ...extra
  });
  return {
    view, elements, ports,
    /** @param {string} question */
    ask(question) { elements.input.value = question; elements.form.fire("submit"); }
  };
}

/** @param {number} index @param {string} answer @param {string} [session] @param {string} [provider] */
function answerRequest(index, answer, session, provider = "codex") {
  const { request, owner } = native[index];
  /** @param {string} name @param {any} payload */
  const event = (name, payload) => owner.onEvent({
    version: 1, type: "event", request_id: request.request_id, event: name, payload
  });
  if (session) event("conversation.created", { conversation_id: session });
  event("response.started", { provider_id: provider, conversation_id: session ?? request.payload.conversation_id });
  event("response.delta", { text: answer.slice(0, 5) });
  event("response.delta", { text: answer.slice(5) });
  event("response.completed", {});
}

async function settle() {
  await new Promise((resolve) => setTimeout(resolve, 0));
  await new Promise((resolve) => setTimeout(resolve, 0));
}

const popup = openView();
popup.ask("First question");
assert.equal(native.length, 1);
assert.equal(native[0].request.payload.provider_id, "codex");
assert.deepEqual(native[0].request.payload.input, { text: "First question" });
answerRequest(0, "First answer", "host_session_1");
await settle();
const id = /** @type {string} */ (popup.view.getConversationId());
assert.ok(id?.startsWith("conv_"));
assert.deepEqual(popup.elements.history.children.map((item) => item.children[1].textContent),
  ["First question", "First answer"]);
assert.equal((await store.get(id)).provider_session_id, undefined, "the page never sees native metadata");
assert.equal((await store.getPrivate(id)).provider_session_id, "host_session_1");

popup.ask("Follow up in popup");
await settle();
assert.equal(native.length, 2);
assert.equal(native[1].request.payload.conversation_id, "host_session_1");
assert.deepEqual(native[1].request.payload.input.history, [
  { role: "user", text: "First question" },
  { role: "assistant", text: "First answer" }
]);
answerRequest(1, "Second answer");
await settle();
assert.equal(popup.view.getConversationId(), id);
assert.deepEqual(popup.elements.history.children.map((item) => item.children[1].textContent),
  ["First question", "First answer", "Follow up in popup", "Second answer"]);

// Closing the popup and restarting the worker keeps the recent index and ID.
store = createConversationStore(storage, uuid);
inFlight = new Set();
const reopened = openView();
assert.equal((await store.list())[0].id, id);
assert.equal(await reopened.view.loadConversation(id), true);
assert.equal(reopened.view.getConversationId(), id);
assert.equal(reopened.elements.history.children.length, 4);

// The popup handoff URL carries only the stable local ID. Full view loads it
// and continues through the exact same request protocol and worker bridge.
const fullPageUrl = new URL(`chrome-extension://test/src/fullpage/index.html?conversation=${id}`);
assert.equal(fullPageUrl.searchParams.get("conversation"), id);
assert.equal(fullPageUrl.href.includes("host_session_1"), false);
const fullPage = openView();
assert.equal(await fullPage.view.loadConversation(fullPageUrl.searchParams.get("conversation") ?? ""), true);
fullPage.ask("Follow up in full view");
await settle();
assert.equal(native.length, 3);
assert.equal(native[2].request.payload.conversation_id, "host_session_1");
assert.deepEqual(native[2].request.payload.input.history.map((/** @type {any} */ message) => message.text),
  ["First question", "First answer", "Follow up in popup", "Second answer"]);
answerRequest(2, "Third answer");
await settle();
const complete = await store.get(id);
assert.deepEqual(complete.messages.map((/** @type {any} */ message) => message.text), [
  "First question", "First answer", "Follow up in popup", "Second answer",
  "Follow up in full view", "Third answer"
]);
assert.equal(fullPage.view.getConversationId(), id);
assert.deepEqual(dialogueHistory(complete).map((message) => message.text), complete.messages.map((/** @type {any} */ message) => message.text));
assert.equal(fullPage.elements.history.children.length, 6);

// A second surface cannot submit concurrently into the same conversation.
reopened.ask("One more");
await settle();
assert.equal(native.length, 4);
const handoff = openView();
assert.equal(await handoff.view.loadConversation(id), true);
assert.equal(handoff.elements.history.children.at(-1)?.attributes.get("data-state"), "pending");
fullPage.ask("Conflicting follow up");
await settle();
assert.equal(native.length, 4);
assert.equal(fullPage.elements.status.textContent.includes("another question"), true);
answerRequest(3, "Fourth answer");
await settle();
assert.equal(handoff.elements.history.children.at(-1)?.children[1].textContent, "Fourth answer");
assert.equal(handoff.elements.history.children.at(-1)?.attributes.get("data-state"), "complete");

// A first request can fail before a native conversation exists. A retry must
// not leave phantom pending prompts in the transcript.
const unavailable = openView();
unavailable.ask("First attempt");
await settle();
native[4].owner.onEvent({ version: 1, type: "event", request_id: native[4].request.request_id,
  event: "response.failed", payload: { error: { code: "PROVIDER_NOT_FOUND", reason: "EXECUTABLE_NOT_FOUND",
    message: "Install Codex.", retryable: false } } });
await settle();
assert.equal(unavailable.elements.history.children.length, 0);
assert.equal(unavailable.elements.status.textContent, "Install Codex.");
unavailable.ask("Retry");
await settle();
assert.equal(unavailable.elements.history.children.length, 1);

// The input remains editable while answering; completion must preserve the
// next prompt the user started typing.
unavailable.elements.input.value = "Draft follow up";
answerRequest(5, "Working now", "host_session_2");
await settle();
assert.equal(unavailable.elements.input.value, "Draft follow up");

// Retrying a failed follow-up reuses the failed pair instead of appending a
// duplicate "You" turn to persisted or rendered history.
const retryView = openView();
assert.equal(await retryView.view.loadConversation(id), true);
retryView.ask("Retry this turn");
await settle();
const failedIndex = native.length - 1;
native[failedIndex].owner.onEvent({
  version: 1, type: "event",
  request_id: native[failedIndex].request.request_id,
  event: "response.failed",
  payload: { error: {
    code: "PROVIDER_FAILED", reason: "PROCESS_EXITED",
    message: "Provider stopped.", retryable: true
  } }
});
await settle();
const failedRecord = await store.getPrivate(id);
const failedLength = failedRecord.messages.length;
assert.equal(failedRecord.messages.at(-2).text, "Retry this turn");
assert.equal(failedRecord.messages.at(-1).status, "failed");
retryView.elements.retry.fire("click");
await settle();
const retryIndex = native.length - 1;
answerRequest(retryIndex, "Retried answer");
await settle();
const retriedRecord = await store.getPrivate(id);
assert.equal(retriedRecord.messages.length, failedLength);
assert.equal(
  retriedRecord.messages.filter((/** @type {any} */ message) => message.role === "user" && message.text === "Retry this turn").length,
  1
);
assert.equal(retriedRecord.messages.at(-1).text, "Retried answer");

// Retry state belongs to one conversation only. Switching threads clears the
// old failed question so it cannot be posted into the newly selected thread.
retryView.ask("Do not cross threads");
await settle();
const crossThreadIndex = native.length - 1;
native[crossThreadIndex].owner.onEvent({
  version: 1, type: "event",
  request_id: native[crossThreadIndex].request.request_id,
  event: "response.failed",
  payload: { error: {
    code: "PROVIDER_FAILED", reason: "PROCESS_EXITED",
    message: "Provider stopped.", retryable: true
  } }
});
await settle();
assert.equal(retryView.elements.retry.hidden, false);
const other = await store.create({
  providerId: "codex",
  providerSessionId: "other_session",
  text: "Other thread"
});
assert.equal(await retryView.view.loadConversation(other.id), true);
assert.equal(retryView.elements.retry.hidden, true);
const beforeStaleRetry = native.length;
retryView.elements.retry.fire("click");
await settle();
assert.equal(native.length, beforeStaleRetry);

// Eviction between viewing and asking must preserve the actionable NOT_FOUND
// failure instead of replacing it with a failed history reload.
for (let index = 0; index < 32; index += 1) {
  await store.create({ providerId: "codex", providerSessionId: "another", text: `Other ${index}` });
}
assert.equal((await store.list()).some((entry) => entry.id === id), false);
fullPage.ask("Evicted follow-up");
await settle();
assert.equal(fullPage.elements.status.textContent, "Conversation not found. Start a new one.");

// Unknown schema versions are refused without silently replacing the data.
const original = structuredClone(saved);
saved["pervue.conversations"].schema_version = 3;
let rejected = false;
try {
  await store.list();
} catch (error) {
  rejected = /compatible Pervue version/.test(/** @type {Error} */ (error).message);
}
assert.equal(rejected, true, "a future schema version must not be discarded");
assert.equal(saved["pervue.conversations"].schema_version, 3);
saved = original;

const longConversation = { messages: Array.from({ length: 40 }, (_, i) => [
  { role: "user", text: `question ${i}` },
  { role: "assistant", text: `answer ${i}`, status: "complete" }
]).flat() };
const recentHistory = dialogueHistory(longConversation);
assert.equal(recentHistory.length, MAX_HISTORY_MESSAGES);
assert.equal(recentHistory[0].text, "question 24");
assert.equal(recentHistory.at(-1)?.text, "answer 39");
longConversation.messages.push(
  { role: "user", text: "Large answer?" },
  { role: "assistant", text: "x".repeat(MAX_HISTORY_BYTES + 2000), status: "complete" }
);
const boundedHistory = dialogueHistory(longConversation);
assert.equal(boundedHistory.length, 2);
assert.ok(boundedHistory.at(-1)?.text.length > 0);
assert.ok(boundedHistory.reduce((sum, message) => sum + utf8ByteLength(message.text), 0) <= MAX_HISTORY_BYTES);
longConversation.messages.push({ role: "user", text: "pending" },
  { role: "assistant", text: "incomplete", status: "pending" });
assert.deepEqual(dialogueHistory(longConversation), boundedHistory);

// A new conversation carries the capability-selected provider into native
// request routing and persists that provider with the conversation.
const claudeView = openView("claude");
claudeView.ask("Claude question");
await settle();
const claudeIndex = native.length - 1;
assert.equal(native[claudeIndex].request.payload.provider_id, "claude");
answerRequest(claudeIndex, "Claude answer", "claude_session_1", "claude");
await settle();
const claudeId = /** @type {string} */ (claudeView.view.getConversationId());
assert.equal((await store.getPrivate(claudeId)).provider_id, "claude");

// A new conversation reports the provider its question was sent to, even if
// the shown choice changed while it was answering: that is what it locks to.
let shownProvider = "claude";
/** @type {[string | null, string | undefined][]} */
const created = [];
const switched = openView(() => shownProvider, {
  /** @param {string | null} conversationId @param {string} [provider] */
  onConversationId: (conversationId, provider) => created.push([conversationId, provider])
});
switched.ask("Asked with Claude");
await settle();
shownProvider = "codex";
const switchedIndex = native.length - 1;
assert.equal(native[switchedIndex].request.payload.provider_id, "claude");
answerRequest(switchedIndex, "Claude answer", "claude_session_2", "claude");
await settle();
// The first report is the creation; a later reload reports the ID alone.
assert.deepEqual(created[0], [switched.view.getConversationId(), "claude"]);

console.log("Conversation continuity tests passed");
