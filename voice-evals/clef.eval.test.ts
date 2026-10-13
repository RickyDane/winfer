/**
 * Accuracy/latency eval against the real Clef API. Skipped unless CF_ACCOUNT_ID and
 * CF_API_TOKEN are set:  CF_ACCOUNT_ID=… CF_API_TOKEN=… pnpm eval:voice [clef-flash|clef]
 */
import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import type { ToolContext } from "@/tools/registry";
import type { Provider } from "@/features/providers/providers";
import type { Workspace } from "@/stores/workspaces";
import { buildRequest, parseDecision, type DecisionResult } from "@/features/voice/decision";

const { CF_ACCOUNT_ID, CF_API_TOKEN, CLEF_MODEL = "clef-flash" } = process.env;

interface Case {
  utterance: string;
  expect: string[];
  second?: string[];
  counts?: Record<string, number>;
}

const cases: Case[] = readFileSync(new URL("./cases.jsonl", import.meta.url), "utf8")
  .split("\n")
  .filter(Boolean)
  .map((l: string) => JSON.parse(l));

const provider = (id: string, name: string): Provider => ({ id, name, command: id, args: [], resumeArgs: [], aliases: [], statusHooks: false, promptArg: false, approveInput: null, installed: true });
const pane = (id: string, p: string, status: "working" | "idle" | "needs-input") => ({
  id, workspaceId: "ws", provider: p, title: "api", cwd: "~", status, worktree: null, wantsWorktree: false, resume: false, unseen: false, devUrl: null, raceId: null, prompt: null,
});
const ws: Workspace = {
  id: "ws", name: "bridgemind", rootDir: "~", zoomedPaneId: null, presetIndex: 0, useWorktrees: true,
  panes: [pane("p-1", "claude", "working"), pane("p-2", "codex", "needs-input"), pane("p-3", "shell", "idle")],
  layout: { kind: "split", dir: "row", sizes: [0.5, 0.5], children: [{ kind: "leaf", paneId: "p-1" }, { kind: "split", dir: "col", sizes: [0.5, 0.5], children: [{ kind: "leaf", paneId: "p-2" }, { kind: "leaf", paneId: "p-3" }] }] },
};
const ctx: ToolContext = {
  providers: [provider("claude", "Claude Code"), provider("codex", "Codex"), provider("agy", "Antigravity"), provider("grok", "Grok Build"), provider("shell", "Shell")],
  workspaces: [ws, { ...ws, id: "ws-2", name: "bridgemind api", panes: [], layout: null }],
  active: ws,
  focusedPaneId: "p-1",
  browserTabs: ["localhost:3000", "GitHub"],
  lastCommands: [],
};

async function ask(utterance: string): Promise<{ result: DecisionResult; ms: number }> {
  const started = performance.now();
  const res = await fetch(`https://api.cloudflare.com/client/v4/accounts/${CF_ACCOUNT_ID}/ai/run/@cf/cloudflare/${CLEF_MODEL}`, {
    method: "POST",
    headers: { Authorization: `Bearer ${CF_API_TOKEN}`, "Content-Type": "application/json" },
    body: JSON.stringify({ model: CLEF_MODEL, ...buildRequest(utterance, ctx) }),
  });
  const json = await res.json();
  if (!json.success) throw new Error(JSON.stringify(json.errors));
  return { result: json.result, ms: performance.now() - started };
}

describe.skipIf(!CF_ACCOUNT_ID || !CF_API_TOKEN)(`voice eval (${CLEF_MODEL})`, () => {
  it(
    "routes utterances to the expected tools",
    async () => {
      const rows: { utterance: string; got: string; ok: boolean; confidence: number; ms: number }[] = [];
      for (const c of cases) {
        const { result, ms } = await ask(c.utterance);
        const decisions = parseDecision(result, ctx);
        const got = decisions[0]?.tool.id ?? "none";
        let ok = c.expect.includes(got);
        if (ok && c.counts && got === "spawn_panes") {
          const counts = decisions[0].args.counts as Record<string, number>;
          ok = Object.entries(c.counts).every(([p, n]) => counts[p] === n);
        }
        rows.push({ utterance: c.utterance, got, ok, confidence: decisions[0]?.confidence ?? 0, ms: Math.round(ms) });
      }
      const accuracy = rows.filter((r) => r.ok).length / rows.length;
      const sorted = rows.map((r) => r.ms).sort((a, b) => a - b);
      console.table(rows.filter((r) => !r.ok));
      console.log(
        `accuracy ${(accuracy * 100).toFixed(1)}% (${rows.filter((r) => r.ok).length}/${rows.length}) · p50 ${sorted[Math.floor(sorted.length / 2)]} ms · p95 ${sorted[Math.floor(sorted.length * 0.95)]} ms`,
      );
      expect(accuracy).toBeGreaterThanOrEqual(0.9);
    },
    10 * 60_000,
  );
});
