import { CONVERSATIONS_DELETE_MESSAGE, CONVERSATIONS_LIST_MESSAGE } from "../background/conversation-messages.js";

const SVG_NS = "http://www.w3.org/2000/svg";
const DAY_MS = 86_400_000;
/** How long a delete button waits for its confirming second click. */
export const CONFIRM_MS = 3000;

/**
 * @typedef {{id: string, title: string, created_at?: string, updated_at?: string}} ConversationSummary
 */

/** Local midnight of the day holding `time`. @param {number} time */
function startOfDay(time) {
  const date = new Date(time);
  date.setHours(0, 0, 0, 0);
  return date.getTime();
}

/**
 * The heading a conversation last used at `iso` sorts under.
 * @param {string | undefined} iso
 * @param {number} now
 */
export function dayGroup(iso, now) {
  const time = Date.parse(iso ?? "");
  if (Number.isNaN(time)) return "Older";
  const today = startOfDay(now);
  if (time >= today) return "Today";
  if (time >= today - DAY_MS) return "Yesterday";
  if (time >= today - 7 * DAY_MS) return "Previous 7 days";
  if (time >= today - 30 * DAY_MS) return "Previous 30 days";
  return "Older";
}

/**
 * A short "when": minutes or hours today, then a weekday, then a date.
 * @param {string | undefined} iso
 * @param {number} now
 */
export function shortWhen(iso, now) {
  const time = Date.parse(iso ?? "");
  if (Number.isNaN(time)) return "";
  const minutes = Math.floor((now - time) / 60_000);
  if (minutes < 1) return "now";
  if (minutes < 60) return `${minutes}m`;
  if (time >= startOfDay(now)) return `${Math.floor(minutes / 60)}h`;
  if (time >= startOfDay(now) - 6 * DAY_MS) return new Intl.DateTimeFormat(undefined, { weekday: "short" }).format(time);
  return new Intl.DateTimeFormat(undefined, { month: "short", day: "numeric" }).format(time);
}

/**
 * The saved conversations, newest first. The popup's History view and the
 * full view's sidebar both use it. Choosing one loads it into `view`; if the
 * view can't switch (an answer is running) the current one stays marked.
 * Deleting takes a second click to confirm.
 *
 * @param {{list: HTMLElement, empty?: HTMLElement}} elements `list` receives
 *   the rows; `empty` shows when there are none
 * @param {{sendMessage(message: any): Promise<any>}} runtime
 * @param {{getConversationId(): string | null, loadConversation(id: string): Promise<boolean>}} view
 * @param {{
 *   limit?: number,
 *   grouped?: boolean,
 *   deletable?: boolean,
 *   now?: () => number,
 *   onOpen?(id: string): void,
 *   onBlocked?(): void,
 *   onDeleted?(id: string, wasActive: boolean): void,
 *   onError?(message: string): void,
 *   onChange?(items: ConversationSummary[]): void
 * }} [options]
 */
