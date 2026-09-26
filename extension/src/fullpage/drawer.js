/**
 * The full view's sidebar as a drawer in narrow windows. While it is open,
 * focus moves into it and the main column is inert, so keyboard and screen
 * reader users can't wander behind the scrim. Closing returns focus to the
 * control that opened it (or to `focusAfter`). Widening the window past the
 * breakpoint closes the drawer, so its scrim can't linger over the desktop
 * layout.
 *
 * @param {{
 *   app: HTMLElement,
 *   main: HTMLElement,
 *   scrim: HTMLElement,
 *   openButton: HTMLElement,
 *   closeButton: HTMLElement,
 *   narrow: {matches: boolean, addEventListener(type: "change", listener: () => void): void}
 * }} elements `narrow` is the media query under which the sidebar is a drawer
 * @param {{addEventListener(type: "keydown", listener: (event: any) => void): void}} keyTarget
 */
export function bindDrawer({ app, main, scrim, openButton, closeButton, narrow }, keyTarget) {
  const isOpen = () => app.getAttribute("data-sidebar") === "open";

  function open() {
    if (!narrow.matches || isOpen()) return;
    app.setAttribute("data-sidebar", "open");
    scrim.hidden = false;
    openButton.setAttribute("aria-expanded", "true");
    main.inert = true;
    closeButton.focus();
  }

  /** @param {HTMLElement | null} [focusAfter] where focus goes; null leaves it alone */
  function close(focusAfter = openButton) {
    if (!isOpen()) return;
    app.removeAttribute("data-sidebar");
    scrim.hidden = true;
    openButton.setAttribute("aria-expanded", "false");
    main.inert = false;
    focusAfter?.focus();
  }

  openButton.addEventListener("click", open);
  closeButton.addEventListener("click", () => close());
  scrim.addEventListener("click", () => close());
  keyTarget.addEventListener("keydown", (event) => {
    if (event.key === "Escape" && isOpen()) {
      event.preventDefault();
      close();
    }
  });
  narrow.addEventListener("change", () => {
    // The open button is hidden on desktop, so focus can't go back to it.
    if (!narrow.matches) close(null);
  });

  return { open, close, isOpen };
}
