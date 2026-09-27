/**
 * TST-14: web search and source grounding, from the host's events to what
 * the popup and the full view show.
 *
 * - Sources are checked and bounded before anything stores or shows them
 *   (SEC-05), and normalize the same way whichever way they arrive.
 * - The sources an answer keeps are exactly the ones its own turn retrieved:
 *   nothing from another turn, nothing malformed, nothing repeated.
 * - The popup and the full view show the same sources with the same
 *   numbers and identities.
 * - A search that fails says why, keeps no sources, and leaves the
 *   conversation usable.
 */
import assert from "node:assert/strict";
import { bindAskForm } from "../src/popup/ask-form.js";
import { serveConversationAskPort } from "../src/background/conversation-bridge.js";
import { createConversationStore } from "../src/background/conversation-store.js";
import { dialogueHistory } from "../src/shared/conversation-model.js";
import { ASK_PORT_NAME } from "../src/shared/ask-port.js";
import { bindSearchToggle } from "../src/shared/search-toggle.js";
import { renderSources } from "../src/shared/source-list.js";
import {
  MAX_SOURCES_PER_ANSWER, MAX_SOURCE_SNIPPET_LENGTH, MAX_SOURCE_TITLE_LENGTH,
  createSourceSet, normalizeSource, sourceFromEvent, sourceHost, sourceText, sourceUrl, storedSources
} from "../src/shared/sources.js";
import { MockPort } from "./support/mock-port.mjs";

// ---------- A small DOM ----------

const doc = {
  /** @param {string} tag */
  createElement: (tag) => new Element(tag)
};

