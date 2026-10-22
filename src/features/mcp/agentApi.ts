/**
 * Executes tool calls from the winfer MCP server (src-tauri/src/mcp.rs) that agents use to
 * coordinate: every call is scoped to the calling pane's workspace.
 */
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { leaves } from "@/features/layout/tree";
import { markAgentOpened, markAgentTab, openUrl, shownWorkspaceId, tabLabel, useBrowser, type Opened } from "@/features/browser/browser";
import { relatedTab } from "@/features/browser/related";
import { normalizeUrl } from "@/features/browser/url";
import { reportStatus } from "@/features/status/agentStatus";
import { openFile, resolveFiles } from "@/features/viewer/viewer";
import { pasteText, readTerminal, sendText, startDetached } from "@/features/terminal/xtermRegistry";
import { providerName, useWorkspaces, type Pane, type Workspace } from "@/stores/workspaces";
import { logActivity } from "@/stores/activity";
import { setTask } from "@/features/progress/progress";
import { agentCaller, pluginAgentToolList, runPluginAgentTool, type AgentCaller } from "@/features/plugins/plugins";
import { answersText, askQuestions, DISMISSED, dropQuestions, parseQuestions } from "@/features/questions/questions";
import { cancelWake, limitsOf, pendingWake, refreshLimits, scheduleWake, usageReport } from "@/features/usage/agentUsage";

interface Call {
  id: string;
  tool: string;
  args: Record<string, unknown>;
  paneId: string;
}

class ToolError extends Error {}

/** Tiled panes in layout order, then archived ones (still running, so still reachable). */
function order(ws: Workspace): Pane[] {
  const tiled = ws.layout ? leaves(ws.layout).map((id) => ws.panes.find((p) => p.id === id)!) : [];
  return [...tiled, ...ws.panes.filter((p) => p.archived)];
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
  // Asked by `tools/list` in src-tauri/src/mcp.rs.
  if (call.tool === "__plugin_tools") return JSON.stringify(pluginAgentToolList());
  const ws = useWorkspaces.getState().workspaces.find((w) => w.panes.some((p) => p.id === call.paneId));
  if (!ws) throw new ToolError("The calling pane isn't open in winfer anymore.");
  return runWorkspaceTool(ws, ws.panes.find((p) => p.id === call.paneId)!, call.tool, call.args ?? {});
}

/** Live voice: the agent tools in the workspace the user is looking at, with no calling pane. */
export function runVoiceTool(tool: string, args: Record<string, unknown>): Promise<string> {
  const s = useWorkspaces.getState();
  const ws = s.workspaces.find((w) => w.id === s.activeWorkspaceId);
  if (!ws) return Promise.reject(new ToolError("No workspace is open."));
  return runWorkspaceTool(ws, null, tool, args);
}

/** How long an `ask_user` question waits for the user (the app's own limit is an hour). */
const ASK_TIMEOUT_MS = 59 * 60_000;
const findPane = (id: string) => useWorkspaces.getState().workspaces.flatMap((w) => w.panes).find((p) => p.id === id);

const VOICE = "Live voice";
const who = (ws: Workspace, self: Pane | null) => (self ? describe(ws, self) : VOICE);

