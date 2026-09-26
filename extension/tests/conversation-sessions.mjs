import assert from "node:assert/strict";
import {
  CONVERSATIONS_DELETE_MESSAGE,
  CONVERSATIONS_LIST_MESSAGE,
  answerConversationMessage
} from "../src/background/conversation-messages.js";
import { createConversationStore } from "../src/background/conversation-store.js";
import { DELETED_STATUS, bindAskForm } from "../src/popup/ask-form.js";
import { RESUME_WINDOW_MS, TAB_SESSIONS_KEY, conversationToResume, pageKey, rememberConversation } from "../src/popup/session.js";
import { CONFIRM_MS, bindConversationList, dayGroup, shortWhen } from "../src/shared/conversation-list.js";

/** Whether `promise` rejects. @param {Promise<any>} promise */
async function rejects(promise) {
  try {
    await promise;
    return false;
  } catch {
    return true;
  }
}

const ID_A = "conv_00000000-0000-4000-8000-00000000000a";
const ID_B = "conv_00000000-0000-4000-8000-00000000000b";
const ID_C = "conv_00000000-0000-4000-8000-00000000000c";

// ---------- Store: deleting a conversation ----------
{
  /** @type {Record<string, any>} */
  const saved = {};
  const storage = {
    async get() { return structuredClone(saved); },
    /** @param {any} values */
    async set(values) { Object.assign(saved, structuredClone(values)); },
    /** @param {string} key */
    async remove(key) { delete saved[key]; }
  };
  let n = 0;
  const store = createConversationStore(storage, () => `00000000-0000-4000-8000-${String(++n).padStart(12, "0")}`);
  const first = await store.create({ providerId: "fake", providerSessionId: "s1", text: "First" });
  const second = await store.create({ providerId: "fake", providerSessionId: "s2", text: "Second" });
  assert.deepEqual((await store.list()).map((item) => item.title), ["Second", "First"]);

  await store.remove(first.id);
  assert.deepEqual((await store.list()).map((item) => item.id), [second.id]);
  assert.equal(saved[`pervue.conversation.${first.id}`], undefined, "the record itself is gone");
  assert.ok(await rejects(store.get(first.id)));
  assert.ok(await rejects(store.remove(first.id)), "deleting twice fails");
  assert.ok(await rejects(store.remove("not-an-id")));

  // The worker's answer: a running conversation can't be deleted.
  const inFlight = new Set([second.id]);
  const busy = await answerConversationMessage({ type: CONVERSATIONS_DELETE_MESSAGE, conversation_id: second.id }, store, inFlight);
  assert.equal(busy.ok, false);
  assert.ok(/Stop or finish/.test(/** @type {any} */ (busy).error));
  assert.equal((await store.list()).length, 1);
  inFlight.clear();
  assert.deepEqual(await answerConversationMessage({ type: CONVERSATIONS_DELETE_MESSAGE, conversation_id: second.id }, store, inFlight), { ok: true });
  assert.deepEqual(await answerConversationMessage({ type: CONVERSATIONS_LIST_MESSAGE }, store, inFlight), { ok: true, value: [] });
  const missing = await answerConversationMessage({ type: CONVERSATIONS_DELETE_MESSAGE, conversation_id: second.id }, store, inFlight);
  assert.deepEqual(missing, { ok: false, error: "Couldn't delete the conversation." });
}

