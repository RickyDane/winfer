/** Max height (px) the web composer textarea grows to before it scrolls. */
export const COMPOSER_MAX_HEIGHT = 220;

/**
 * Inline height for the composer textarea, or null to clear the inline height.
 *
 * Empty text always clears: `rows={1}` plus the CSS min-height give the
 * single-line height. A measured `scrollHeight` for empty text cannot be
 * trusted — it counts the placeholder, which wraps to many lines when the
 * pane mounts hidden or narrow (and engines differ on this), so a clean
 * session could open with a composer up to COMPOSER_MAX_HEIGHT tall and
 * nothing later re-measured it while the text stayed empty.
 */
export function composerHeight(text: string, scrollHeight: number): number | null {
  if (!text) return null;
  return Math.min(Math.max(scrollHeight, 0), COMPOSER_MAX_HEIGHT);
}