class Element {
  /** @param {string} [tag] */
  constructor(tag = "div") {
    this.tagName = tag.toUpperCase();
    /** @type {Map<string, ((event: any) => void)[]>} */
    this.listeners = new Map();
    /** @type {Element[]} */
    this.children = [];
    /** @type {Map<string, string>} */
    this.attributes = new Map();
    this.ownerDocument = doc;
    this.textContent = "";
    this.value = "";
    this.hidden = false;
    this.disabled = false;
    this.title = "";
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
  click() { this.fire("click"); }
  /** @param {string} name @param {string} value */
  setAttribute(name, value) { this.attributes.set(name, String(value)); }
  /** @param {string} name */
  getAttribute(name) { return this.attributes.get(name) ?? null; }
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
  focus() {}
  requestSubmit() { this.fire("submit"); }
}

/**
 * @param {Element} container @param {unknown} list
 * @param {import("../src/shared/source-list.js").SourceListOptions} [options]
 */
function render(container, list, options) {
  return renderSources(/** @type {any} */ (container), list, options);
}

/** Every element under `root`, depth first. @param {Element} root @returns {Element[]} */
function descendants(root) {
  return root.children.flatMap((child) => [child, ...descendants(child)]);
}

/** @param {Element} root @param {string} className */
function byClass(root, className) {
  return descendants(root).filter((node) => node.className.split(" ").includes(className));
}

/** What a list of sources shows, in order. @param {Element} root */
function shown(root) {
  return descendants(root)
    .filter((node) => node.tagName === "A" && node.getAttribute("data-source-id"))
    .map((node) => ({
      id: node.getAttribute("data-source-id"),
      number: byClass(node, "source-number")[0]?.textContent,
      href: node.getAttribute("href"),
      target: node.getAttribute("target"),
      rel: node.getAttribute("rel")
    }));
}

// ---------- SEC-05: one check for every source ----------

for (const url of ["https://example.com/", "http://example.test/a?b=1#c", "https://www.rust-lang.org/learn"]) {
  assert.equal(typeof sourceUrl(url), "string", url);
}
for (const url of [
  "javascript:alert(1)", "data:text/html,<script>alert(1)</script>", "file:///etc/passwd",
  "chrome-extension://abc/src/popup/index.html", "/relative", "//example.com/x", "https://",
  "https://user:secret@example.com/", "https://user@example.com/", "https://example.com/has space",
  "https://example.com/\n", `https://example.com/${"a".repeat(4100)}`, 7, null, undefined
]) {
  assert.equal(sourceUrl(url), null, String(url));
}
assert.equal(sourceHost("https://www.example.com/a"), "example.com");

assert.equal(sourceText("  Rust\u202E 1.90\n\treleased\u0007 \u200B ", 64), "Rust 1.90 released");
assert.equal(sourceText("x".repeat(400), MAX_SOURCE_TITLE_LENGTH).length, MAX_SOURCE_TITLE_LENGTH);
assert.ok(sourceText("x".repeat(400), MAX_SOURCE_TITLE_LENGTH).endsWith("…"));
assert.equal(sourceText("😀".repeat(10), 5), "😀😀😀😀…", "bounded by characters, never splitting one");
assert.equal(sourceText({ toString: () => "no" }, 10), "");
assert.equal(sourceText(`${" ".repeat(39)}😀`, 10), "", "a character split by the cut is dropped, not half-kept");

/** @param {Record<string, any>} [overrides] */
function source(overrides = {}) {
  return {
    id: "src_claude_1",
    backend_id: "claude",
    title: "Rust 1.90 released",
    url: "https://blog.rust-lang.org/2026/09/18/Rust-1.90.0.html",
    snippet: "The Rust team is happy to announce a new version.",
    source_name: "Rust Blog",
    age: "1 week ago",
    ...overrides
  };
}

assert.deepEqual(normalizeSource(source()), source(), "a well-formed source is kept as it is");
assert.deepEqual(
  normalizeSource(source({ html: "<img src=x onerror=alert(1)>", onclick: "alert(1)", snippet: "", age: " " })),
  { id: "src_claude_1", backend_id: "claude", title: "Rust 1.90 released", url: source().url, source_name: "Rust Blog" },
  "unknown fields and empty details are dropped"
);
assert.equal(normalizeSource(source({ title: "<b>\u202E</b>" }))?.title, "<b></b>", "markup stays text; it's never parsed");
assert.equal(normalizeSource(source({ title: " \u200B " }))?.title, "blog.rust-lang.org", "a blank title falls back to the site");
assert.equal(normalizeSource(source({ snippet: "y".repeat(5000) }))?.snippet?.length, MAX_SOURCE_SNIPPET_LENGTH);
for (const bad of [
  source({ id: "" }), source({ id: "-x" }), source({ id: "a b" }), source({ id: "x".repeat(129) }), source({ id: 1 }),
  source({ backend_id: "" }), source({ backend_id: "Claude Search" }), source({ url: "javascript:alert(1)" }),
  source({ url: "https://user:pw@example.com/" }), null, "src_claude_1", []
]) {
  assert.equal(normalizeSource(bad), null, JSON.stringify(bad));
}

// A `response.source` event must name the source it carries.
assert.deepEqual(sourceFromEvent({ source_id: "src_claude_1", data: source() }), source());
assert.equal(sourceFromEvent({ source_id: "src_claude_2", data: source() }), null);
assert.equal(sourceFromEvent({ source_id: "src_claude_1" }), null);
assert.equal(sourceFromEvent(null), null);

// One answer: new, well-formed sources in arrival order, up to the limit.
{
  const set = createSourceSet();
  assert.equal(set.add(normalizeSource(source())), true);
  assert.equal(set.add(normalizeSource(source({ id: "src_claude_2" }))), false, "a repeated URL is one source");
  assert.equal(set.add(normalizeSource(source({ url: "https://example.com/other" }))), false, "a repeated ID is one source");
  assert.equal(set.add(null), false);
  for (let index = 2; index <= MAX_SOURCES_PER_ANSWER + 5; index += 1) {
    set.add(normalizeSource(source({ id: `src_claude_${index}`, url: `https://example.com/${index}` })));
  }
  assert.equal(set.size, MAX_SOURCES_PER_ANSWER);
  assert.deepEqual(set.list().slice(0, 3).map((item) => item.id), ["src_claude_1", "src_claude_2", "src_claude_3"]);
}

// Saved sources are checked again, including the shape earlier versions saved.
assert.deepEqual(
  storedSources([
    { id: "src_codex_1", data: { id: "src_codex_1", backend_id: "codex", title: "Legacy", url: "https://example.com/legacy", snippet: "" } },
    { ...source(), url: "javascript:alert(1)" },
    source(),
    source(),
    "junk"
  ]).map((item) => item.id),
  ["src_codex_1", "src_claude_1"]
);
assert.deepEqual(storedSources(undefined), []);

// ---------- The popup and the full view show the same sources ----------

{
  const saved = [
    source(),
    source({ id: "src_claude_2", title: "Release notes", url: "https://github.com/rust-lang/rust/releases", source_name: undefined }),
    source({ id: "src_claude_3", title: "<img src=x onerror=alert(1)>", url: "https://example.com/hostile" })
  ];
  const popup = new Element("section");
  const full = new Element("section");
  assert.equal(render(popup, saved, { variant: "compact", limit: 4 }), 3);
  assert.equal(render(full, saved, { variant: "rich" }), 3);
  assert.deepEqual(shown(popup), shown(full), "same identities, numbers, and links in both views");
  assert.deepEqual(shown(popup).map((item) => [item.id, item.number]),
    [["src_claude_1", "1"], ["src_claude_2", "2"], ["src_claude_3", "3"]]);
  for (const link of shown(full)) {
    assert.equal(link.target, "_blank");
    assert.equal(link.rel, "noopener noreferrer");
    assert.ok(link.href?.startsWith("https://"));
  }
  // Untrusted text is only ever text: no element comes from it.
  assert.ok(descendants(full).every((node) => ["P", "OL", "LI", "A", "SPAN"].includes(node.tagName)));
  assert.equal(byClass(full, "source-title")[2].textContent, "<img src=x onerror=alert(1)>");
  assert.equal(popup.getAttribute("data-variant"), "compact");
  assert.equal(full.getAttribute("data-variant"), "rich");
  assert.equal(popup.getAttribute("aria-label"), "3 sources");
  // The full view adds what the popup leaves out.
  assert.deepEqual(byClass(full, "source-card")[0].children.map((node) => node.className),
    ["source-link", "source-meta", "source-snippet"]);
  assert.deepEqual(byClass(full, "source-meta")[0].children.map((node) => node.textContent),
    ["blog.rust-lang.org", "Rust Blog", "1 week ago"]);
  assert.deepEqual(byClass(popup, "source-host").map((node) => node.textContent),
    ["blog.rust-lang.org", "github.com", "example.com"]);
}

{
  // A few sources fit the popup; the rest are one click away.
  const many = Array.from({ length: 6 }, (_, index) =>
    source({ id: `src_codex_${index + 1}`, backend_id: "codex", url: `https://example.com/${index + 1}` }));
  const popup = new Element("section");
  let more = 0;
  render(popup, many, { variant: "compact", limit: 4, onMore: () => { more += 1; } });
  assert.deepEqual(shown(popup).map((item) => item.number), ["1", "2", "3", "4"]);
  const button = byClass(popup, "source-more")[0];
  assert.equal(button.textContent, "+2");
  assert.ok(/full view/.test(button.getAttribute("aria-label") ?? ""));
  button.click();
  assert.equal(more, 1, "hands off to the full view");

  const inline = new Element("section");
  render(inline, many, { variant: "compact", limit: 4 });
  byClass(inline, "source-more")[0].click();
  assert.deepEqual(shown(inline).map((item) => item.number), ["1", "2", "3", "4", "5", "6"], "or expands in place");

  const none = new Element("section");
  assert.equal(render(none, [{ ...source(), url: "javascript:alert(1)" }]), 0);
  assert.equal(none.hidden, true, "nothing to show hides the list");
}

// ---------- Search the web, as a switch ----------

{
  const button = new Element("button");
  /** @type {boolean[]} */
  const changes = [];
  const search = bindSearchToggle(/** @type {any} */ (button), { onChange: (on) => changes.push(on) });
  assert.equal(search.isOn(), false);
  assert.equal(button.getAttribute("aria-pressed"), "false");
  button.click();
  assert.equal(search.isOn(), true);
  assert.equal(button.getAttribute("aria-pressed"), "true");
  search.setSupported("unknown", "Codex");
  assert.equal(search.isOn(), true, "an unknown capability is left to the host");
  search.setSupported(undefined, "Codex");
  assert.equal(button.disabled, false);
  search.setSupported(false, "Codex");
  assert.equal(search.isOn(), false, "a provider that can't search turns it off");
  assert.equal(button.disabled, true);
  assert.ok(/Codex can't search the web/.test(button.title));
  button.click();
  assert.equal(search.isOn(), false);
  search.setSupported(true, "Claude");
  assert.equal(search.isOn(), false, "it stays off until it's chosen again");
  search.set(true);
  search.set(false);
  assert.deepEqual(changes, [true, false, true, false]);
}

// ---------- A search journey through the worker and the store ----------

let nextUuid = 0;
const uuid = () => `00000000-0000-4000-8000-${String(++nextUuid).padStart(12, "0")}`;
/** @type {Record<string, any>} */
let saved = {};
const storage = {
  async get() { return structuredClone(saved); },
  /** @param {any} update */
  async set(update) { saved = { ...saved, ...structuredClone(update) }; },
  /** @param {string} key */
  async remove(key) { delete saved[key]; }
};
const store = createConversationStore(storage, uuid);
const inFlight = new Set();
/** @type {{request: any, owner: any}[]} */
const native = [];
const manager = {
  /** @param {any} request @param {any} owner */
  send(request, owner) { native.push({ request, owner }); }
};

/**
 * A view (popup or full view) wired to the real worker bridge and store.
 * @param {"compact" | "rich"} variant @param {() => boolean} getSearch
 */
function openView(variant, getSearch) {
  const elements = {
    form: new Element("form"), input: new Element("textarea"), submit: new Element("button"),
    status: new Element("p"), answer: new Element("section"), history: new Element("section"),
    sources: new Element("section"), cancel: new Element("button"), retry: new Element("button")
  };
  /** @type {MockPort[]} */
  const pagePorts = [];
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
      pagePorts.push(worker);
      return ui;
    },
    /** @param {any} message */
    async sendMessage(message) {
      if (message.type === "pervue.conversations.get") return { ok: true, value: await store.get(message.conversation_id) };
      throw new Error("unexpected worker message");
    }
  };
  const view = bindAskForm(/** @type {any} */ (elements), runtime, undefined, {
    getProviderId: () => "claude",
    getSearch,
    renderSources: (container, list) => renderSources(container, list, { variant, limit: 4 })
  });
  return {
    view, elements, pagePorts,
    /** @param {string} question */
    ask(question) { elements.input.value = question; elements.form.fire("submit"); }
  };
}