// ---------- Which conversation a new popup opens ----------
{
  /** @type {Record<string, any>} */
  let saved = {};
  const session = {
    async get() { return structuredClone(saved); },
    /** @param {any} values */
    async set(values) { saved = { ...saved, ...structuredClone(values) }; }
  };
  const now = 1_000_000_000;
  const article = { id: 7, url: "https://example.com/article?ref=x#part" };
  const exists = [{ id: ID_A }];

  assert.equal(pageKey(article.url), "https://example.com/article", "query and fragment are dropped");
  assert.equal(pageKey("chrome://extensions"), null);

  // Nothing remembered: start fresh.
  assert.equal(await conversationToResume(session, article, exists, now), null);

  await rememberConversation(session, article, ID_A, now);
  assert.deepEqual(saved[TAB_SESSIONS_KEY], { 7: { conversation_id: ID_A, page: "https://example.com/article", at: now } });

  // Reopened over the same tab and page soon after: continue.
  assert.equal(await conversationToResume(session, { ...article, url: "https://example.com/article#other" }, exists, now + 60_000), ID_A);
  // Another page in that tab, another tab, or much later: start fresh.
  assert.equal(await conversationToResume(session, { id: 7, url: "https://example.com/other" }, exists, now + 60_000), null);
  assert.equal(await conversationToResume(session, { id: 8, url: article.url }, exists, now + 60_000), null);
  assert.equal(await conversationToResume(session, article, exists, now + RESUME_WINDOW_MS + 1), null);
  // A conversation deleted since: start fresh.
  assert.equal(await conversationToResume(session, article, [], now + 60_000), null);
  // No tab to go by: start fresh.
  assert.equal(await conversationToResume(session, null, exists, now), null);

  // A new conversation forgets the tab; stale entries are dropped.
  await rememberConversation(session, { id: 9, url: "https://example.org/" }, ID_B, now - RESUME_WINDOW_MS - 1);
  await rememberConversation(session, article, null, now);
  assert.deepEqual(saved[TAB_SESSIONS_KEY], {});

  // Without a web page to match, nothing resumes: not chrome:// pages, and
  // not a tab whose URL Chrome doesn't share.
  const settings = { id: 11, url: "chrome://settings" };
  await rememberConversation(session, settings, ID_A, now);
  assert.equal(await conversationToResume(session, { id: 11, url: "chrome://extensions" }, exists, now + 1_000), null);
  assert.equal(await conversationToResume(session, settings, exists, now + 1_000), null);
  await rememberConversation(session, { id: 12 }, ID_A, now);
  assert.equal(await conversationToResume(session, { id: 12 }, exists, now + 1_000), null);

  // At most 50 tabs are remembered; the least recently used go first.
  saved = {};
  for (let tabId = 1; tabId <= 51; tabId += 1) {
    await rememberConversation(session, { id: tabId, url: `https://example.com/${tabId}` }, ID_A, now + tabId);
  }
  const kept = Object.keys(saved[TAB_SESSIONS_KEY]);
  assert.equal(kept.length, 50);
  assert.ok(!kept.includes("1"), "the oldest tab is dropped");
  assert.ok(kept.includes("51"));

  // Storage failures never break the popup.
  const broken = { async get() { throw new Error("gone"); }, async set() { throw new Error("gone"); } };
  assert.equal(await conversationToResume(broken, article, exists, now), null);
  await rememberConversation(broken, article, ID_A, now);
}

// ---------- A conversation deleted while it's open ----------
{
  class Field {
    constructor() {
      this.textContent = "";
      this.className = "";
      this.value = "";
      this.hidden = false;
      /** @type {any[]} */
      this.children = [];
      /** @type {Map<string, string>} */
      this.attributes = new Map();
      /** @type {Map<string, ((event: any) => void)[]>} */
      this.listeners = new Map();
      this.ownerDocument = { activeElement: null, createElement: () => new Field() };
    }
    /** @param {string} type @param {(event: any) => void} listener */
    addEventListener(type, listener) { this.listeners.set(type, [...this.listeners.get(type) ?? [], listener]); }
    /** @param {string} name @param {string} value */
    setAttribute(name, value) { this.attributes.set(name, String(value)); }
    /** @param {string} name */
    getAttribute(name) { return this.attributes.get(name) ?? null; }
    /** @param {string} name */
    removeAttribute(name) { this.attributes.delete(name); }
    /** @param {...any} nodes */
    append(...nodes) { this.children.push(...nodes); }
    /** @param {...any} nodes */
    replaceChildren(...nodes) { this.children = nodes; }
    focus() {}
  }
  const conversation = {
    id: ID_A,
    messages: [
      { role: "user", text: "Question", status: "complete" },
      { role: "assistant", text: "Answer", status: "complete" }
    ]
  };
  let loads = 0;
  /** @type {((changes: any, area: string) => void)[]} */
  const listeners = [];
  /** @type {(string | null)[]} */
  const ids = [];
  const elements = {
    form: new Field(), input: new Field(), submit: new Field(),
    status: new Field(), answer: new Field(), history: new Field()
  };
  const view = bindAskForm(/** @type {any} */ (elements), /** @type {any} */ ({
    connect() { throw new Error("not used"); },
    async sendMessage() {
      loads += 1;
      return { ok: true, value: conversation };
    }
  }), undefined, {
    onConversationId: (id) => ids.push(id),
    storageChanges: { addListener: (listener) => { listeners.push(listener); } }
  });
  /** @param {any} change */
  const changed = (change) => {
    for (const listener of listeners) listener({ [`pervue.conversation.${ID_A}`]: change }, "local");
  };
  assert.equal(await view.loadConversation(ID_A), true);
  assert.equal(elements.history.children.length, 2);

  // Saved again elsewhere: reload it.
  changed({ oldValue: {}, newValue: {} });
  await new Promise((resolve) => setTimeout(resolve, 0));
  assert.equal(loads, 2);
  assert.equal(view.getConversationId(), ID_A);

  // Deleted elsewhere: leave it, and say why.
  changed({ oldValue: {} });
  assert.equal(view.getConversationId(), null);
  assert.equal(elements.history.children.length, 0);
  assert.equal(elements.status.textContent, DELETED_STATUS);
  assert.equal(elements.status.getAttribute("data-state"), "notice");
  assert.deepEqual(ids.slice(-1), [null], "the page forgets it too");
  assert.equal(loads, 2, "no reload of a conversation that's gone");
}

