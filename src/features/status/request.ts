/**
 * What an agent waiting for input is asking, read from the bottom of its screen, for the
 * notification: a permission prompt (approve / decline) or an open question (answer it).
 */

export interface AgentRequest {
  kind: "approval" | "question";
  /** The question as the agent shows it, condensed to a few lines. */
  text: string;
}

// Permission prompts across CLIs: Claude ("Do you want to proceed? ❯ 1. Yes"), Codex
// ("Allow command? … Yes (y)"), and generic y/n prompts.
const APPROVAL = /\b(do you want to|allow|approve|permission|proceed\?|run this command|make this edit)|[([]y\/n[)\]]|^\s*❯?\s*1\.\s*yes\b|\byes \(y\)/im;

/** Box-drawing, prompt markers and other chrome around TUI dialogs. */
const CHROME = /[─━═│┃╭╮╰╯┌┐└┘├┤┬┴┼▌▐█░]/g;

function clean(line: string): string {
  return line.replace(CHROME, " ").replace(/\s+/g, " ").trim();
}

export function describeRequest(screen: string, hookMessage?: string): AgentRequest {
  const lines = screen
    .split("\n")
    .map(clean)
    .filter(Boolean)
    .slice(-30);
  const tail = lines.join("\n");
  const kind: AgentRequest["kind"] = APPROVAL.test(tail) || (hookMessage ? /permission|approv/i.test(hookMessage) : false) ? "approval" : "question";

  // The question is the last line ending in "?"; the few lines above it say what it's about
  // (the command or file). Option lists below it ("1. Yes", "2. No") are left out.
  let q = -1;
  lines.forEach((l, i) => {
    if (/\?\s*$/.test(l) || /\?\s*\(/.test(l)) q = i;
  });
  let text: string;
  if (q >= 0) {
    const context = lines.slice(Math.max(0, q - 3), q).filter((l) => !/^(\d+\.|❯|>)\s/.test(l));
    text = [...context, lines[q]].join("\n");
  } else {
    text = hookMessage ?? lines.slice(-3).join("\n");
  }
  if (hookMessage && !text.includes(hookMessage) && q < 0) text = hookMessage;
  return { kind, text: text.length > 280 ? `${text.slice(0, 277)}…` : text };
}
