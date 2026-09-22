chrome.runtime.onMessage.addListener(
  (
    /** @type {any} */ message,
    /** @type {any} */ _sender,
    /** @type {(response: any) => void} */ sendResponse
  ) => {
    if (message?.type === "pervue.ping") {
      sendResponse({
        ok: true,
        surface: "content",
        page: {
          title: document.title,
          url: window.location.href
        }
      });
    }
  }
);

export {};