/** @param {number} index */
function host(index) {
  const { request, owner } = native[index];
  /** @param {string} name @param {any} payload */
  return (name, payload) => owner.onEvent({ version: 1, type: "event", request_id: request.request_id, event: name, payload });
}

async function settle() {
  for (let turn = 0; turn < 4; turn += 1) await new Promise((resolve) => setTimeout(resolve, 0));
}

let searchOn = true;
const popup = openView("compact", () => searchOn);
popup.ask("What's new in Rust?");
await settle();
assert.equal(native.length, 1);
assert.deepEqual(native[0].request.payload.search, {}, "the question asks the provider to search");
assert.equal("context" in native[0].request.payload, false);
assert.equal(popup.elements.history.children[0].getAttribute("data-search"), "true");

// What the host retrieved this turn: two good sources among malformed,
// repeated, and hostile ones.
const retrieved = [
  source({ title: "Rust 1.90 <b>released</b>\u202E" }),
  source({ id: "src_claude_2" }),                                             // same URL
  source({ id: "src_claude_3", url: "javascript:alert(document.cookie)" }),   // not http(s)
  source({ id: "src_claude_4", url: "https://example.com/four" }),            // mismatched below
  source({ id: "src_claude_5", title: "Release notes", url: "https://github.com/rust-lang/rust/releases", snippet: "y".repeat(5000) }),
  source({ id: "src_claude_1", url: "https://example.com/again" })            // same ID
];
const send = host(0);
send("conversation.created", { conversation_id: "claude-session-1" });
send("response.started", { provider_id: "claude", conversation_id: "claude-session-1" });
for (const [index, data] of retrieved.entries()) {
  send("response.source", { source_id: index === 3 ? "src_claude_9" : data.id, data });
}
await settle();
assert.equal(popup.elements.status.textContent, "Searching the web…");
assert.equal(popup.elements.sources.hidden, false, "sources show while the answer is on its way");
assert.deepEqual(shown(popup.elements.sources).map((item) => item.id), ["src_claude_1", "src_claude_5"]);