export function bindConversationList(elements, runtime, view, options = {}) {
  const { list, empty } = elements;
  const { limit = Infinity, grouped = true, deletable = false, now = () => Date.now() } = options;
  const doc = list.ownerDocument;
  /** @type {ConversationSummary[]} */
  let items = [];
  let query = "";
  /** @type {string | null} */
  let confirming = null;
  /** @type {any} */
  let confirmTimer = null;

  async function refresh() {
    try {
      const result = await runtime.sendMessage({ type: CONVERSATIONS_LIST_MESSAGE });
      if (result?.ok !== true || !Array.isArray(result.value)) throw new Error("unavailable");
      items = result.value;
    } catch {
      items = [];
    }
    render();
    options.onChange?.(items);
    return items;
  }

  function render() {
    const active = view.getConversationId();
    const words = query.toLowerCase().split(/\s+/).filter(Boolean);
    const shown = items
      .filter((item) => words.every((word) => item.title.toLowerCase().includes(word)))
      .slice(0, limit);
    const time = now();
    /** @type {HTMLElement[]} */
    const nodes = [];
    let heading = "";
    let group = /** @type {HTMLElement | null} */ (null);
    for (const item of shown) {
      const label = grouped ? dayGroup(item.updated_at, time) : "";
      if (!group || label !== heading) {
        heading = label;
        group = doc.createElement("ul");
        group.className = "conversation-group";
        if (grouped) {
          const title = doc.createElement("li");
          title.className = "conversation-group-label";
          title.setAttribute("role", "presentation");
          title.textContent = label;
          group.append(title);
        }
        nodes.push(group);
      }
      group.append(row(item, item.id === active, time));
    }
    list.replaceChildren(...nodes);
    if (empty) {
      empty.hidden = shown.length > 0;
      empty.textContent = items.length && query ? "No conversations match." : "No conversations yet.";
    }
  }

  /** @param {ConversationSummary} item @param {boolean} active @param {number} time */
  function row(item, active, time) {
    const li = doc.createElement("li");
    li.className = "conversation-row";
    li.setAttribute("data-conversation", item.id);
    const open = doc.createElement("button");
    open.type = "button";
    open.className = "conversation-open";
    open.title = item.title;
    if (active) open.setAttribute("aria-current", "true");
    const name = doc.createElement("span");
    name.className = "conversation-name";
    // Titles come from what the person asked: text only, never markup.
    name.textContent = item.title;
    const when = doc.createElement("span");
    when.className = "conversation-when";
    when.textContent = shortWhen(item.updated_at, time);
    open.append(icon("chat"), name, when);
    open.addEventListener("click", () => { void choose(item.id); });
    li.append(open);

    if (deletable) {
      const remove = doc.createElement("button");
      remove.type = "button";
      remove.className = "conversation-delete";
      const armed = confirming === item.id;
      remove.setAttribute("aria-label", armed ? `Confirm deleting “${item.title}”` : `Delete “${item.title}”`);
      remove.title = armed ? "Click again to delete" : "Delete";
      if (armed) {
        remove.setAttribute("data-confirm", "true");
        remove.append("Delete?");
      } else {
        remove.append(icon("trash"));
      }
      remove.addEventListener("click", () => { void del(item.id, remove); });
      li.append(remove);
    }
    return li;
  }

  /** @param {string} name */
  function icon(name) {
    const svg = doc.createElementNS(SVG_NS, "svg");
    svg.setAttribute("class", "icon icon-sm");
    svg.setAttribute("aria-hidden", "true");
    const use = doc.createElementNS(SVG_NS, "use");
    use.setAttribute("href", `../shared/icons.svg#${name}`);
    svg.append(use);
    return svg;
  }

  /** @param {string} id */
  async function choose(id) {
    if (id !== view.getConversationId() && !await view.loadConversation(id)) {
      render();
      options.onBlocked?.();
      return;
    }
    render();
    options.onOpen?.(id);
  }

  /** @param {string} id @param {HTMLElement} button */
  async function del(id, button) {
    clearTimeout(confirmTimer);
    if (confirming !== id) {
      confirming = id;
      render();
      // Keep focus on the same row's button now that it asks to confirm.
      focusDelete(id);
      confirmTimer = setTimeout(() => {
        confirming = null;
        render();
      }, CONFIRM_MS);
      return;
    }
    confirming = null;
    button.setAttribute("aria-busy", "true");
    const wasActive = id === view.getConversationId();
    const result = await runtime.sendMessage({ type: CONVERSATIONS_DELETE_MESSAGE, conversation_id: id })
      .catch(() => null);
    if (result?.ok !== true) {
      render();
      options.onError?.(typeof result?.error === "string" ? result.error : "Couldn't delete the conversation.");
      return;
    }
    options.onDeleted?.(id, wasActive);
    await refresh();
  }

  /** @param {string} id conversation IDs are `conv_` and a UUID, safe in a selector */
  function focusDelete(id) {
    const button = /** @type {HTMLElement | null | undefined} */ (
      list.querySelector?.(`[data-conversation="${id}"] .conversation-delete`));
    button?.focus();
  }

  return {
    refresh,
    render,
    /** @param {string} text */
    setQuery(text) {
      query = text.trim();
      render();
    },
    items: () => items
  };
}
