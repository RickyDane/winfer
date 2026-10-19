/**
 * What an agent waiting for input is asking, read from the bottom of its screen, for the
 * notification: a permission prompt (approve / decline) or an open question (answer it).
 */

export interface RequestOption {
  /** The number that picks it (typing it into the pane selects the option). */
  n: number;
  label: string;
  /** The line under the option, when the TUI shows one. */
  detail?: string;
}

export interface AgentRequest {
  kind: "approval" | "question";
  /** The question as the agent shows it, condensed to a few lines. */
  text: string;
  /** The numbered choices under the question (Claude Code's and winfer code's pickers). */
  options?: RequestOption[];
}

// Permission prompts across CLIs: Claude ("Do you want to proceed? ❯ 1. Yes"), Codex
// ("Allow command? … Yes (y)"), and generic y/n prompts.
const APPROVAL = /\b(do you want to|allow|approve|permission|proceed\?|run this command|make this edit)|[([]y\/n[)\]]|^\s*❯?\s*1\.\s*yes\b|\byes \(y\)/im;

/** Box-drawing, prompt markers and other chrome around TUI dialogs. */
const CHROME = /[─━═│┃╭╮╰╯┌┐└┘├┤┬┴┼▌▐█░]/g;

function clean(line: string): string {
  return line.replace(CHROME, " ").replace(/\s+/g, " ").trim();
}

const OPTION = /^(?:[❯>›▶]\s*)?(?:[☐☑✓✔◯◉●○]\s*)?(\d{1,2})[.)]\s+(.+)$/;
/** Key hints under a picker ("Enter to select · ↑/↓ to navigate · Esc to cancel"). */
const FOOTER = /enter to (select|confirm|submit)|esc to (cancel|go back)|↑\/↓|tab to |ctrl\+/i;

/** The last run of numbered options (1, 2, 3…) in `lines`, with the index of its first line. */
export function readOptions(lines: string[]): { options: RequestOption[]; start: number } | null {
  let best: { options: RequestOption[]; start: number } | null = null;
  for (let i = 0; i < lines.length; i++) {
    const m = lines[i].match(OPTION);
    if (!m || Number(m[1]) !== 1) continue;
    const options: RequestOption[] = [];
    let j = i;
    while (j < lines.length) {
      const o = lines[j].match(OPTION);
      if (o && Number(o[1]) === options.length + 1) {
        options.push({ n: options.length + 1, label: o[2].trim() });
        j++;
        continue;
      }
      // One description line per option; anything else ends the list.
      const last = options[options.length - 1];
      if (last && !last.detail && lines[j].length > 2 && !FOOTER.test(lines[j]) && !/\?\s*$/.test(lines[j])) {
        last.detail = lines[j];
        j++;
        continue;
      }
      break;
    }
    if (options.length >= 2) best = { options, start: i };
    i = j - 1;
  }
  return best;
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
  const picker = readOptions(lines);
  const before = picker ? lines.slice(0, picker.start) : lines;
  let q = -1;
  before.forEach((l, i) => {
    if (/\?\s*$/.test(l) || /\?\s*\(/.test(l)) q = i;
  });
  // A picker's prompt needn't end in "?" ("Pick a library"): take the line above the options.
  if (q < 0 && picker && picker.start > 0) q = picker.start - 1;
  let text: string;
  if (q >= 0) {
    const context = lines.slice(Math.max(0, q - 3), q).filter((l) => !/^(\d+\.|❯|>)\s/.test(l));
    text = [...context, lines[q]].join("\n");
  } else {
    text = hookMessage ?? lines.slice(-3).join("\n");
  }
  if (hookMessage && !text.includes(hookMessage) && q < 0) text = hookMessage;
  const req: AgentRequest = { kind, text: text.length > 280 ? `${text.slice(0, 277)}…` : text };
  if (picker) req.options = picker.options.map((o) => ({ ...o, label: o.label.slice(0, 120), ...(o.detail ? { detail: o.detail.slice(0, 160) } : {}) }));
  return req;
}
