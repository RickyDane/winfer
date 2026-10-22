/** The window an element is in: the undocked browser window's, or the main one. Listeners, frame timing and the clipboard go there. */
export const viewOf = (el: Element | null | undefined) => (el?.ownerDocument.defaultView ?? window) as Window & typeof globalThis;
