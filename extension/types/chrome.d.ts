declare namespace chrome {
  namespace runtime {
    interface Manifest {
      version: string;
    }

    const onInstalled: {
      addListener(callback: () => void | Promise<void>): void;
    };

    const onMessage: {
      addListener(
        callback: (
          message: any,
          sender: any,
          sendResponse: (response: any) => void
        ) => void | boolean
      ): void;
    };

    function getURL(path: string): string;
    function getManifest(): Manifest;
    function sendMessage(message: any): Promise<any>;
  }

  namespace commands {
    const onCommand: {
      addListener(callback: (command: string) => void | Promise<void>): void;
    };
  }

  namespace contextMenus {
    interface OnClickData {
      menuItemId: string | number;
    }

    const onClicked: {
      addListener(callback: (info: OnClickData) => void | Promise<void>): void;
    };

    function removeAll(): Promise<void>;
    function create(properties: {
      id: string;
      title: string;
      contexts: string[];
    }): string | number;
  }

  namespace tabs {
    function create(properties: { url: string }): Promise<unknown>;
  }
}