// The page only ever received the checked copies.
const forwarded = popup.pagePorts[0].messages
  .filter((message) => message.event === "response.source")
  .map((message) => message.payload);
assert.deepEqual(forwarded.map((payload) => payload.source_id), ["src_claude_1", "src_claude_5"]);
assert.equal(forwarded[0].data.title, "Rust 1.90 <b>released</b>");
assert.equal(forwarded[1].data.snippet.length, MAX_SOURCE_SNIPPET_LENGTH);

send("response.delta", { text: "Rust 1.90 is out. " });
await settle();
assert.equal(popup.elements.status.textContent, "Answering…");
send("response.delta", { text: "See the release notes." });
send("response.completed", {});
await settle();

const id = /** @type {string} */ (popup.view.getConversationId());
let conversation = await store.get(id);
const answer = conversation.messages[1];
assert.equal(answer.status, "complete");
assert.equal(conversation.messages[0].search, true);
// Grounding: the answer keeps exactly what its turn retrieved and passed the
// check, in order, with the identities the page was shown.
const retrievedIds = new Set(retrieved.map((item) => item.id));
assert.ok(answer.sources.every((/** @type {any} */ item) => retrievedIds.has(item.id)));
assert.deepEqual(answer.sources, forwarded.map((payload) => payload.data));
assert.deepEqual(conversation.sources.map((/** @type {any} */ item) => item.id), ["src_claude_1", "src_claude_5"]);

