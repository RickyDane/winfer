/** The composer's slash commands: what `tui::app` does for each, mapped to the web UI. */
import type { StoreApi } from "zustand";
import type { CodeState, Item } from "./store";
import { setPlanSidebar, usePlanSidebar } from "./planSidebar";
import { INIT_PROMPT, MODES, REVIEW_PROMPT, type Mode } from "./protocol";

let seq = 0;

function exportMarkdown(s: CodeState) {
  let md = `# ${s.title || "winfer code session"}\n\n`;
  for (const it of s.items as Item[]) {
    if (it.kind === "user") md += `## You\n\n${it.text}\n\n`;
    else if (it.kind === "text") md += `## Assistant\n\n${it.text}\n\n`;
    else if (it.kind === "tool") {
      md += `**${it.name}** \`${it.title}\`\n\n`;
      if (it.diff) md += "```diff\n" + it.diff + "\n```\n\n";
      else if (it.output.trim()) md += "```\n" + it.output.trim().slice(0, 4000) + "\n```\n\n";
    }
  }
  return md;
}

/** Runs `text` if it's a command; false means it's a message to send. */
export function runCommand(text: string, store: StoreApi<CodeState>): boolean {
  const t = text.trim();
  if (!t.startsWith("/")) return false;
  const [cmd, ...rest] = t.split(/\s+/);
  const arg = rest.join(" ");
  const s = store.getState();
  const notice = (text: string) => s.apply({ type: "notice", text });
  switch (cmd) {
    case "/model":
      s.setSheet("model");
      return true;
    case "/effort":
      if (arg) s.send({ type: "set_effort", effort: arg === "default" ? null : arg });
      else s.setSheet("effort");
      return true;
    case "/mode":
      if (arg && MODES.some((m) => m.id === arg)) s.send({ type: "set_mode", mode: arg as Mode });
      else s.setSheet("mode");
      return true;
    case "/new":
    case "/clear":
    case "/reset":
      s.send({ type: "new" });
      return true;
    case "/resume":
    case "/sessions":
      s.send({ type: "sessions" });
      s.setSheet("sessions");
      return true;
    case "/btw":
      if (arg) s.send({ type: "btw", id: `btw-${++seq}`, question: arg });
      else s.setSheet("btw");
      return true;
    case "/compact":
      if (s.busy) notice("Wait for the turn to finish.");
      else s.send({ type: "compact" });
      return true;
    case "/compaction":
    case "/autocompact":
      s.setSheet("compaction");
      return true;
    case "/undo":
    case "/revert":
      s.send({ type: "undo", files: true });
      return true;
    case "/init":
      s.submit(INIT_PROMPT);
      return true;
    case "/review":
      s.submit(REVIEW_PROMPT);
      return true;
    case "/diff":
      s.send({ type: "diff" });
      return true;
    case "/cost":
    case "/usage":
    case "/context":
      s.setSheet("usage");
      return true;
    case "/copy": {
      const last = [...s.items].reverse().find((i) => i.kind === "text");
      if (!last || last.kind !== "text") notice("Nothing to copy yet.");
      else void navigator.clipboard.writeText(last.text).then(() => notice("Copied the last reply."));
      return true;
    }
    case "/export":
      s.send({ type: "export", markdown: exportMarkdown(s) });
      return true;
    case "/agents":
    case "/subagents":
    case "/tasks":
    case "/shells":
      s.setSheet("agents");
      return true;
    case "/mcp":
      s.setSheet("mcp");
      return true;
    case "/status":
      s.setSheet("status");
      return true;
    case "/login":
      s.setSheet("login");
      return true;
    case "/help":
      s.setSheet("help");
      return true;
    case "/sidebar": {
      const on = arg === "on" ? true : arg === "off" ? false : !usePlanSidebar.getState().on;
      setPlanSidebar(on).catch((e) => notice(`Couldn't save the plan sidebar setting: ${e}`));
      notice(on ? "Plan sidebar on (shows when the pane is wide enough)" : "Plan sidebar off");
      return true;
    }
    default:
      notice(`Unknown command ${cmd} — try /help`);
      return true;
  }
}