/** `self` is the calling pane, or null for live voice. */
async function runWorkspaceTool(ws: Workspace, self: Pane | null, tool: string, a: Record<string, unknown>): Promise<string> {
  const s = useWorkspaces.getState();
  switch (tool) {
    case "list_panes": {
      const rows = order(ws).map((p, i) => {
        const parts = [`${i + 1}. ${providerName(s.providers, p.provider)}`, p.status];
        if (p.worktree) parts.push(`branch ${p.worktree.branch}`);
        if (p.archived) parts.push("archived");
        if (p.id === self?.id) parts.push("(you)");
        if (!self && p.id === s.focusedPaneId) parts.push("focused");
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
      if (pane.id === self?.id) throw new ToolError("That's your own pane.");
      const text = String(a.text ?? "");
      if (!text) throw new ToolError("text is empty");
      if (text.includes("\n")) pasteText(pane.id, text);
      else sendText(pane.id, text);
      if (a.submit !== false) {
        sendText(pane.id, "\r");
        setTask(pane.id, text);
      }
      logActivity(ws.id, `${who(ws, self)} → ${describe(ws, pane)}: ${text.split("\n")[0].slice(0, 80)}`, self?.provider);
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
        splitFrom: self?.id ?? s.focusedPaneId ?? undefined,
      });
      const fresh = useWorkspaces.getState().workspaces.find((w) => w.id === ws.id)!;
      const pane = fresh.panes.find((p) => p.id === id)!;
      // Panes in a workspace the user isn't looking at start immediately, off-screen.
      if (ws.id !== useWorkspaces.getState().activeWorkspaceId) startDetached(pane);
      logActivity(ws.id, `${who(ws, self)} started ${provider.name}`, self?.provider);
      return `Started ${provider.name} as pane ${order(fresh).indexOf(pane) + 1}.`;
    }
    case "open_url": {
      const url = String(a.url ?? "");
      if (!url) throw new ToolError("url is empty");
      // Shown when the panel already shows this workspace or it's on screen; otherwise the tab
      // opens in the background and is listed in the panel without taking it over.
      const onScreen = s.view === "workspace" && ws.id === s.activeWorkspaceId;
      let opened: Opened | undefined;
      if (onScreen || ws.id === shownWorkspaceId()) opened = await openUrl(url, { newTab: Boolean(a.new_tab), workspaceId: ws.id });
      else {
        // In the background too, a tab already on that site is taken over rather than duplicated.
        const target = normalizeUrl(url);
        const related = a.new_tab ? undefined : relatedTab(useBrowser.getState().tabs[ws.id] ?? [], target);
        if (related) await invoke("browser_navigate", { workspaceId: ws.id, targetId: related.id, url: target });
        opened = related ? { id: related.id, tookOver: related } : { id: await invoke<string>("browser_new_tab", { workspaceId: ws.id, url: target }) };
      }
      if (!opened) return `Couldn't open ${url} in the browser panel.`;
      if (!opened.tookOver) markAgentOpened(opened.id);
      // Lit up right away; the agent's own browser tools keep it lit while it works there.
      markAgentTab(opened.id, ws.id, Boolean(opened.tookOver));
      if (opened.tookOver) {
        const was = opened.tookOver;
        logActivity(ws.id, `${who(ws, self)} took over ${tabLabel(was)} for ${url}`, self?.provider);
        return (
          `Took over the open tab "${tabLabel(was)}" (${was.url}), which already showed this site, and opened ${url} in it. ` +
          `To keep working there, select that tab with your browser tools (list the tabs, then select the one now on ${url}) instead of opening a new tab.`
        );
      }
      logActivity(ws.id, `${who(ws, self)} opened ${url}`, self?.provider);
      return `Opened ${url} in a new tab of the browser panel.`;
    }
    case "open_file": {
      const path = String(a.path ?? "").trim();
      if (!path) throw new ToolError("path is empty");
      const [found] = await resolveFiles(self?.cwd ?? ws.rootDir, [path]);
      if (!found) throw new ToolError(`No file at ${path} (relative paths start from ${self?.cwd ?? ws.rootDir}).`);
      // Takes over the side panel only when the user is looking at this workspace.
      const onScreen = s.view === "workspace" && ws.id === s.activeWorkspaceId;
      openFile(found, { line: typeof a.line === "number" ? a.line : undefined, background: !onScreen });
      logActivity(ws.id, `${who(ws, self)} opened ${found}`, self?.provider);
      return onScreen ? `Showing ${found} in the file viewer.` : `Added ${found} to the file viewer (the user is in another workspace).`;
    }
    case "notify_user": {
      if (!self) throw new ToolError("Live voice talks to the user directly; just say it.");
      reportStatus(self.id, "needs-input", String(a.message ?? "An agent needs you"));
      return "The user has been notified.";
    }
    case "ask_user": {
      if (!self) throw new ToolError("Live voice talks to the user directly; just ask.");
      const questions = parseQuestions(a.questions);
      if (!questions.length) throw new ToolError("questions is empty: give 1-4 questions, each with its options.");
      if (questions.length > 4) throw new ToolError("Ask at most 4 questions at once.");
      reportStatus(self.id, "needs-input", questions[0].question);
      logActivity(ws.id, `${who(ws, self)} asks: ${questions[0].question.slice(0, 80)}`, self.provider);
      // winfer gives up on a call after an hour (src-tauri/src/mcp.rs); so does the dialog.
      const timer = setTimeout(() => dropQuestions(self.id), ASK_TIMEOUT_MS);
      const answers = await askQuestions(self.id, questions).finally(() => clearTimeout(timer));
      if (findPane(self.id)?.status === "needs-input") reportStatus(self.id, "working");
      return answers ? answersText(questions, answers) : DISMISSED;
    }
    case "usage_status": {
      // Live voice asks about a pane's agent (its plan), defaulting to the focused pane.
      const subject = self ?? resolve(ws, a.pane ?? s.focusedPaneId ?? 1);
      const { provider, note } = await limitsOf(subject);
      await refreshLimits(provider);
      const lines = [note, usageReport(provider)];
      const wake = self && pendingWake(self.id);
      if (wake) lines.push(`You are paused: winfer will wake you at ${new Date(wake.at).toLocaleString()}.`);
      return lines.filter(Boolean).join("\n");
    }
    case "wait_for_usage_reset": {
      if (!self) throw new ToolError("Only agent panes can wait for a usage reset.");
      if (a.cancel === true) return cancelWake(self.id) ? "Cancelled: winfer won't wake you." : "You had no wake-up scheduled.";
      const { provider } = await limitsOf(self);
      if (a.minutes === undefined) await refreshLimits(provider);
      try {
        return scheduleWake(self.id, provider, {
          below: a.below === undefined ? undefined : Number(a.below),
          minutes: a.minutes === undefined ? undefined : Number(a.minutes),
          message: typeof a.message === "string" ? a.message : undefined,
        });
      } catch (err) {
        throw new ToolError(err instanceof Error ? err.message : String(err));
      }
    }
    default: {
      const caller = self ? agentCaller(self.id)! : voiceCaller(ws);
      const result = await runPluginAgentTool(tool, a, caller);
      if (result === null) throw new ToolError(`Unknown tool ${tool}`);
      return result;
    }
  }
}

/** Live voice as a plugin tool caller. */
const voiceCaller = (ws: Workspace): AgentCaller => ({
  paneId: "voice",
  workspaceId: ws.id,
  workspaceName: ws.name,
  cwd: ws.rootDir,
  provider: "voice",
  label: "live voice",
});

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
