// macOS Option key in the terminal. Panes treat Option as Meta (macOptionIsMeta), so
// Option+B/F/Backspace move and delete by word. But on many keyboard layouts Option is also
// how you type basic characters: German, Swiss, French, Nordic and others type @ [ ] { } | \ ~
// with it (Option+L is "@" on a German Mac). Those must reach the program as the character,
// not as Meta+key.

type Key = Pick<KeyboardEvent, "type" | "key" | "code" | "altKey" | "metaKey" | "ctrlKey">;

/** The plain key under `code` ("KeyL" → "l", "Digit5" → "5"), if it's a letter or digit. */
function baseChar(code: string): string | null {
  if (/^Key[A-Z]$/.test(code)) return code.slice(3).toLowerCase();
  if (/^Digit[0-9]$/.test(code)) return code.slice(5);
  return null;
}

/**
 * The character an Option-key press types, when it should be typed rather than sent as Meta:
 * a printable ASCII character that differs from the key's own letter or digit. Option combos
 * that make symbols like "¬" or "ƒ" (the US layout) stay Meta, so word navigation keeps working.
 */
export function optionTypedChar(e: Key): string | null {
  if (e.type !== "keydown" || !e.altKey || e.metaKey || e.ctrlKey) return null;
  const k = e.key;
  if (k.length !== 1 || k < "!" || k > "~") return null;
  const base = baseChar(e.code);
  if (base !== null && k.toLowerCase() === base) return null;
  return k;
}