// ---------- Grouping and times ----------
{
  const now = new Date(2026, 8, 26, 15, 0).getTime();
  /** @param {number} y @param {number} m @param {number} d @param {number} [h] */
  const at = (y, m, d, h = 12) => new Date(y, m, d, h).toISOString();
  assert.equal(dayGroup(at(2026, 8, 26, 1), now), "Today");
  assert.equal(dayGroup(at(2026, 8, 25, 23), now), "Yesterday");
  assert.equal(dayGroup(at(2026, 8, 21), now), "Previous 7 days");
  assert.equal(dayGroup(at(2026, 8, 5), now), "Previous 30 days");
  assert.equal(dayGroup(at(2026, 5, 1), now), "Older");
  assert.equal(dayGroup(undefined, now), "Older");
  assert.equal(shortWhen(new Date(now - 20_000).toISOString(), now), "now");
  assert.equal(shortWhen(new Date(now - 5 * 60_000).toISOString(), now), "5m");
  assert.equal(shortWhen(new Date(now - 3 * 3_600_000).toISOString(), now), "3h");
  assert.equal(shortWhen("garbage", now), "");
}

// ---------- The conversation list ----------

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
    this.type = "";
    this.title = "";
    this.hidden = false;
    this.ownerDocument = doc;
  }
  /** @returns {string} */
  get textContent() {
    return this.children.map((child) => typeof child === "string" ? child : child.textContent).join("");
  }
  /** @param {string} value */
  set textContent(value) { this.children = [value]; }
  /** @param {...any} nodes */
  append(...nodes) { this.children.push(...nodes); }
  /** @param {...any} nodes */
  replaceChildren(...nodes) { this.children = nodes; }
  /** @param {string} name @param {string} value */
  setAttribute(name, value) { this.attributes.set(name, value); }
  /** @param {string} name */
  getAttribute(name) { return this.attributes.get(name) ?? null; }
  /** @param {string} type @param {() => void} listener */
  addEventListener(type, listener) { this.listeners.set(type, [...this.listeners.get(type) ?? [], listener]); }
  click() { for (const listener of this.listeners.get("click") ?? []) listener(); }
  /** @returns {Node[]} */
  all() { return this.children.flatMap((child) => typeof child === "string" ? [] : [child, ...child.all()]); }
  /** @param {string} className */
  find(className) { return this.all().filter((node) => node.className.split(" ").includes(className)); }
}
const doc = {
  /** @param {string} tag */
  createElement: (tag) => new Node(tag),
  /** @param {string} _ns @param {string} tag */
  createElementNS: (_ns, tag) => new Node(tag)
};

