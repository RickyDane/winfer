/** Turns what the user typed into a URL: localhost → http, bare domains → https, else a search. */
export function normalizeUrl(input: string): string {
  const text = input.trim();
  if (/^[a-z][a-z0-9+.-]*:/i.test(text) && !/^(localhost|[\w.-]+):\d+/i.test(text)) return text;
  if (/^(localhost|127\.0\.0\.1|0\.0\.0\.0|\[::1\])(:\d+)?(\/|$)/i.test(text)) return `http://${text}`;
  if (/^[\w-]+(\.[\w-]+)+(:\d+)?(\/.*)?$/.test(text) && !text.includes(" ")) return `https://${text}`;
  return `https://www.google.com/search?q=${encodeURIComponent(text)}`;
}
