const SUFFIXES = new Set(["fast", "minimal", "low", "medium", "high", "xhigh", "max", "latest", "preview", "thinking"]);
const UPPER = new Set(["gpt", "glm"]);

/** Human-friendly model name: drops the provider and speed/effort/date suffixes.
 *  `codex/gpt-6.1-sol-fast` → `GPT 6.1 Sol`, `claude-opus-4-7` → `Opus 4.7`. */
export function displayModelName(model: string): string {
  const tokens = model.slice(model.lastIndexOf("/") + 1).replace(/\[[^\]]*\]/g, "").split(/[-_\s]+/).filter(Boolean);
  while (tokens.length > 1) {
    const last = tokens[tokens.length - 1].toLowerCase();
    if (SUFFIXES.has(last) || /^\d{6,}$/.test(last)) tokens.pop();
    else break;
  }
  // Claude models go by their family name: claude-opus-4-7 → Opus 4.7, claude-3-5-haiku → Haiku 3.5.
  if (tokens[0]?.toLowerCase() === "claude" && tokens.slice(1).some((t) => !/^\d/.test(t))) {
    tokens.shift();
    const name = tokens.findIndex((t) => !/^\d/.test(t));
    if (name > 0) tokens.unshift(...tokens.splice(name, 1));
  }
  const words: string[] = [];
  for (const t of tokens) {
    const prev = words[words.length - 1];
    // Version parts split by dashes (opus-4-7) become dotted (4.7).
    if (/^\d+$/.test(t) && prev && /^\d+(\.\d+)*$/.test(prev)) words[words.length - 1] = `${prev}.${t}`;
    else if (UPPER.has(t.toLowerCase())) words.push(t.toUpperCase());
    else words.push(/^[a-z]/.test(t) && !/^o\d/.test(t) ? t[0].toUpperCase() + t.slice(1) : t);
  }
  return words.join(" ") || model;
}
