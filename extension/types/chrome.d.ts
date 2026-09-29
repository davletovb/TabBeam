interface TabBeamChromeEvent<TCallback> {
  addListener(callback: TCallback): void;
  removeListener?(callback: TCallback): void;
}

interface TabBeamChromePort {
  postMessage(message: any): void;
  disconnect(): void;
  onMessage: TabBeamChromeEvent<(message: any) => void>;
  onDisconnect: TabBeamChromeEvent<() => void>;
}

interface TabBeamChromeRuntimePort extends TabBeamChromePort {
  name: string;
  sender?: {
    url?: string;
  };
}

interface TabBeamChrome {
  runtime: {
    onInstalled: TabBeamChromeEvent<() => void | Promise<void>>;
    onConnect: TabBeamChromeEvent<(port: TabBeamChromeRuntimePort) => void>;
    onMessage: TabBeamChromeEvent<
      (
        message: any,
        sender: any,
        sendResponse: (response: any) => void
      ) => void | boolean
    >;
    getURL(path: string): string;
    getManifest(): { version: string };
    getPlatformInfo(): Promise<{os: string; arch: string}>;
    sendMessage(message: any): Promise<any>;
    connect(connectInfo: { name: string }): TabBeamChromeRuntimePort;
    connectNative(hostName: string): TabBeamChromePort;
    lastError?: {
      message?: string;
    };
  };
  commands: {
    onCommand: TabBeamChromeEvent<
      (command: string) => void | Promise<void>
    >;
  };
  contextMenus: {
    onClicked: TabBeamChromeEvent<
      (info: { menuItemId: string | number; selectionText?: string }, tab?: { id?: number; url?: string; title?: string }) => void | Promise<void>
    >;
    removeAll(): Promise<void>;
    create(properties: {
      id: string;
      title: string;
      contexts: string[];
    }): string | number;
  };
  tabs: {
    create(properties: { url: string }): Promise<unknown>;
    query(query: { active: boolean; currentWindow: boolean }): Promise<{ id?: number; url?: string; title?: string }[]>;
    sendMessage(tabId: number, message: any, options: { frameId: number }): Promise<any>;
  };
  action: {
    openPopup(): Promise<void>;
  };
  storage: {
    local: {
      get(key: string): Promise<Record<string, any>>;
      set(values: Record<string, any>): Promise<void>;
      remove(key: string): Promise<void>;
    };
    session: {
      get(key: string): Promise<Record<string, any>>;
      set(values: Record<string, any>): Promise<void>;
      remove(key: string): Promise<void>;
    };
    onChanged: { addListener(callback: (changes: Record<string, any>, area: string) => void): void };
  };
}

declare var chrome: TabBeamChrome;
