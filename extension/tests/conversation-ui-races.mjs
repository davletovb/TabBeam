import assert from "node:assert/strict";
import { bindAskForm } from "../src/popup/ask-form.js";
import { bindContextControls } from "../src/popup/context-controls.js";
import { bindRecentConversations } from "../src/shared/recent-conversations.js";

class Element {
  constructor() {
    /** @type {Map<string, (event: any) => void>} */
    this.listeners = new Map();
    this.attributes = new Map();
    this.value = "";
    this.textContent = "";
    this.hidden = false;
    this.disabled = false;
    this.ownerDocument = { createElement: () => new Element() };
  }
  /** @param {string} event @param {(value: any) => void} callback */
  addEventListener(event, callback) { this.listeners.set(event, callback); }
  /** @param {string} event */
  fire(event) { return this.listeners.get(event)?.({ preventDefault() {} }); }
  /** @param {string} key @param {string} value */
  setAttribute(key, value) { this.attributes.set(key, value); }
  /** @param {string} key */
  removeAttribute(key) { this.attributes.delete(key); }
  /** @param {...any} values */
  append(...values) { this.children = [...this.children ?? [], ...values]; }
  /** @param {...any} values */
  replaceChildren(...values) { this.children = values; }
  focus() {}
  requestSubmit() { return this.fire("submit"); }
}

// A picker that cannot load B while A is answering must point back at A.
const select = new Element();
const currentId = "conv_00000000-0000-4000-8000-000000000001";
bindRecentConversations(/** @type {any} */ (select), /** @type {any} */ ({}), {
  getConversationId: () => currentId,
  async loadConversation() { return false; }
});
select.value = "conv_00000000-0000-4000-8000-000000000002";
select.fire("change");
await new Promise((resolve) => setTimeout(resolve, 0));
assert.equal(select.value, currentId);

// An ask while history is loading must wait instead of going to the old ID.
const elements = {
  form: new Element(), input: new Element(), submit: new Element(),
  status: new Element(), answer: new Element(), history: new Element()
};
/** @type {(value: any) => void} */
let resolveGet = () => {};
let connects = 0;
const view = bindAskForm(/** @type {any} */ (elements), {
  sendMessage() { return new Promise((resolve) => { resolveGet = resolve; }); },
  connect() { connects += 1; throw new Error("not used"); }
});
const loading = view.loadConversation(currentId);
elements.input.value = "Follow-up";
elements.form.fire("submit");
assert.equal(connects, 0);
resolveGet({ ok: true, value: { id: currentId, messages: [] } });
assert.equal(await loading, true);
assert.equal(view.getConversationId(), currentId);

// Consuming context used by an answer cannot cancel a new capture that
// started while that answer was streaming.
/** @type {Record<string, () => void | Promise<void>>} */
const clicks = {};
const button = (/** @type {string} */ name) => {
  const element = new Element();
  element.addEventListener("click", () => clicks[name]?.());
  return element;
};
const controls = {
  none: button("none"), selection: button("selection"), page: button("page"),
  status: new Element(), preview: new Element()
};
/** @type {((value: any) => void)[]} */
const pending = [];
const state = bindContextControls(/** @type {any} */ (controls), {
  sendMessage() { return new Promise((resolve) => { pending.push(resolve); }); }
});
// The bind function overwrites click handlers on the same elements.
const first = controls.selection.fire("click");
pending.shift()?.({ ok: true, context: {
  mode: "selection", text: "first", truncated: false,
  page: { title: "Page", url: "https://example.com/" }
} });
await first;
const sent = state.getContext();
const second = controls.page.fire("click");
state.consume(sent);
assert.equal(state.isPending(), true);
pending.shift()?.({ ok: true, context: {
  mode: "page", text: "second", truncated: false,
  page: { title: "Page", url: "https://example.com/" }
} });
await second;
assert.equal(state.getContext().text, "second");
console.log("EXT-05/07 context and picker race tests passed");
