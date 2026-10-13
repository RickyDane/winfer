/**
 * Executes tool calls from the winfer MCP server (src-tauri/src/mcp.rs) that agents use to
 * coordinate: every call is scoped to the calling pane's workspace.
 */
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { leaves } from "@/features/layout/tree";
import { openUrl } from "@/features/browser/browser";
import { reportStatus } from "@/features/status/agentStatus";
import { pasteText, readTerminal, sendText, startDetached } from "@/features/terminal/xtermRegistry";
import { providerName, useWorkspaces, type Pane, type Workspace } from "@/stores/workspaces";

interface Call {
  id: string;
  tool: string;
  args: Record<string, unknown>;
  paneId: string;
}

class ToolError extends Error {}

function order(ws: Workspace): Pane[] {
  return ws.layout ? leaves(ws.layout).map((id) => ws.panes.find((p) => p.id === id)!) : [];
}

function resolve(ws: Workspace, ref: unknown): Pane {
  const panes = order(ws);
  const n = typeof ref === "number" ? ref : /^\d+$/.test(String(ref)) ? Number(ref) : NaN;
  const pane = Number.isNaN(n) ? panes.find((p) => p.id === ref) : panes[n - 1];
  if (!pane) throw new ToolError(`No pane ${String(ref)} in this workspace. Use list_panes to see them.`);
  return pane;
}

const describe = (ws: Workspace, pane: Pane) => `pane ${order(ws).indexOf(pane) + 1} (${providerName(useWorkspaces.getState().providers, pane.provider)})`;

export async function runTool(call: Call): Promise<string> {
  const s = useWorkspaces.getState();
  const ws = s.workspaces.find((w) => w.panes.some((p) => p.id === call.paneId));
  if (!ws) throw new ToolError("The calling pane isn't open in winfer anymore.");
  const self = ws.panes.find((p) => p.id === call.paneId)!;
  const a = call.args ?? {};

  switch (call.tool) {
    case "list_panes": {
      const rows = order(ws).map((p, i) => {
        const parts = [`${i + 1}. ${providerName(s.providers, p.provider)}`, p.status];
        if (p.worktree) parts.push(`branch ${p.worktree.branch}`);
        if (p.id === self.id) parts.push("(you)");
        return parts.join(" · ");
      });
      return `Workspace “${ws.name}” (${ws.rootDir}):\n${rows.join("\n")}`;
    }
    case "read_pane": {
      const pane = resolve(ws, a.pane);
      const lines = Math.min(400, Math.max(1, Number(a.lines ?? 80)));
      const text = readTerminal(pane.id, lines);
      if (text === null) throw new ToolError(`${describe(ws, pane)} hasn't started yet.`);
      return text || "(no output yet)";
    }
    case "send_to_pane": {
      const pane = resolve(ws, a.pane);
      if (pane.id === self.id) throw new ToolError("That's your own pane.");
      const text = String(a.text ?? "");
      if (!text) throw new ToolError("text is empty");
      if (text.includes("\n")) pasteText(pane.id, text);
      else sendText(pane.id, text);
      if (a.submit !== false) sendText(pane.id, "\r");
      return `Sent to ${describe(ws, pane)}.`;
    }
    case "spawn_agent": {
      const provider = s.providers.find((p) => p.id === a.provider && p.installed);
      if (!provider) {
        const available = s.providers.filter((p) => p.installed).map((p) => p.id).join(", ");
        throw new ToolError(`Unknown or uninstalled provider “${String(a.provider)}”. Available: ${available}.`);
      }
      const id = s.addPane(provider.id, {
        workspaceId: ws.id,
        prompt: typeof a.prompt === "string" ? a.prompt : undefined,
        worktree: a.worktree === undefined ? undefined : Boolean(a.worktree),
        splitFrom: self.id,
      });
      const fresh = useWorkspaces.getState().workspaces.find((w) => w.id === ws.id)!;
      const pane = fresh.panes.find((p) => p.id === id)!;
      // Panes in a workspace the user isn't looking at start immediately, off-screen.
      if (ws.id !== useWorkspaces.getState().activeWorkspaceId) startDetached(pane);
      return `Started ${provider.name} as pane ${order(fresh).indexOf(pane) + 1}.`;
    }
    case "open_url": {
      const url = String(a.url ?? "");
      if (!url) throw new ToolError("url is empty");
      if (ws.id === s.activeWorkspaceId) await openUrl(url, { newTab: Boolean(a.new_tab) });
      else await invoke("browser_new_tab", { workspaceId: ws.id, url });
      return `Opened ${url} in the browser panel.`;
    }
    case "notify_user": {
      reportStatus(self.id, "needs-input", String(a.message ?? "An agent needs you"));
      return "The user has been notified.";
    }
    default:
      throw new ToolError(`Unknown tool ${call.tool}`);
  }
}

let started = false;
export function startAgentApi() {
  if (started) return;
  started = true;
  void listen<Call>("mcp://call", async ({ payload }) => {
    let reply: { ok: boolean; text: string };
    try {
      reply = { ok: true, text: await runTool(payload) };
    } catch (err) {
      reply = { ok: false, text: err instanceof Error ? err.message : String(err) };
    }
    void invoke("mcp_respond", { id: payload.id, reply });
  });
}
