// String helpers used by the CMS templates.

export function capitalize(s) {
  return s.length ? s[0].toUpperCase() + s.slice(1) : s;
}

// TODO #1: truncate(s, max) — shorten `s` to at most `max` characters, ending with "…"
// when it was cut. Should not cut in the middle of a word if a space is near.
export function truncate(s, max) {
  throw new Error("truncate: not implemented");
}

// TODO #2: wordWrap(text, width) — wrap `text` into lines of at most `width` characters.
// Words are separated by single spaces in the input and are never split, unless a single
// word is longer than `width`: then it goes on its own line, unsplit. Lines are joined
// with "\n" and have no leading/trailing spaces. An empty string returns "".
export function wordWrap(text, width) {
  if (text === "") return "";
  const lines = [];
  let line = "";
  for (const word of text.split(" ")) {
    if (line === "") line = word;
    else if (line.length + 1 + word.length <= width) line += " " + word;
    else {
      lines.push(line);
      line = word;
    }
  }
  lines.push(line);
  return lines.join("\n");
}

// TODO #3: initials(name) — "ada lovelace" -> "A.L."; handle hyphenated names?
export function initials(name) {
  throw new Error("initials: not implemented");
}
