import assert from "node:assert/strict";
import { bindAskForm } from "../src/popup/ask-form.js";
import { parseInline, parseMarkdown, renderMarkdown, safeHref } from "../src/shared/markdown.js";
import { createStreamReveal } from "../src/shared/stream-reveal.js";
import { ASK_PORT_NAME } from "../src/shared/ask-port.js";
import { MockPort } from "./support/mock-port.mjs";

// ---------- Parsing ----------

/** @param {string} text */
const t = (text) => ({ type: "text", text });

assert.deepEqual(parseInline("**bold** and *em* and ~~gone~~ and `code`"), [
  { type: "strong", children: [t("bold")] },
  t(" and "),
  { type: "em", children: [t("em")] },
  t(" and "),
  { type: "del", children: [t("gone")] },
  t(" and "),
  { type: "code", text: "code" }
]);
assert.deepEqual(parseInline("***both***"), [{ type: "strong", children: [{ type: "em", children: [t("both")] }] }]);
assert.deepEqual(parseInline("*outer **inner** outer*"), [
  { type: "em", children: [t("outer "), { type: "strong", children: [t("inner")] }, t(" outer")] }
]);

// Things that only look like emphasis stay text.
assert.deepEqual(parseInline("snake_case_name and 2 * 3 * 4"), [t("snake_case_name and 2 * 3 * 4")]);
assert.deepEqual(parseInline("**unclosed while streaming"), [t("**unclosed while streaming")]);
assert.deepEqual(parseInline("`a*b*c`"), [{ type: "code", text: "a*b*c" }]);
assert.deepEqual(parseInline("\\*not em\\*"), [t("*not em*")]);

// A single newline is a line break, as chat models mean it.
assert.deepEqual(parseInline("one  \ntwo"), [t("one"), { type: "break" }, t("two")]);

// Links: safe targets only, labels parsed, no nesting.
assert.deepEqual(parseInline("[the *docs*](https://example.com/a_(b))"), [
  { type: "link", href: "https://example.com/a_(b)", children: [t("the "), { type: "em", children: [t("docs")] }] }
]);
assert.deepEqual(parseInline("[click](javascript:alert(1))"), [t("click")]);
assert.deepEqual(parseInline("[rel](/src/popup/index.html)"), [t("rel")]);
assert.deepEqual(parseInline("see https://example.com/path)."), [
  t("see "),
  { type: "link", href: "https://example.com/path", children: [t("https://example.com/path")] },
  t(").")
]);
assert.deepEqual(parseInline("<mailto:a@example.com>"), [
  { type: "link", href: "mailto:a@example.com", children: [t("mailto:a@example.com")] }
]);
// An image becomes a link to it: nothing is fetched.
assert.deepEqual(parseInline("![chart](https://example.com/c.png)"), [
  { type: "link", href: "https://example.com/c.png", children: [t("chart")] }
]);
assert.equal(safeHref("data:text/html,<script>"), null);
assert.equal(safeHref("HTTPS://Example.com"), "https://example.com/");

{
  const blocks = parseMarkdown([
    "## Steps",
    "",
    "Intro line",
    "1. First",
    "   - nested *a*",
    "   - nested b",
    "2. Second",
    "",
    "- [x] done",
    "- [ ] todo",
    "",
    "> quoted",
    "> more",
    "",
    "```js",
    "const x = 1 < 2;",
    "```",
    "",
    "| Name | Qty |",
    "| :--- | --: |",
    "| a \\| b | `1|2` |",
    "",
    "---"
  ].join("\n"));
  assert.deepEqual(blocks.map((block) => block.type),
    ["heading", "paragraph", "list", "list", "quote", "code", "table", "rule"]);
  assert.deepEqual(blocks[0], { type: "heading", level: 2, children: [t("Steps")] });
  const ordered = /** @type {any} */ (blocks[2]);
  assert.equal(ordered.ordered, true);
  assert.equal(ordered.items.length, 2);
  assert.equal(ordered.items[0].children[1].type, "list", "the nested list belongs to its item");
  assert.equal(ordered.items[0].children[1].items.length, 2);
  const tasks = /** @type {any} */ (blocks[3]);
  assert.deepEqual(tasks.items.map((/** @type {any} */ item) => item.checked), [true, false]);
  assert.deepEqual(blocks[4], { type: "quote", children: [{ type: "paragraph", children: [t("quoted"), { type: "break" }, t("more")] }] });
  assert.deepEqual(blocks[5], { type: "code", lang: "js", text: "const x = 1 < 2;" });
  const tableBlock = /** @type {any} */ (blocks[6]);
  assert.deepEqual(tableBlock.align, ["left", "right"]);
  assert.deepEqual(tableBlock.rows[0], [[t("a | b")], [{ type: "code", text: "1|2" }]]);
}

