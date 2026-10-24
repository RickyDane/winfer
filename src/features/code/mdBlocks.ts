/**
 * Markdown cut into top-level blocks that render the same on their own, so a reply that's
 * streaming in re-renders only its last block instead of re-parsing all of it on every token.
 *
 * It cuts only at a blank line outside code fences that's followed by an unindented line which
 * doesn't start a list item: nothing before such a line can continue past it (a list can go on
 * after a blank line, and indented lines may belong to the block above). Text with link or
 * footnote definitions stays whole, since those apply across blocks.
 */

const LIST_ITEM = /^([*+-]|\d{1,9}[.)])(\s|$)/;
const DEFINITION = /^ {0,3}\[[^\]\n]+\]:/m;
/** A backtick fence's info string has no backticks (else it's inline code). */
const FENCE_OPEN = /^[ \t]*(?:(`{3,})[^`]*|(~{3,}).*)$/;
const FENCE_CLOSE = /^[ \t]*(`{3,}|~{3,})[ \t]*$/;

export function splitBlocks(text: string): string[] {
  if (DEFINITION.test(text)) return [text];
  const lines = text.split("\n");
  const blocks: string[] = [];
  let start = 0;
  let fence: { ch: string; len: number } | null = null;
  let afterBlank = false;
  for (let i = 0; i < lines.length; i++) {
    const line = lines[i];
    if (fence) {
      const close = FENCE_CLOSE.exec(line);
      if (close && close[1][0] === fence.ch && close[1].length >= fence.len) fence = null;
      continue;
    }
    if (!line.trim()) {
      afterBlank = true;
      continue;
    }
    if (afterBlank && i > start && !/^\s/.test(line) && !LIST_ITEM.test(line)) {
      blocks.push(lines.slice(start, i).join("\n"));
      start = i;
    }
    afterBlank = false;
    const open = FENCE_OPEN.exec(line);
    const run = open?.[1] ?? open?.[2];
    if (run) fence = { ch: run[0], len: run.length };
  }
  blocks.push(lines.slice(start).join("\n"));
  return blocks;
}
