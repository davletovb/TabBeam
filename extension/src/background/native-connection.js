export const NATIVE_HOST_NAME = "com.pervue.host";

const TERMINAL_EVENTS = new Set([
  "response.completed",
  "response.failed",
  "request.cancelled"
]);

/**
 * @typedef {{
 *   onEvent?: (event: any) => void,
 *   onDisconnect?: (details: {requestId: string, message: string | null}) => void
 * }} RequestOwner
 */

/**
 * @typedef {{
 *   postMessage(message: any): void,
 *   disconnect(): void,
 *   onMessage: {
 *     addListener(listener: (message: any) => void): void,
 *     removeListener?(listener: (message: any) => void): void
 *   },
 *   onDisconnect: {
 *     addListener(listener: () => void): void,
 *     removeListener?(listener: () => void): void
 *   }
 * }} NativePort
 */

/**
 * @typedef {{
 *   connectNative: (hostName: string) => NativePort,
 *   hostName?: string,
 *   getLastError?: () => string | null
 * }} NativeConnectionOptions
 */

/**
 * Owns the Native Messaging port for the MV3 service worker.
 *
 * The manager is deliberately UI-agnostic. Popup/full-page requesters register
 * ownership by request_id, while the service worker keeps connection state in
 * module scope for as long as MV3 keeps the worker alive.
 */
export class NativeConnectionManager {
  /** @param {NativeConnectionOptions} options */
  constructor(options) {
    if (!options || typeof options.connectNative !== "function") {
      throw new TypeError("connectNative is required");
    }

    this.connectNative = options.connectNative;
    this.hostName = options.hostName ?? NATIVE_HOST_NAME;
    this.getLastError = options.getLastError ?? (() => null);

    /** @type {NativePort | null} */
    this.port = null;

    /** @type {Map<string, RequestOwner>} */
    this.routes = new Map();

    /** @type {Set<(event: any) => void>} */
    this.lifecycleListeners = new Set();

    /** @type {Set<(details: {message: string | null, requestIds: string[]}) => void>} */
    this.disconnectListeners = new Set();
  }

  get connected() {
    return this.port !== null;
  }

  get pendingRequestCount() {
    return this.routes.size;
  }

  /**
   * Connects only when work actually needs the native host.
   * @returns {NativePort}
   */
  ensurePort() {
    if (this.port !== null) {
      return this.port;
    }

    const port = this.connectNative(this.hostName);
    if (!port || typeof port.postMessage !== "function") {
      throw new Error("connectNative did not return a valid port");
    }

    const onMessage = (/** @type {any} */ message) => {
      if (this.port !== port) {
        return;
      }
      this.handleMessage(message);
    };

    const onDisconnect = () => {
      if (this.port !== port) {
        return;
      }
      this.handleDisconnect(this.getLastError());
    };

    port.onMessage.addListener(onMessage);
    port.onDisconnect.addListener(onDisconnect);
    this.port = port;
    return port;
  }

  /**
   * Sends one protocol request and binds subsequent correlated events to its
   * owner until a terminal event arrives or the native port disconnects.
   *
   * @param {any} request
   * @param {RequestOwner} [owner]
   */
  send(request, owner = {}) {
    const requestId = request?.request_id;

    if (typeof requestId !== "string" || requestId.length === 0) {
      throw new TypeError("request.request_id must be a non-empty string");
    }

    if (this.routes.has(requestId)) {
      throw new Error(`request_id already in flight: ${requestId}`);
    }

    const port = this.ensurePort();
    this.routes.set(requestId, owner);

    try {
      port.postMessage(request);
    } catch (error) {
      if (this.port === port) {
        this.handleDisconnect(
          error instanceof Error ? error.message : String(error)
        );
        try {
          port.disconnect();
        } catch {
          // The port may already be closed. The manager state is already reset.
        }
      }
      throw error;
    }
  }

  /**
   * @param {(event: any) => void} listener
   * @returns {() => void}
   */
  onLifecycleEvent(listener) {
    this.lifecycleListeners.add(listener);
    return () => {
      this.lifecycleListeners.delete(listener);
    };
  }

  /**
   * @param {(details: {message: string | null, requestIds: string[]}) => void} listener
   * @returns {() => void}
   */
  onDisconnect(listener) {
    this.disconnectListeners.add(listener);
    return () => {
      this.disconnectListeners.delete(listener);
    };
  }

  /**
   * Explicitly drops the current native port and pending routes.
   * The next send() reconnects lazily.
   *
   * @param {string | null} [message]
   */
  disconnect(message = null) {
    const port = this.port;
    if (port === null) {
      return;
    }

    this.handleDisconnect(message);
    port.disconnect();
  }

  /** @param {any} event */
  handleMessage(event) {
    const requestId = event?.request_id;

    if (requestId === null) {
      for (const listener of this.lifecycleListeners) {
        listener(event);
      }
      return;
    }

    if (typeof requestId !== "string") {
      return;
    }

    const owner = this.routes.get(requestId);
    if (!owner) {
      return;
    }

    try {
      owner.onEvent?.(event);
    } finally {
      if (TERMINAL_EVENTS.has(event?.event)) {
        this.routes.delete(requestId);
      }
    }
  }

  /** @param {string | null} message */
  handleDisconnect(message) {
    const requestIds = [...this.routes.keys()];
    const owners = [...this.routes.entries()];

    this.port = null;
    this.routes.clear();

    for (const [requestId, owner] of owners) {
      owner.onDisconnect?.({ requestId, message });
    }

    const details = { message, requestIds };
    for (const listener of this.disconnectListeners) {
      listener(details);
    }
  }
}

/** @param {NativeConnectionOptions} options */
export function createNativeConnectionManager(options) {
  return new NativeConnectionManager(options);
}