// A fence still streaming runs to the end of the answer.
assert.deepEqual(parseMarkdown("Here:\n```py\nprint(1)"), [
  { type: "paragraph", children: [t("Here:")] },
  { type: "code", lang: "py", text: "print(1)" }
]);
// "#tag" and "1.5" are text, not a heading or a list.
assert.deepEqual(parseMarkdown("#tag costs 1.5 each").map((block) => block.type), ["paragraph"]);
// Ordered lists keep their start.
assert.equal(/** @type {any} */ (parseMarkdown("3. three\n4. four")[0]).start, 3);

// A closing run longer than its opener closes nested emphasis too.
assert.deepEqual(parseInline("**bold and *italic***"), [
  { type: "strong", children: [t("bold and "), { type: "em", children: [t("italic")] }] }
]);
assert.deepEqual(parseInline("*italic and **bold***"), [
  { type: "em", children: [t("italic and "), { type: "strong", children: [t("bold")] }] }
]);
assert.deepEqual(parseInline("**a *b* c**"), [
  { type: "strong", children: [t("a "), { type: "em", children: [t("b")] }, t(" c")] }
]);
assert.deepEqual(parseInline("*a**"), [{ type: "em", children: [t("a")] }, t("*")]);

// ---------- Resource bounds on hostile answers ----------

/** Milliseconds `run` takes. @param {() => void} run */
function timed(run) {
  const start = globalThis.performance.now();
  run();
  return globalThis.performance.now() - start;
}

// Nesting deep enough to exhaust the stack stays text past the depth limit.
{
  const blocks = parseMarkdown(`${">".repeat(50_000)} deep`);
  let depth = 0;
  /** @type {any} */
  let block = blocks[0];
  while (block?.type === "quote") {
    depth += 1;
    block = block.children[0];
  }
  assert.ok(depth <= 12, `quotes nest ${depth} deep`);
  assert.equal(block.type, "paragraph");
  // These threw RangeError before the limit; now they parse.
  parseMarkdown(`${"- ".repeat(50_000)}x`);
  parseMarkdown(`${"1. ".repeat(50_000)}x`);
}

// Unmatched openers cost linear time, not quadratic: at this size a
// quadratic scan takes many seconds.
for (const hostile of ["[".repeat(80_000), "`a".repeat(40_000), "*.js ".repeat(20_000), "h ".repeat(40_000), "<http://x ".repeat(8_000)]) {
  const ms = timed(() => parseMarkdown(hostile));
  assert.ok(ms < 1500, `${JSON.stringify(hostile.slice(0, 12))}… parsed in ${ms.toFixed(0)}ms`);
}

// ---------- Rendering ----------

class Node {
  /** @param {string} tag */
  constructor(tag) {
    this.tag = tag;
    /** @type {(Node | string)[]} */
    this.children = [];
    /** @type {Map<string, string>} */
    this.attributes = new Map();
    this.className = "";
    /** @type {Map<string, (() => void)[]>} */
    this.listeners = new Map();
    this.ownerDocument = fakeDocument;
  }
  /** @returns {string} */
  get textContent() {
    return this.children.map((child) => typeof child === "string" ? child : child.textContent).join("");
  }
  /** @param {string} value */
  set textContent(value) { this.children = value ? [String(value)] : []; }
  /** @param {...(Node | string)} nodes */
  append(...nodes) { this.children.push(...nodes); }
  /** @param {...(Node | string)} nodes */
  replaceChildren(...nodes) { this.children = nodes; }
  /** @param {string} name @param {string} value */
  setAttribute(name, value) { this.attributes.set(name, value); }
  /** @param {string} name */
  getAttribute(name) { return this.attributes.get(name) ?? null; }
  /** @param {string} name */
  removeAttribute(name) { this.attributes.delete(name); }
  /** @param {string} type @param {() => void} listener */
  addEventListener(type, listener) { this.listeners.set(type, [...this.listeners.get(type) ?? [], listener]); }
  /** @returns {Node[]} */
  all() {
    return this.children.flatMap((child) => typeof child === "string" ? [] : [child, ...child.all()]);
  }
}
const fakeDocument = {
  /** @param {string} tag */
  createElement(tag) { return new Node(tag); }
};

