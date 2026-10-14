/**
 * Live check of the LLM planner on long/complex commands, through the logged-in CLI:
 *   PLANNER=claude [PLANNER_MODEL=sonnet] pnpm eval:planner
 *   PLANNER=openai OPENAI_TOKEN=… [PLANNER_MODEL=gpt-6-luna] pnpm eval:planner   (Responses API; an
 *     API key or a Sign in with ChatGPT access token)
 * Skipped unless PLANNER is set.
 */
import { execFileSync } from "node:child_process";
import { appendFileSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { describe, expect, it, vi } from "vitest";
import type { ToolContext } from "@/tools/registry";
import type { Provider } from "@/features/providers/providers";
import type { Workspace } from "@/stores/workspaces";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@/features/terminal/xtermRegistry", () => ({ readTerminal: vi.fn(() => "") }));
const { PLANNER_SYSTEM, parsePlan, plannerPrompt } = await import("@/features/voice/planner");

const { PLANNER, PLANNER_MODEL = "" } = process.env;

interface Case {
  utterance: string;
  /** Tool ids the plan must contain, with how many calls each (spawn_panes counts new agents); a list accepts any of them. */
  expect: Record<string, number> | Record<string, number>[];
  /** Tools that may also appear (e.g. stopping an agent before giving it a new task). */
  optional?: string[];
  /** Every new agent must start in its own worktree. */
  worktrees?: boolean;
}

const cases: Case[] = readFileSync(new URL("./planner-cases.jsonl", import.meta.url), "utf8")
  .split("\n")
  .filter(Boolean)
  .map((l) => JSON.parse(l) as Case)
  .filter((c) => !process.env.EVAL_ONLY || new RegExp(process.env.EVAL_ONLY, "i").test(c.utterance));

const aliases: Record<string, string[]> = Object.fromEntries(
  JSON.parse(readFileSync(new URL("../src-tauri/resources/providers.json", import.meta.url), "utf8")).map((p: { id: string; aliases?: string[] }) => [p.id, p.aliases ?? []]),
);
const provider = (id: string, name: string): Provider => ({ id, name, command: id, args: [], resumeArgs: [], aliases: aliases[id] ?? [], statusHooks: false, promptArg: false, approveInput: null, modelArg: id === "shell" ? null : "--model", modelCommand: null, installed: true });
const pane = (id: string, p: string, status: "working" | "idle" | "needs-input") => ({
  id, workspaceId: "ws", provider: p, title: "api", cwd: "~", status, worktree: null, wantsWorktree: false, resume: false, unseen: false, devUrl: null, raceId: null, prompt: null, model: null, agentTitle: null,
});
const ws: Workspace = {
  id: "ws", name: "winfer", rootDir: "~/Coding/winfer", zoomedPaneId: null, presetIndex: 0, useWorktrees: true,
  panes: [pane("p-1", "claude", "working"), pane("p-2", "codex", "needs-input"), pane("p-3", "shell", "idle")],
  layout: { kind: "split", dir: "row", sizes: [0.5, 0.5], children: [{ kind: "leaf", paneId: "p-1" }, { kind: "split", dir: "col", sizes: [0.5, 0.5], children: [{ kind: "leaf", paneId: "p-2" }, { kind: "leaf", paneId: "p-3" }] }] },
};
const ctx: ToolContext = {
  providers: [provider("claude", "Claude Code"), provider("codex", "Codex"), provider("opencode", "OpenCode"), provider("shell", "Shell")],
  workspaces: [ws],
  active: ws,
  focusedPaneId: "p-1",
  browserTabs: ["localhost:5173"],
  lastCommands: [],
  recentAddresses: {},
  models: { claude: [{ id: "opus", label: "Opus (latest)" }, { id: "sonnet", label: "Sonnet (latest)" }] },
};

/** The Responses API, as src-tauri/src/chatgpt.rs calls it (streamed, not stored). */
async function respond(user: string): Promise<string> {
  const res = await fetch("https://api.openai.com/v1/responses", {
    method: "POST",
    headers: { Authorization: `Bearer ${process.env.OPENAI_TOKEN}`, "Content-Type": "application/json" },
    body: JSON.stringify({
      model: PLANNER_MODEL || "gpt-6-luna",
      instructions: PLANNER_SYSTEM,
      input: [{ role: "user", content: [{ type: "input_text", text: user }] }],
      store: false,
      stream: true,
    }),
  });
  if (!res.ok) throw new Error(await res.text());
  let text = "";
  for (const line of (await res.text()).split("\n")) {
    if (!line.startsWith("data:")) continue;
    const event = JSON.parse(line.slice(5));
    if (event.type === "response.output_text.delta") text += event.delta;
  }
  return text;
}

/** Same flags as src-tauri/src/llm.rs. */
async function plan(utterance: string) {
  const started = performance.now();
  const dir = mkdtempSync(join(tmpdir(), "winfer-planner-"));
  const user = plannerPrompt(utterance, ctx);
  let text: string;
  try {
    if (PLANNER === "openai") {
      text = await respond(user);
    } else {
      writeFileSync(join(dir, "system.txt"), PLANNER_SYSTEM);
      text = execFileSync(
        "claude",
        ["-p", "--output-format", "text", "--no-session-persistence", "--tools", "", "--strict-mcp-config", "--setting-sources", "", "--system-prompt-file", join(dir, "system.txt"), ...(PLANNER_MODEL ? ["--model", PLANNER_MODEL] : [])],
        { cwd: dir, input: user, encoding: "utf8" },
      );
    }
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
  return { text, ms: performance.now() - started };
}

/** Passing tests' console output is hidden; EVAL_LOG=file keeps it. */
const log = (line: string) => (process.env.EVAL_LOG ? appendFileSync(process.env.EVAL_LOG, line + "\n") : console.log(line));

describe.skipIf(!PLANNER)(`planner eval (${PLANNER} ${PLANNER_MODEL})`, () => {
  it(
    "plans complex commands",
    async () => {
      let passed = 0;
      for (const c of cases) {
        const { text, ms } = await plan(c.utterance);
        const decisions = parsePlan(text, ctx) ?? [];
        const got: Record<string, number> = {};
        for (const d of decisions) {
          const n = d.tool.id === "spawn_panes" ? d.tool.toCommands(d.args, ctx).reduce((sum, cmd) => sum + (cmd.type === "pane.spawn" ? (cmd.count ?? 1) : 0), 0) : 1;
          got[d.tool.id] = (got[d.tool.id] ?? 0) + n;
        }
        const key = (m: Record<string, number>) => JSON.stringify(Object.entries(m).filter(([id]) => !c.optional?.includes(id)).sort());
        let ok = (Array.isArray(c.expect) ? c.expect : [c.expect]).some((e) => key(e) === key(got));
        if (ok && c.worktrees) ok = decisions.filter((d) => d.tool.id === "spawn_panes").every((d) => d.args.worktree === true);
        if (ok) passed++;
        log(`${ok ? "✓" : "✗"} ${Math.round(ms)} ms  ${c.utterance.slice(0, 80)}\n   ${JSON.stringify(got)}`);
        for (const d of decisions) log(`   · ${d.tool.id} ${JSON.stringify(d.args)}`);
      }
      log(`${PLANNER} ${PLANNER_MODEL || "default"}: passed ${passed}/${cases.length}`);
      expect(passed / cases.length).toBeGreaterThanOrEqual(0.8);
    },
    20 * 60_000,
  );
});
