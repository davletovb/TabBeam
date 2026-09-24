export class MockEvent {
  constructor() {
    /** @type {Set<(value: any) => void>} */
    this.listeners = new Set();
  }

  /** @param {(value: any) => void} listener */
  addListener(listener) {
    this.listeners.add(listener);
  }

  /** @param {(value: any) => void} listener */
  removeListener(listener) {
    this.listeners.delete(listener);
  }

  /** @param {any} value */
  emit(value) {
    for (const listener of [...this.listeners]) {
      listener(value);
    }
  }
}

/**
 * A runtime or native port. Like Chrome's, it throws when used after either
 * side has disconnected it, and a local disconnect() does not fire the local
 * onDisconnect.
 */
export class MockPort {
  /**
   * @param {string} [name]
   * @param {{url?: string}} [sender]
   */
  constructor(name = "", sender = undefined) {
    this.name = name;
    this.sender = sender;
    /** @type {any[]} */
    this.messages = [];
    this.disconnectCalls = 0;
    this.disconnected = false;
    this.throwOnPost = false;
    this.onMessage = new MockEvent();
    this.onDisconnect = new MockEvent();
  }

  /** @param {any} message */
  postMessage(message) {
    if (this.disconnected || this.throwOnPost) {
      throw new Error("Attempting to use a disconnected port object");
    }
    this.messages.push(message);
  }

  disconnect() {
    this.disconnectCalls += 1;
    this.disconnected = true;
  }

  /**
   * Delivers a message from the other side.
   * @param {any} message
   */
  emitMessage(message) {
    this.onMessage.emit(message);
  }

  /** The other side disconnects. */
  emitDisconnect() {
    this.disconnected = true;
    this.onDisconnect.emit(undefined);
  }
}