{
  const container = new Node("div");
  const hostile = [
    "<img src=x onerror=alert(1)> <b>not bold</b>",
    "",
    "[x](javascript:alert(1)) [y](https://example.com)",
    "",
    "```html",
    "<script>alert(1)</script>",
    "```"
  ].join("\n");
  renderMarkdown(/** @type {any} */ (container), hostile);
  const nodes = container.all();
  assert.ok(!nodes.some((node) => ["img", "b", "script"].includes(node.tag)), "HTML in an answer stays text");
  assert.equal(nodes[0].tag, "p");
  assert.ok(nodes[0].textContent.startsWith("<img src=x onerror=alert(1)> <b>not bold</b>"));
  const links = nodes.filter((node) => node.tag === "a");
  assert.equal(links.length, 1);
  assert.equal(links[0].getAttribute("href"), "https://example.com/");
  assert.equal(links[0].getAttribute("target"), "_blank");
  assert.equal(links[0].getAttribute("rel"), "noopener noreferrer");
  const code = nodes.find((node) => node.tag === "code");
  assert.equal(code?.textContent, "<script>alert(1)</script>");
  assert.equal(code?.className, "language-html");
  const copy = nodes.find((node) => node.className.includes("code-copy"));
  assert.equal(copy?.getAttribute("aria-label"), "Copy html code");
}

{
  const container = new Node("div");
  renderMarkdown(/** @type {any} */ (container), "# Title\n\n3. a\n4. b\n\n- [x] done\n\n| A |\n|---|\n| 1 |");
  const [heading, list, tasks, wrap] = /** @type {Node[]} */ (container.children);
  assert.equal(heading.tag, "h3", "an answer's headings sit below the page's own");
  assert.equal(list.tag, "ol");
  assert.equal(list.getAttribute("start"), "3");
  assert.equal(list.textContent, "ab");
  const box = /** @type {Node} */ (/** @type {Node} */ (tasks.children[0]).children[0]);
  assert.equal(box.tag, "input");
  assert.equal(box.getAttribute("type"), "checkbox");
  assert.equal(box.getAttribute("disabled"), "");
  assert.equal(box.getAttribute("checked"), "");
  assert.equal(wrap.className, "table-wrap");
  assert.deepEqual(wrap.all().map((node) => node.tag), ["table", "thead", "tr", "th", "tbody", "tr", "td"]);
}

// An answer too costly to format renders as plain text, quickly.
{
  const container = new Node("div");
  const hostile = `${"*a **b ".repeat(4_000)}${"c** d*".repeat(4_000)}`;
  const ms = timed(() => renderMarkdown(/** @type {any} */ (container), hostile));
  assert.ok(ms < 1500, `rendered in ${ms.toFixed(0)}ms`);
  const [plain] = /** @type {Node[]} */ (container.children);
  assert.equal(container.children.length, 1);
  assert.equal(plain.tag, "p");
  assert.equal(plain.className, "markdown-plain");
  assert.equal(plain.textContent, hostile);
}

// While an answer types out, links and copy buttons can't take focus: each
// frame rebuilds them.
{
  const container = new Node("div");
  renderMarkdown(/** @type {any} */ (container), "[docs](https://example.com)\n\n```sh\nls\n```", { interactive: false });
  const nodes = container.all();
  assert.ok(!nodes.some((node) => node.tag === "a"));
  const link = nodes.find((node) => node.className === "link");
  assert.equal(link?.tag, "span");
  assert.equal(link?.textContent, "docs");
  const copy = nodes.find((node) => node.className.includes("code-copy"));
  assert.equal(copy?.getAttribute("disabled"), "");
  assert.equal(copy?.listeners.size, 0);
}

// ---------- Reveal ----------

