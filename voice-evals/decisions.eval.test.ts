/**
 * Accuracy/latency eval against the real OpenAI Decisions API (GPT-6 Luna). Skipped unless
 * OPENAI_TOKEN is set (an API key or a Sign in with ChatGPT access token):
 *   OPENAI_TOKEN=… pnpm eval:voice
 */
import { readFileSync, writeFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import type { ToolContext } from "@/tools/registry";
import type { Provider } from "@/features/providers/providers";
import type { Workspace } from "@/stores/workspaces";
import { buildRequest, fromApi, parseDecision, toApi, type DecisionResult } from "@/features/voice/decision";

const { OPENAI_TOKEN } = process.env;
const MODEL = "gpt-6-luna";

interface Case {
  utterance: string;
  expect: string[];
  second?: string[];
  counts?: Record<string, number>;
  /** Substring the extracted spawn prompt must contain. */
  prompt?: string;
  /** Substring the text typed into a pane must contain. */
  text?: string;
  /** Model the decision must carry. */
  model?: string;
  /** URL the browser must open. */
  url?: string;
}

// Real aliases, since they're part of what the model sees.
const aliases: Record<string, string[]> = Object.fromEntries(
  JSON.parse(readFileSync(new URL("../src-tauri/resources/providers.json", import.meta.url), "utf8")).map((p: { id: string; aliases?: string[] }) => [p.id, p.aliases ?? []]),
);

const cases: Case[] = readFileSync(new URL("./cases.jsonl", import.meta.url), "utf8")
  .split("\n")
  .filter((l: string) => l.trim()) // blank lines; "\r" alone when git checks it out with CRLF
  .map((l: string) => JSON.parse(l) as Case)
  // EVAL_ONLY=regex runs just the matching utterances.
  .filter((c) => !process.env.EVAL_ONLY || new RegExp(process.env.EVAL_ONLY, "i").test(c.utterance));

const provider = (id: string, name: string): Provider => ({ id, name, command: id, args: [], resumeArgs: [], aliases: aliases[id] ?? [], statusHooks: false, promptArg: false, approveInput: null, modelArg: id === "shell" ? null : "--model", modelCommand: null, installed: true });
const pane = (id: string, p: string, status: "working" | "idle" | "needs-input") => ({
  id, workspaceId: "ws", provider: p, title: "api", cwd: "~", status, worktree: null, wantsWorktree: false, resume: false, unseen: false, devUrl: null, raceId: null, prompt: null, model: null, agentTitle: null,
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
  recentAddresses: {
    "file:///Users/ricky/Coding/index.html": "HTML file index.html that pane 1 (Claude Code) created (/Users/ricky/Coding/index.html), most recent",
    "http://localhost:5173/": "local dev server at localhost:5173/ started in pane 3 (Shell)",
  },
  models: { claude: [{ id: "fable", label: "Fable (latest)" }, { id: "opus", label: "Opus (latest)" }, { id: "sonnet", label: "Sonnet (latest)" }, { id: "haiku", label: "Haiku (latest)" }] },
};

async function ask(utterance: string): Promise<{ result: DecisionResult; ms: number; tokens: number }> {
  const request = buildRequest(utterance, ctx);
  const started = performance.now();
  const res = await fetch("https://api.openai.com/v1/decisions", {
    method: "POST",
    headers: { Authorization: `Bearer ${OPENAI_TOKEN}`, "Content-Type": "application/json" },
    body: JSON.stringify({ model: MODEL, ...toApi(request) }),
  });
  const json = await res.json();
  if (!res.ok) throw new Error(JSON.stringify(json.error ?? json));
  return { result: fromApi(json, request), ms: performance.now() - started, tokens: json.usage?.input_tokens ?? 0 };
}

describe.skipIf(!OPENAI_TOKEN)(`voice eval (${MODEL})`, () => {
  it(
    "routes utterances to the expected tools",
    async () => {
      const rows: { utterance: string; got: string; ok: boolean; confidence: number; ms: number; top: number; margin: number; ratio: number; tokens: number; text: string }[] = [];
      for (const c of cases) {
        const { result, ms, tokens } = await ask(c.utterance);
        const decisions = parseDecision(result, ctx);
        const got = decisions.map((d) => d.tool.id).join("+") || "none";
        // Two-action commands may come back in either order.
        let ok = decisions.length ? decisions.some((d) => c.expect.includes(d.tool.id)) : c.expect.includes("none");
        const spawn = decisions.find((d) => d.tool.id === "spawn_panes" || d.tool.id === "start_features");
        if (ok && c.counts && spawn) {
          // What actually opens, after the tool's own fix-ups.
          const counts: Record<string, number> = {};
          for (const cmd of spawn.tool.toCommands(spawn.args, ctx)) if (cmd.type === "pane.spawn") counts[cmd.provider] = (counts[cmd.provider] ?? 0) + (cmd.count ?? 1);
          ok = Object.entries(c.counts).every(([p, n]) => (counts[p] ?? 0) === n);
        }
        // The prompt must contain the instruction and not start with a lead-in like "and tell them to".
        const prompt = String(spawn?.args.prompt ?? "").toLowerCase();
        if (ok && c.prompt) ok = prompt.includes(c.prompt) && !/^(and |tell |ask )/.test(prompt);
        // Plain "open X" commands must not invent a prompt.
        if (ok && spawn && !c.prompt && prompt) ok = false;
        if (ok && c.model) ok = decisions.some((d) => d.args.model === c.model);
        if (ok && c.url) {
          const opened = decisions.flatMap((d) => d.tool.toCommands(d.args, ctx)).find((cmd) => cmd.type === "browser.open");
          ok = opened?.type === "browser.open" && opened.url.includes(c.url);
        }
        const typed = decisions.find((d) => d.tool.id === "send_to_pane");
        if (ok && c.text) ok = String(typed?.args.text ?? "").toLowerCase().includes(c.text);
        const probs = Object.values((result.answers.action_1 as { probabilities?: Record<string, number> })?.probabilities ?? {}).sort((a, b) => b - a);
        rows.push({
          utterance: c.utterance,
          got,
          ok,
          confidence: decisions[0]?.confidence ?? 0,
          ms: Math.round(ms),
          top: probs[0] ?? 0,
          margin: (probs[0] ?? 0) - (probs[1] ?? 0),
          ratio: (probs[0] ?? 0) / Math.max(probs[1] ?? 0, 1e-6),
          tokens,
          text: String(
            decisions.find((d) => d.args.prompt)?.args.prompt ??
              decisions.find((d) => d.args.text)?.args.text ??
              (decisions.flatMap((d) => d.tool.toCommands(d.args, ctx)).find((c) => c.type === "browser.open") as { url?: string } | undefined)?.url ??
              "",
          ),
        });
      }
      const accuracy = rows.filter((r) => r.ok).length / rows.length;
      if (process.env.EVAL_REPORT) writeFileSync(process.env.EVAL_REPORT, JSON.stringify(rows, null, 1));
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
