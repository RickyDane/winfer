/** A background task's end note (`bg.rs` `task_event`), split for a compact one-line row. */
export interface BgNote {
  id: string;
  name: string;
  cmd: string;
  /** Exit code; null when the user stopped the task. */
  code: number | null;
  secs: number | null;
  /** The output since the agent's last read (empty when there was none). */
  output: string;
}

const EXITED = /^Background task (\S+?)(?: "([^"]*)")? \(`([^`]*)`\) exited with code (-?\d+) after (\d+)s\.\s*([\s\S]*)$/;
const STOPPED = /^The user stopped background task (\S+?)(?: "([^"]*)")? \(`([^`]*)`\)\.?\s*$/;

/** Parses a background task note; null for any other notice. */
export function parseBgNote(text: string): BgNote | null {
  const t = text.trim();
  const m = EXITED.exec(t);
  if (m) {
    const output = m[6].replace(/^Its output since your last read:\s*/, "").replace(/^No new output\.?$/, "").trim();
    return { id: m[1], name: m[2] ?? "", cmd: m[3], code: Number(m[4]), secs: Number(m[5]), output };
  }
  const s = STOPPED.exec(t);
  if (s) return { id: s[1], name: s[2] ?? "", cmd: s[3], code: null, secs: null, output: "" };
  return null;
}

/** The one-line summary: `Verify Azure SKU · exit 0 · 9s`. */
export function bgNoteTitle(n: BgNote): string {
  const label = n.name || n.cmd.split("\n")[0] || n.id;
  if (n.code === null) return `${label} · stopped`;
  return `${label} · ${n.code === 0 ? "done" : `exit ${n.code}`} · ${n.secs}s`;
}