{
  /** @type {((now: number) => void)[]} */
  let frames = [];
  let clock = 0;
  /** @type {string[]} */
  const rendered = [];
  const reveal = createStreamReveal({
    render: (_element, text) => { rendered.push(text); },
    schedule: (callback) => { frames.push(callback); return frames.length; },
    cancel: () => { frames = []; }
  });
  const flush = () => {
    const due = frames;
    frames = [];
    clock += 16;
    for (const frame of due) frame(clock);
  };
  const element = /** @type {any} */ ({});

  // A whole answer arriving at once is typed out over a few frames.
  const answer = "Rust gives every value one owner. ".repeat(20);
  let settled = false;
  void reveal(element, answer).then(() => { settled = true; });
  assert.equal(rendered.length, 0, "nothing shows before the first frame");
  flush();
  assert.ok(rendered[0].length > 0 && rendered[0].length < answer.length, "the first frame shows part of it");
  assert.equal(answer[rendered[0].length], " ", "a frame ends at a word boundary");
  let guard = 0;
  while (frames.length && guard++ < 500) flush();
  await Promise.resolve();
  assert.equal(rendered.at(-1), answer);
  assert.equal(settled, true);
  assert.ok(rendered.length > 5 && rendered.length < 120, `revealed over ${rendered.length} frames`);
  for (let i = 1; i < rendered.length; i += 1) assert.ok(rendered[i].startsWith(rendered[i - 1]), "text only grows");

  // More text arriving continues from where it is, and "" clears at once.
  rendered.length = 0;
  void reveal(element, `${answer}More.`);
  while (frames.length && guard++ < 1000) flush();
  assert.equal(rendered.at(-1), `${answer}More.`);
  assert.ok(rendered[0].length > answer.length, "an addition doesn't retype what is shown");
  let cleared = false;
  void reveal(element, "Something else entirely").then(() => { cleared = true; });
  flush();
  void reveal(element, "");
  await Promise.resolve();
  assert.equal(cleared, true, "clearing settles a reveal in progress");
  assert.equal(rendered.at(-1), "");
  assert.equal(frames.length, 0);

  // Never splits an emoji's surrogate pair.
  rendered.length = 0;
  void reveal(element, "😀".repeat(400));
  while (frames.length && guard++ < 2000) flush();
  assert.ok(rendered.every((text) => !/[\uD800-\uDBFF]$/.test(text)));
}

{
  // A render that throws ends the reveal instead of leaving it pending.
  /** @type {((now: number) => void)[]} */
  const frames = [];
  const reveal = createStreamReveal({
    render: () => { throw new Error("render failed"); },
    schedule: (callback) => { frames.push(callback); return frames.length; },
    cancel: () => {}
  });
  let settled = false;
  void reveal(/** @type {any} */ ({}), "Some answer text").then(() => { settled = true; });
  /** @type {(now: number) => void} */ (frames.shift())(16);
  await Promise.resolve();
  assert.equal(settled, true);
  assert.equal(frames.length, 0);
}

{
  // With reduced motion the text shows at once.
  /** @type {string[]} */
  const rendered = [];
  const reveal = createStreamReveal({
    render: (_element, text) => { rendered.push(text); },
    animate: () => false,
    schedule: () => { throw new Error("no frames with reduced motion"); },
    cancel: () => {}
  });
  await reveal(/** @type {any} */ ({}), "All of it");
  assert.deepEqual(rendered, ["All of it"]);
}

// ---------- The ask form with renderers ----------

