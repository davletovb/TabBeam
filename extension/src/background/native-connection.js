import { MAX_NATIVE_MESSAGE_BYTES, utf8ByteLength } from "../shared/limits.js";

export const NATIVE_HOST_NAME = "com.pervue.host";

// Protocol v1 request-ID grammar (docs/protocol/v1.md §2). The host rejects any
// other ID without being able to echo it back, so its route could never finish.
export const REQUEST_ID_PATTERN = /^[A-Za-z0-9][A-Za-z0-9._:-]{0,127}$/;

/** A request larger than the native host accepts. Nothing was sent. */
export class RequestTooLargeError extends RangeError {
  /** @param {number} bytes */
  constructor(bytes) {
    super(
      `request is ${bytes} bytes; the native host accepts at most ${MAX_NATIVE_MESSAGE_BYTES}`
    );
    this.name = "RequestTooLargeError";
    this.bytes = bytes;
  }
}

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
 *   getLastError?: () => string | null,
 *   reportError?: (context: string, error: unknown) => void
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
    this.reportError =
      options.reportError ??
      ((_context, _error) => {
        // Callback errors may contain page text in their messages or stacks.
        // Default diagnostics record the failure only, never raw content.
        console.error("Pervue native connection callback failed.");
      });

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
      // Read lastError even for a replaced port, so Chrome does not log it as
      // an unchecked runtime.lastError.
      const message = this.getLastError();
      if (this.port !== port) {
        return;
      }
      this.handleDisconnect(message);
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

    if (typeof requestId !== "string" || !REQUEST_ID_PATTERN.test(requestId)) {
      throw new TypeError(
        "request.request_id must be 1-128 characters: an ASCII letter or digit, then letters, digits, '.', '_', ':' or '-'"
      );
    }

    if (this.routes.has(requestId)) {
      throw new Error(`request_id already in flight: ${requestId}`);
    }

    // The host echoes only an ID it read before a malformed or too-deep
    // member (docs/protocol/v1.md §8.4), so request_id is serialized first.
    const message = { request_id: requestId, ...request };

    // Chrome sends JSON.stringify(message) as UTF-8. The host closes the whole
    // connection on a frame over its limit, failing every request in flight,
    // so an oversized or unserializable request fails here, on its own.
    const bytes = utf8ByteLength(JSON.stringify(message));
    if (bytes > MAX_NATIVE_MESSAGE_BYTES) {
      throw new RequestTooLargeError(bytes);
    }

    const port = this.ensurePort();
    this.routes.set(requestId, owner);

    try {
      port.postMessage(message);
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
   * Stops routing a request its owner no longer waits for: the owner hears
   * nothing more about it, and the request's later events are dropped. The
   * host isn't told, so the request runs to its own end there.
   *
   * @param {string} requestId
   * @returns {boolean} whether the request was in flight
   */
  forget(requestId) {
    return this.routes.delete(requestId);
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

    try {
      this.handleDisconnect(message);
    } finally {
      port.disconnect();
    }
  }

  /** @param {any} event */
  handleMessage(event) {
    const requestId = event?.request_id;

    if (requestId === null) {
      for (const listener of this.lifecycleListeners) {
        this.invokeCallback("lifecycle listener", () => listener(event));
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

    // Retire a finished request before its owner runs, so the owner can reuse
    // the ID or disconnect without the request still counting as in flight.
    if (TERMINAL_EVENTS.has(event?.event)) {
      this.routes.delete(requestId);
    }
    this.invokeCallback("request event handler", () => owner.onEvent?.(event));
  }

  /** @param {string | null} message */
  handleDisconnect(message) {
    const requestIds = [...this.routes.keys()];
    const owners = [...this.routes.entries()];

    this.port = null;
    this.routes.clear();

    for (const [requestId, owner] of owners) {
      this.invokeCallback("request disconnect handler", () =>
        owner.onDisconnect?.({ requestId, message })
      );
    }

    const details = { message, requestIds };
    for (const listener of this.disconnectListeners) {
      this.invokeCallback("disconnect listener", () => listener(details));
    }
  }

  /**
   * Runs requester or listener code so that a throw cannot skip the other
   * callbacks or the manager's own cleanup. Failures are reported, not hidden.
   *
   * @param {string} context
   * @param {() => void} callback
   */
  invokeCallback(context, callback) {
    try {
      callback();
    } catch (error) {
      this.reportError(context, error);
    }
  }
}

/** @param {NativeConnectionOptions} options */
export function createNativeConnectionManager(options) {
  return new NativeConnectionManager(options);
}