// The saved answer shows its sources, and the full view shows the same ones.
const popupSources = byClass(popup.elements.history, "message-sources");
assert.equal(popupSources.length, 1);
assert.equal(popup.elements.sources.hidden, true, "the live list gives way to the saved answer's");
const fullView = openView("rich", () => false);
assert.equal(await fullView.view.loadConversation(id), true);
const fullSources = byClass(fullView.elements.history, "message-sources");
assert.equal(fullSources[0].getAttribute("data-variant"), "rich");
assert.deepEqual(shown(popupSources[0]), shown(fullSources[0]));
assert.deepEqual(shown(fullSources[0]).map((item) => [item.id, item.number]), [["src_claude_1", "1"], ["src_claude_5", "2"]]);

// ---------- A failed search leaves the conversation as it was ----------

popup.ask("And what's next?");
await settle();
assert.equal(native.length, 2);
assert.deepEqual(native[1].request.payload.search, {});
const failing = host(1);
failing("response.started", { provider_id: "claude", conversation_id: "claude-session-1" });
failing("response.source", { source_id: "src_claude_1", data: source({ url: "https://example.com/partial" }) });
failing("response.delta", { text: "Partial" });
failing("response.failed", {
  error: { code: "REQUEST_TIMEOUT", reason: "PROVIDER_TIMEOUT", message: "The answer took too long. Try again.", retryable: true }
});
await settle();
conversation = await store.get(id);
assert.equal(conversation.messages.length, 4);
assert.equal(conversation.messages[3].status, "failed");
assert.equal("sources" in conversation.messages[3], false, "a failed answer mustn't look grounded");
assert.deepEqual(conversation.messages[1].sources, answer.sources, "earlier answers keep theirs");
assert.deepEqual(conversation.sources.map((/** @type {any} */ item) => item.id), ["src_claude_1", "src_claude_5"]);
assert.deepEqual(dialogueHistory(conversation).map((message) => message.text),
  ["What's new in Rust?", "Rust 1.90 is out. See the release notes."], "a failed turn isn't dialogue");
assert.equal(byClass(popup.elements.history, "message-sources").length, 1);
assert.equal(popup.elements.retry.hidden, false);

// A retry searches again, even if the switch was turned off since, and
// keeps only its own turn's sources.
searchOn = false;
popup.elements.retry.click();
await settle();
assert.equal(native.length, 3);
assert.deepEqual(native[2].request.payload.search, {}, "the retry repeats the question as it was asked");
assert.deepEqual(native[2].request.payload.input.history.map((/** @type {any} */ message) => message.text),
  ["What's new in Rust?", "Rust 1.90 is out. See the release notes."]);
const retried = host(2);
retried("response.started", { provider_id: "claude", conversation_id: "claude-session-1" });
retried("response.source", { source_id: "src_claude_1", data: source({ title: "Roadmap", url: "https://example.com/roadmap" }) });
retried("response.delta", { text: "The 2027 edition." });
retried("response.completed", {});
await settle();
conversation = await store.get(id);
assert.equal(conversation.messages.length, 4, "the retry replaces the failed turn");
assert.equal(conversation.messages[3].status, "complete");
assert.deepEqual(conversation.messages[3].sources.map((/** @type {any} */ item) => item.url), ["https://example.com/roadmap"]);
assert.equal(conversation.messages[2].search, true);

