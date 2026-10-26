/** Keep startup independent of the size of a restored session. Older messages are
 * still in the store, and can be revealed a page at a time without losing row state. */
export const HISTORY_PAGE = 40;

export function historyStart(length: number, previous?: number, following = false): number {
  if (previous === undefined || previous >= length || (following && length - previous > HISTORY_PAGE * 2))
    return Math.max(0, length - HISTORY_PAGE);
  // Do not slide the window on every new message: the user may be reading its top.
  return previous;
}
