interface PervueChrome {
  runtime: {
    onInstalled: {
      addListener(callback: () => void | Promise<void>): void;
    };
    onMessage: {
      addListener(
        callback: (
          message: any,
          sender: any,
          sendResponse: (response: any) => void
        ) => void | boolean
      ): void;
    };
    getURL(path: string): string;
    getManifest(): { version: string };
    sendMessage(message: any): Promise<any>;
  };
  commands: {
    onCommand: {
      addListener(callback: (command: string) => void | Promise<void>): void;
    };
  };
  contextMenus: {
    onClicked: {
      addListener(
        callback: (info: { menuItemId: string | number }) => void | Promise<void>
      ): void;
    };
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
