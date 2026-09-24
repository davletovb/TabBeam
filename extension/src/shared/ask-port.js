/**
 * Contract between a UI page and the service worker for one question.
 *
 * The page opens one runtime port named ASK_PORT_NAME per question and posts
 * a single `{type: "ask", text}` message. The service worker answers with that
 * request's protocol v1 events in order (docs/protocol/v1.md §7), ending with
 * exactly one terminal event, and then closes the port. Failures the native
 * host cannot report itself, such as an unreachable host, arrive as a
 * `response.failed` event in the DOC-02 error vocabulary.
 */
export const ASK_PORT_NAME = "pervue.ask";

/** Events that end a `conversation.send` request. */
export const ASK_TERMINAL_EVENTS = new Set([
  "response.completed",
  "response.failed"
]);

/**
 * @typedef {{
 *   postMessage(message: any): void,
 *   disconnect(): void,
 *   onMessage: { addListener(listener: (message: any) => void): void },
 *   onDisconnect: { addListener(listener: () => void): void }
 * }} AskPort
 */
