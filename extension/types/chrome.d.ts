interface PervueChromeEvent<TCallback> {
  addListener(callback: TCallback): void;
  removeListener?(callback: TCallback): void;
}

interface PervueChromePort {
  postMessage(message: any): void;
  disconnect(): void;
  onMessage: PervueChromeEvent<(message: any) => void>;
  onDisconnect: PervueChromeEvent<() => void>;
}

interface PervueChromeRuntimePort extends PervueChromePort {
  name: string;
  sender?: {
    url?: string;
  };
}

interface PervueChrome {
  runtime: {
    onInstalled: PervueChromeEvent<() => void | Promise<void>>;
    onConnect: PervueChromeEvent<(port: PervueChromeRuntimePort) => void>;
    onMessage: PervueChromeEvent<
      (
        message: any,
        sender: any,
        sendResponse: (response: any) => void
      ) => void | boolean
    >;
    getURL(path: string): string;
    getManifest(): { version: string };
    sendMessage(message: any): Promise<any>;
    connect(connectInfo: { name: string }): PervueChromeRuntimePort;
    connectNative(hostName: string): PervueChromePort;
    lastError?: {
      message?: string;
    };
  };
  commands: {
    onCommand: PervueChromeEvent<
      (command: string) => void | Promise<void>
    >;
  };
  contextMenus: {
    onClicked: PervueChromeEvent<
      (info: { menuItemId: string | number }) => void | Promise<void>
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
  };
}

declare var chrome: PervueChrome;
