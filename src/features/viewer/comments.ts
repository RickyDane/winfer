/**
 * Review comments in markdown documents, kept in the file itself as HTML comments
 * (`<!-- @user: … -->`) so an agent reading the file sees them where they apply, and replies
 * the same way (`<!-- @agent: … -->`). The rendered view shows them as notes.
 */

export interface DocComment {
  author: string;
  body: string;
  /** Offsets of the whole `<!-- … -->` in the text. */
  start: number;
  end: number;
}

const COMMENT = /<!--\s*@([\w.-]+):\s*([\s\S]*?)\s*-->/g;
/** An HTML comment's text (as parsed, between `<!--` and `-->`) when it's a review comment. */
const BODY = /^\s*@([\w.-]+):\s*([\s\S]*?)\s*$/;

/** The author and text of an HTML comment's content, or null when it isn't a review comment. */
export function parseCommentValue(value: string): { author: string; body: string } | null {
  const m = BODY.exec(value);
  return m ? { author: m[1], body: m[2] } : null;
}

/** The review comments in `text`, in order. */
export function findComments(text: string): DocComment[] {
  return [...text.matchAll(COMMENT)].map((m) => ({ author: m[1], body: m[2], start: m.index, end: m.index + m[0].length }));
}

/** `body` made safe inside an HTML comment (it can't contain `-->`). */
const escapeBody = (body: string) => body.trim().replace(/--(!?)>/g, "-- $1>");

export const formatComment = (body: string, author = "user") => `<!-- @${author}: ${escapeBody(body)} -->`;

/**
 * `text` with a comment added on its own after line `afterLine` (1-based; 0 puts it first),
 * set apart by blank lines so markdown keeps it out of the block above.
 */
export function insertComment(text: string, afterLine: number, body: string, author = "user"): string {
  const lines = text.split("\n");
  const at = Math.max(0, Math.min(afterLine, lines.length));
  const add = [formatComment(body, author)];
  if (at > 0 && lines[at - 1].trim() !== "") add.unshift("");
  if (at < lines.length && lines[at].trim() !== "") add.push("");
  lines.splice(at, 0, ...add);
  return lines.join("\n");
}

/** `text` without the comment that starts at `start`, along with its line when it had one to itself. */
export function removeComment(text: string, start: number): string {
  const c = findComments(text).find((x) => x.start === start);
  if (!c) return text;
  const lineStart = text.lastIndexOf("\n", c.start - 1) + 1;
  const nl = text.indexOf("\n", c.end);
  const lineEnd = nl === -1 ? text.length : nl;
  const alone = text.slice(lineStart, c.start).trim() === "" && text.slice(c.end, lineEnd).trim() === "";
  if (!alone) return text.slice(0, c.start).replace(/[ \t]+$/, "") + text.slice(c.end);
  // Drop the line and one of the blank lines that set it apart.
  let from = lineStart;
  let to = nl === -1 ? text.length : nl + 1;
  const blankAfter = /^[ \t]*(\n|$)/.exec(text.slice(to));
  if (blankAfter && to < text.length) to += blankAfter[0].length;
  else if (from >= 2 && /\n[ \t]*\n$/.test(text.slice(0, from))) from = text.lastIndexOf("\n", from - 2) + 1;
  return text.slice(0, from) + text.slice(to);
}