{
  const now = new Date(2026, 8, 26, 15, 0).getTime();
  let items = [
    { id: ID_A, title: "Rust ownership", updated_at: new Date(now - 10 * 60_000).toISOString() },
    { id: ID_B, title: "Summarize WebGPU article", updated_at: new Date(now - 26 * 3_600_000).toISOString() },
    { id: ID_C, title: "Vendor email draft", updated_at: new Date(now - 26 * 3_600_000).toISOString() }
  ];
  /** @type {any[]} */
  const sent = [];
  let deleteReply = /** @type {any} */ ({ ok: true });
  const runtime = {
    /** @param {any} message */
    async sendMessage(message) {
      sent.push(message);
      if (message.type === CONVERSATIONS_LIST_MESSAGE) return { ok: true, value: items };
      if (message.type === CONVERSATIONS_DELETE_MESSAGE) {
        if (deleteReply.ok) items = items.filter((item) => item.id !== message.conversation_id);
        return deleteReply;
      }
      return { ok: false };
    }
  };
  let current = ID_A;
  let canSwitch = true;
  const view = {
    getConversationId: () => current,
    /** @param {string} id */
    async loadConversation(id) {
      if (!canSwitch) return false;
      current = id;
      return true;
    }
  };
  const list = new Node("div");
  const empty = new Node("p");
  /** @type {string[]} */
  const events = [];
  const conversations = bindConversationList(/** @type {any} */ ({ list, empty }), runtime, view, {
    deletable: true,
    now: () => now,
    onOpen: (id) => events.push(`open ${id}`),
    onBlocked: () => events.push("blocked"),
    onDeleted: (id, wasActive) => events.push(`deleted ${id} ${wasActive}`),
    onError: (message) => events.push(`error ${message}`)
  });
  await conversations.refresh();

  const labels = () => list.find("conversation-group-label").map((node) => node.textContent);
  const names = () => list.find("conversation-name").map((node) => node.textContent);
  const opens = () => list.find("conversation-open");
  const current_ = () => opens().filter((node) => node.getAttribute("aria-current") === "true").map((node) => node.title);
  assert.deepEqual(labels(), ["Today", "Yesterday"]);
  assert.deepEqual(names(), ["Rust ownership", "Summarize WebGPU article", "Vendor email draft"]);
  assert.deepEqual(list.find("conversation-when").map((node) => node.textContent)[0], "10m");
  assert.deepEqual(current_(), ["Rust ownership"]);
  assert.equal(empty.hidden, true);

  // Search matches every word, in any order.
  conversations.setQuery("article webgpu");
  assert.deepEqual(names(), ["Summarize WebGPU article"]);
  conversations.setQuery("nothing like this");
  assert.deepEqual(names(), []);
  assert.equal(empty.hidden, false);
  assert.equal(empty.textContent, "No conversations match.");
  conversations.setQuery("");

  // Choosing one loads it and marks it.
  opens()[1].click();
  await new Promise((resolve) => setTimeout(resolve, 0));
  assert.equal(current, ID_B);
  assert.deepEqual(current_(), ["Summarize WebGPU article"]);
  assert.deepEqual(events, [`open ${ID_B}`]);

  // A view that can't switch (an answer is running) keeps the current one
  // marked and says why.
  canSwitch = false;
  opens()[2].click();
  await new Promise((resolve) => setTimeout(resolve, 0));
  assert.equal(current, ID_B);
  assert.deepEqual(current_(), ["Summarize WebGPU article"]);
  assert.equal(events.at(-1), "blocked");
  canSwitch = true;

  // Deleting takes a second click; the first only asks.
  const deleteOf = (/** @type {number} */ row) => list.find("conversation-delete")[row];
  deleteOf(2).click();
  await new Promise((resolve) => setTimeout(resolve, 0));
  assert.equal(sent.filter((message) => message.type === CONVERSATIONS_DELETE_MESSAGE).length, 0);
  assert.equal(deleteOf(2).getAttribute("data-confirm"), "true");
  assert.equal(deleteOf(2).textContent, "Delete?");
  deleteOf(2).click();
  await new Promise((resolve) => setTimeout(resolve, 0));
  assert.deepEqual(names(), ["Rust ownership", "Summarize WebGPU article"]);
  assert.equal(events.at(-1), `deleted ${ID_C} false`);

  // Deleting the open conversation says so, so the view can start fresh.
  deleteOf(1).click();
  deleteOf(1).click();
  await new Promise((resolve) => setTimeout(resolve, 0));
  assert.equal(events.at(-1), `deleted ${ID_B} true`);

  // An unconfirmed delete disarms by itself.
  deleteOf(0).click();
  assert.equal(deleteOf(0).getAttribute("data-confirm"), "true");
  await new Promise((resolve) => setTimeout(resolve, CONFIRM_MS + 50));
  assert.equal(deleteOf(0).getAttribute("data-confirm"), null);

  // A refused delete reports the worker's reason and keeps the row.
  deleteReply = { ok: false, error: "Stop or finish the answer in progress, then delete the conversation." };
  deleteOf(0).click();
  deleteOf(0).click();
  await new Promise((resolve) => setTimeout(resolve, 0));
  assert.deepEqual(names(), ["Rust ownership"]);
  assert.ok(/^error Stop or finish/.test(events.at(-1) ?? ""));
}

{
  // The compact recent list: a few, ungrouped, no delete.
  const items = Array.from({ length: 5 }, (_, i) => ({
    id: `conv_00000000-0000-4000-8000-00000000000${i}`,
    title: `Question ${i}`,
    updated_at: new Date().toISOString()
  }));
  const list = new Node("div");
  /** @type {any[]} */
  const changes = [];
  const recent = bindConversationList(/** @type {any} */ ({ list }), {
    async sendMessage() { return { ok: true, value: items }; }
  }, { getConversationId: () => null, async loadConversation() { return true; } }, {
    limit: 3,
    grouped: false,
    onChange: (all) => changes.push(all.length)
  });
  await recent.refresh();
  assert.equal(list.find("conversation-open").length, 3);
  assert.equal(list.find("conversation-group-label").length, 0);
  assert.equal(list.find("conversation-delete").length, 0);
  assert.deepEqual(changes, [5]);

  // An unavailable worker shows an empty list rather than failing.
  const failing = bindConversationList(/** @type {any} */ ({ list: new Node("div") }), {
    async sendMessage() { throw new Error("worker gone"); }
  }, { getConversationId: () => null, async loadConversation() { return true; } });
  assert.deepEqual(await failing.refresh(), []);
}

console.log("Conversation sessions, history list, and delete tests passed");