// With the switch off, the next question doesn't search.
popup.ask("Thanks");
await settle();
assert.equal("search" in native[3].request.payload, false);
host(3)("response.failed", { error: { code: "PROVIDER_FAILED", reason: "PROVIDER_EXITED", message: "Claude stopped.", retryable: false } });
await settle();

// A search that finds nothing fails with the host's message, not an
// ungrounded answer.
searchOn = true;
const empty = openView("compact", () => searchOn);
empty.ask("Find something obscure");
await settle();
const nothing = host(4);
nothing("conversation.created", { conversation_id: "claude-session-2" });
nothing("response.started", { provider_id: "claude", conversation_id: "claude-session-2" });
nothing("response.failed", {
  error: {
    code: "SEARCH_FAILED", reason: "NATIVE_SEARCH_NO_SOURCES",
    message: "The provider finished web search without returning usable sources. Try again or update the provider.",
    retryable: true
  }
});
await settle();
assert.equal(empty.elements.status.getAttribute("data-kind"), "search-failed");
assert.ok(/without returning usable sources/.test(empty.elements.status.textContent));
const emptyConversation = await store.get(/** @type {string} */ (empty.view.getConversationId()));
assert.equal(emptyConversation.messages[1].status, "failed");
assert.equal("sources" in emptyConversation.messages[1], false);
assert.deepEqual(emptyConversation.sources, []);

// ---------- Questions the worker refuses before anything is saved ----------

{
  const before = native.length;
  const port = new MockPort(ASK_PORT_NAME);
  serveConversationAskPort(/** @type {any} */ (port), { manager, store, inFlight });
  port.emitMessage({
    type: "ask", text: "Search this page", provider_id: "claude", search: true,
    context: { mode: "page", text: "Page text", truncated: false, page: { title: "Page", url: "https://example.com/" } }
  });
  await settle();
  assert.equal(port.messages.at(-1).payload.error.reason, "SEARCH_WITH_CONTEXT_UNSUPPORTED");
  assert.equal(port.messages.at(-1).payload.error.code, "SEARCH_FAILED");

  const typed = new MockPort(ASK_PORT_NAME);
  serveConversationAskPort(/** @type {any} */ (typed), { manager, store, inFlight });
  typed.emitMessage({ type: "ask", text: "Hello", search: "yes" });
  await settle();
  assert.equal(typed.messages.at(-1).payload.error.reason, "INVALID_PAYLOAD");
  assert.equal(native.length, before, "nothing reached the host");
}

// ---------- The page checks sources too ----------

{
  // Even if something unchecked reached the page, it wouldn't be shown.
  const port = new MockPort(ASK_PORT_NAME);
  const elements = {
    form: new Element("form"), input: new Element("textarea"), submit: new Element("button"),
    status: new Element("p"), answer: new Element("section"), sources: new Element("section")
  };
  /** @type {any[][]} */
  const handed = [];
  bindAskForm(/** @type {any} */ (elements), { connect: () => /** @type {any} */ (port) }, undefined, {
    getSearch: () => true,
    renderSources: (container, list) => {
      handed.push(list);
      renderSources(container, list);
    }
  });
  elements.input.value = "Search";
  elements.form.fire("submit");
  assert.equal(port.messages[0].search, true);
  port.emitMessage({ event: "response.started", payload: {} });
  port.emitMessage({ event: "response.source", payload: { source_id: "s1", data: { id: "s1", backend_id: "claude", title: "x", url: "javascript:alert(1)" } } });
  port.emitMessage({ event: "response.source", payload: { source_id: "s2", data: {} } });
  assert.equal(elements.sources.hidden, true);
  port.emitMessage({ event: "response.source", payload: { source_id: "s3", data: { id: "s3", backend_id: "claude", title: "ok", url: "https://example.com/" } } });
  assert.deepEqual(shown(elements.sources).map((item) => item.id), ["s3"]);
  assert.deepEqual(handed, [[{ id: "s3", backend_id: "claude", title: "ok", url: "https://example.com/" }]],
    "the renderer is only handed checked sources");
}

console.log("TST-14 search, source grounding, and SEC-05 source tests passed");