{
  const ownerDocument = { activeElement: null, createElement: () => new FormElement() };
  class FormElement {
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
      this.ownerDocument = ownerDocument;
    }
    /** @param {string} type @param {(event: any) => void} listener */
    addEventListener(type, listener) { this.listeners.set(type, [...this.listeners.get(type) ?? [], listener]); }
    /** @param {string} type */
    fire(type) { for (const listener of this.listeners.get(type) ?? []) listener({ preventDefault() {}, key: type }); }
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
    requestSubmit() { this.fire("submit"); }
  }

  const conversation = {
    id: "conv_00000000-0000-4000-8000-000000000009",
    messages: [
      { role: "user", text: "Question", status: "complete" },
      { role: "assistant", text: "**Answer**", status: "complete" }
    ]
  };
  /** @type {MockPort[]} */
  const ports = [];
  let loads = 0;
  const runtime = {
    connect() {
      const port = new MockPort(ASK_PORT_NAME);
      ports.push(port);
      return port;
    },
    async sendMessage() {
      loads += 1;
      return { ok: true, value: conversation };
    }
  };
  /** @type {{text: string, resolve: () => void}[]} */
  const reveals = [];
  /** @type {string[]} */
  const markdownBodies = [];
  /** @type {((changes: any, area: string) => void)[]} */
  const storageListeners = [];
  const elements = {
    form: new FormElement(),
    input: new FormElement(),
    submit: new FormElement(),
    status: new FormElement(),
    answer: new FormElement(),
    history: new FormElement()
  };
  bindAskForm(/** @type {any} */ (elements), /** @type {any} */ (runtime), undefined, {
    renderMessage: (body, text) => { markdownBodies.push(`${body.className}:${text}`); },
    renderAnswer: (_answer, text) => new Promise((resolve) => reveals.push({ text, resolve: () => resolve() })),
    storageChanges: { addListener: (listener) => { storageListeners.push(listener); } }
  });

  elements.input.value = "Question";
  elements.form.fire("submit");
  assert.equal(reveals.at(-1)?.text, "", "a new question clears the answer through the renderer");
  const port = ports[0];
  port.emitMessage({ event: "conversation.created", payload: { conversation_id: conversation.id } });
  port.emitMessage({ event: "response.delta", payload: { text: "**Ans" } });
  port.emitMessage({ event: "response.delta", payload: { text: "wer**" } });
  assert.deepEqual(reveals.slice(-2).map((reveal) => reveal.text), ["**Ans", "**Answer**"],
    "the renderer gets the whole answer so far");
  assert.equal(elements.answer.children.length, 0, "no raw text is appended alongside the renderer");

  port.emitMessage({ event: "response.completed", payload: {} });
  // The conversation is saved as the answer completes (Chrome reports a
  // write with its newValue; only a removal has none).
  for (const listener of storageListeners) {
    listener({ [`tabbeam.conversation.${conversation.id}`]: { newValue: {} } }, "local");
  }
  await new Promise((resolve) => setTimeout(resolve, 0));
  assert.equal(loads, 0, "the saved copy waits for the answer to finish typing out");
  assert.equal(elements.answer.hidden, false);
  assert.equal(elements.answer.getAttribute("aria-busy"), "true");
  assert.equal(elements.submit.getAttribute("aria-disabled"), "false", "asking again is possible meanwhile");

  reveals.at(-1)?.resolve();
  await new Promise((resolve) => setTimeout(resolve, 0));
  assert.equal(loads, 1);
  assert.equal(elements.answer.hidden, true);
  assert.equal(elements.answer.getAttribute("aria-busy"), "false");
  assert.deepEqual(markdownBodies, ["message-body markdown:**Answer**"], "only TabBeam's turns render as Markdown");
  const [question] = elements.history.children;
  assert.equal(question.children[1].className, "message-body");
  assert.equal(question.children[1].textContent, "Question");

  // A follow-up asked while the answer is still typing out: that answer
  // stays in the thread, ahead of the new question.
  elements.input.value = "Second";
  elements.form.fire("submit");
  ports[1].emitMessage({ event: "response.delta", payload: { text: "Second answer" } });
  ports[1].emitMessage({ event: "response.completed", payload: {} });
  await new Promise((resolve) => setTimeout(resolve, 0));
  const typingOut = /** @type {{text: string, resolve: () => void}} */ (reveals.at(-1));
  assert.equal(typingOut.text, "Second answer");

  elements.input.value = "Third";
  elements.form.fire("submit");
  const thread = elements.history.children.slice(-3);
  assert.deepEqual(thread.map((/** @type {any} */ item) => item.className),
    ["message message-user", "message message-assistant", "message message-user"]);
  assert.equal(thread[1].getAttribute("data-state"), "complete");
  assert.equal(markdownBodies.at(-1), "message-body markdown:Second answer");
  assert.equal(thread[2].children[1].textContent, "Third");

  // The superseded reveal settles without reloading over the new question.
  typingOut.resolve();
  await new Promise((resolve) => setTimeout(resolve, 0));
  assert.equal(loads, 1);
  assert.equal(elements.history.children.length, 5);
}

console.log("Markdown rendering and streaming reveal tests passed");
