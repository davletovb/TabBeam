/**
 * A recent index shared by the popup and full view. It displays only the
 * worker's public metadata; provider session IDs stay in native storage.
 * @param {HTMLSelectElement} select
 * @param {{sendMessage(message: any): Promise<any>}} runtime
 * @param {{getConversationId(): string | null, loadConversation(id: string): Promise<boolean>}} view
 */
export function bindRecentConversations(select, runtime, view) {
  select.addEventListener("change", async () => {
    if (select.value && !await view.loadConversation(select.value)) {
      select.value = view.getConversationId() ?? "";
    }
  });

  async function refresh() {
    try {
      const result = await runtime.sendMessage({ type: "pervue.conversations.list" });
      if (result?.ok !== true || !Array.isArray(result.value)) throw new Error("unavailable");
      const placeholder = document.createElement("option");
      placeholder.value = "";
      placeholder.textContent = result.value.length ? "Choose a conversation" : "No recent conversations";
      const options = result.value.map((/** @type {any} */ conversation) => {
        const option = document.createElement("option");
        option.value = conversation.id;
        option.textContent = conversation.title;
        return option;
      });
      select.replaceChildren(placeholder, ...options);
      select.value = view.getConversationId() ?? "";
      return result.value;
    } catch {
      return [];
    }
  }

  return { refresh };
}
