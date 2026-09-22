chrome.runtime.onMessage.addListener((message, _sender, sendResponse) => {
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
});
