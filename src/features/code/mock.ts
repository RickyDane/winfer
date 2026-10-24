/**
 * A scripted stand-in for the winfer code backend: emits the same `Ev` stream a real session
 * does (reasoning, tool calls with streamed output, an edit approval with a diff, a subagent,
 * todos, usage), so the web UI can be designed and reviewed without a running agent.
 */
import { MOCK_MODELS, type Cmd, type Ev, type Transport } from "./protocol";

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

const DIFF = `--- a/src/lib/retry.ts
+++ b/src/lib/retry.ts
@@ -1,12 +1,24 @@
-export async function retry<T>(fn: () => Promise<T>, times = 3): Promise<T> {
-  let last: unknown;
-  for (let i = 0; i < times; i++) {
-    try {
-      return await fn();
-    } catch (e) {
-      last = e;
-    }
-  }
-  throw last;
+export interface RetryOptions {
+  times?: number;
+  /** First delay in ms; doubles each attempt. */
+  baseDelay?: number;
+  signal?: AbortSignal;
+}
+
+export async function retry<T>(fn: () => Promise<T>, opts: RetryOptions = {}): Promise<T> {
+  const { times = 3, baseDelay = 200, signal } = opts;
+  let last: unknown;
+  for (let i = 0; i < times; i++) {
+    signal?.throwIfAborted();
+    try {
+      return await fn();
+    } catch (e) {
+      last = e;
+      if (i < times - 1) await new Promise((r) => setTimeout(r, baseDelay * 2 ** i));
+    }
+  }
+  throw last;
 }`;

const FINAL = `Added exponential backoff to \`retry()\` and made it cancellable.

**What changed**
- \`src/lib/retry.ts:8\` — new \`RetryOptions\` (\`times\`, \`baseDelay\`, \`signal\`); delay doubles per attempt (200 → 400 → 800 ms).
- \`src/api/client.ts:41\` — call sites pass the request's \`AbortSignal\`, so closing a pane stops pending retries.

**Verified**
| check | result |
|---|---|
| \`pnpm test retry\` | 6 passed |
| \`tsc --noEmit\` | clean |

\`\`\`ts
await retry(() => fetchModels(), { times: 4, signal: ctrl.signal });
\`\`\`

The old positional \`times\` argument is gone — the two callers in \`src/api\` were updated.`;

const REASONING =
  "The user wants backoff on retries. Let me look at the current retry helper and who calls it. " +
  "It retries immediately, which hammers the API on 429s. I'll add an options object with a base delay " +
  "that doubles, plus an AbortSignal so cancelled requests stop retrying. Need to update callers too.";

export class MockTransport implements Transport {
  private emit: (ev: Ev) => void = () => {};
  private waiters = new Map<string, (ok: boolean) => void>();
  private cancelled = false;
  private turn = 0;
  private ctx = 18_400;
  private out = 2_100;

  connect(onEvent: (ev: Ev) => void) {
    this.emit = onEvent;
    onEvent({
      type: "ready",
      session: "s_7f3a9c",
      backend_session: "01a16a3a-9e2e-79f1",
      title: "Retry backoff for the API client",
      model: "claude/opus",
      mode: "ask",
      effort: "high",
      cwd: "/Users/you/Coding/winfer",
      branch: "main",
      trim_at: 75,
      compact_at: 90,
      mcp: ["winfer", "context7", "codegraph"],
    });
    onEvent({ type: "models", models: MOCK_MODELS });
    onEvent({ type: "model", model: "claude-opus-5-5" });
    onEvent({ type: "skills", skills: ["review", "simplify", "run", "security-review", "dataviz"] });
    this.usage();
    return () => {
      this.emit = () => {};
    };
  }

  send(cmd: Cmd) {
    switch (cmd.type) {
      case "send":
        void this.run(cmd.text);
        break;
      case "steer":
        this.emit({ type: "notice", text: `Steering: "${cmd.text}" — delivered at the next step` });
        break;
      case "interrupt":
        this.cancelled = true;
        for (const w of this.waiters.values()) w(false);
        this.waiters.clear();
        break;
      case "approve":
        this.waiters.get(cmd.id)?.(cmd.decision !== "deny");
        this.waiters.delete(cmd.id);
        break;
      case "answer":
        this.waiters.get(cmd.id)?.(true);
        this.waiters.delete(cmd.id);
        break;
      case "set_mode":
        break;
      case "set_model":
        this.emit({ type: "model", model: cmd.model });
        if (cmd.model.startsWith("claude/")) this.emit({ type: "model", model: `claude-${cmd.model.slice(7)}-5-5` });
        break;
      case "compact":
        this.ctx = 6_200;
        this.usage();
        this.emit({ type: "notice", text: "Compacted conversation · 41k → 6k tokens" });
        break;
      case "task_log": {
        const id = cmd.id;
        const lines = this.devLog.slice(0, Math.min(this.devLog.length, 4 + Math.floor((Date.now() - this.devStart) / 900)));
        this.emit({ type: "task_log", id, text: id === "bg-dev" ? lines.join("\n") : null, file: id === "bg-dev" ? "/tmp/winfer-bg/bg-dev.log" : null });
        break;
      }
      case "sessions":
        setTimeout(
          () =>
            this.emit({
              type: "sessions",
              sessions: [
                { id: "s_7f3a9c", title: "Retry backoff for the API client", model: "claude/opus", updated: Date.now() / 1000 - 30, messages: 2 },
                { id: "s_61be02", title: "Pane drag & drop reorder", model: "chatgpt/gpt-6.1-sol", updated: Date.now() / 1000 - 7200, messages: 9 },
                { id: "s_44d1aa", title: "Fix updater on the Nebula channel", model: "codex/gpt-6.1-sol", updated: Date.now() / 1000 - 90_000, messages: 4 },
              ],
            }),
          250,
        );
        break;
      case "resume":
        this.emit({ type: "cleared" });
        this.emit({ type: "restore", transcript: [{ role: "user", text: "Make panes reorderable by drag and drop" }, { role: "tool", text: "Read\tsrc/features/layout/Tiles.tsx" }, { role: "assistant", text: "Done — panes now reorder by dragging their header." }] });
        break;
      case "btw":
        setTimeout(() => this.emit({ type: "btw", id: cmd.id, answer: "It's editing `src/lib/retry.ts`: adding a `RetryOptions` object with exponential backoff and an `AbortSignal`, then running the retry tests." }), 1400);
        break;
      case "diff":
        this.emit({ type: "tool_start", id: "diff-1", name: "Diff", title: "1 file", input: {} });
        this.emit({ type: "tool_end", id: "diff-1", ok: true, output: "", diff: DIFF });
        break;
      case "export":
        this.emit({ type: "notice", text: "Saved /Users/you/Coding/winfer/winfer-session-s_7f3a9c.md" });
        break;
      case "set_compaction":
        break;
      case "new":
        this.emit({ type: "cleared" });
        break;
      case "undo":
        this.emit({ type: "undone", files: ["src/lib/retry.ts"] });
        break;
      case "stop_task":
        this.stopped.add(cmd.id);
        this.emit({ type: "task", id: cmd.id, status: "stopped" });
        this.emit({ type: "bg_note", text: `Stopped ${cmd.id === "bg-dev" ? "pnpm dev" : "the background agent"}` });
        break;
      default:
        break;
    }
  }

  private usage() {
    this.emit({
      type: "usage",
      context: this.ctx,
      window: 200_000,
      input: this.ctx * 3,
      cached: this.ctx * 2,
      output: this.out,
      turn: this.out - 2_100,
      turn_input: this.ctx * 2,
      turn_cached: Math.round(this.ctx * 1.4),
      turn_streamed: this.out - 2_100,
      cost: +(this.ctx * 0.000004 + this.out * 0.00002).toFixed(3),
    });
  }

  private stopped = new Set<string>();
  private devStart = Date.now();
  private devLog = [
    "> winfer@0.1.105 dev",
    "> vite",
    "",
    "  VITE v8.3.4  ready in 312 ms",
    "",
    "  ➜  Local:   http://localhost:5173/",
    "  ➜  Network: use --host to expose",
    "",
    "10:41:02 PM [vite] hmr update /src/lib/retry.ts",
    "10:41:02 PM [vite] hmr update /src/api/client.ts",
    "10:41:09 PM [vite] page reload src/main.tsx",
    "10:41:15 PM [vite] hmr update /src/features/updates/poll.ts",
    "10:41:21 PM [vite] ✓ 1342 modules transformed.",
    "10:41:28 PM [vite] hmr update /src/api/usage.ts",
  ];

  /** The background agent works on after the turn, then reports, which starts a short turn. */
  private async backgroundAgent() {
    const steps = ["Grep while.*retry", "Read src/api/usage.ts", "Grep setTimeout\\(.*attempt", "Read src/features/updates/poll.ts", "Read src/lib/ipc.ts", "Grep catch.*retry"];
    for (let n = 0; n < steps.length; n++) {
      await sleep(2600);
      if (this.stopped.has("bg-agent")) return;
      this.emit({ type: "task", id: "bg-agent", tools: n + 1, last: steps[n] });
    }
    await sleep(1500);
    if (this.stopped.has("bg-agent")) return;
    this.emit({ type: "task", id: "bg-agent", status: "done", last: "found 2 hand-rolled loops" });
    this.emit({ type: "bg_note", text: "Background agent “Audit other retry-like loops” finished" });
    if (this.waiters.size) return;
    this.emit({ type: "turn_start" });
    await sleep(400);
    const text =
      "The background audit is back: two more places retry by hand and would benefit from `retry()`:\n\n" +
      "- `src/api/usage.ts:18` — polls with a fixed 1s sleep, no cap\n" +
      "- `src/features/updates/poll.ts:42` — retries forever on network errors\n\n" +
      "Want me to move both onto `retry()` with a signal?";
    const words = text.split(/(\s+)/);
    for (let i = 0; i < words.length; i += 3) {
      await sleep(14 + Math.random() * 14);
      this.emit({ type: "text_delta", id: "bg-report", text: words.slice(i, i + 3).join("") });
    }
    this.emit({ type: "turn_done" });
  }

  private async step(ms: number) {
    await sleep(ms);
    if (this.cancelled) throw new Error("cancelled");
  }

  private async stream(id: string, text: string, kind: "text_delta" | "reasoning_delta", speed = 14) {
    const words = text.split(/(\s+)/);
    for (let i = 0; i < words.length; i += 3) {
      await this.step(speed + Math.random() * speed);
      this.emit({ type: kind, id, text: words.slice(i, i + 3).join("") });
      this.out += 4;
      if (i % 12 === 0) this.usage();
    }
  }

  private approval(id: string, tool: string, title: string, detail: string, diff?: string) {
    return new Promise<boolean>((resolve) => {
      this.waiters.set(id, resolve);
      this.emit({ type: "approval", id, tool, title, detail, diff, questions: [] });
    });
  }

  private async run(prompt: string) {
    this.cancelled = false;
    const t = ++this.turn;
    const id = (s: string) => `t${t}-${s}`;
    const e = (ev: Ev) => this.emit(ev);
    e({ type: "turn_start" });
    try {
      await this.step(500);
      await this.stream(id("r"), t === 1 ? REASONING : `Thinking about "${prompt.slice(0, 60)}"… checking the relevant files first.`, "reasoning_delta", 18);

      e({
        type: "todos",
        todos: [
          { content: "Read retry helper and callers", status: "in_progress" },
          { content: "Add backoff + AbortSignal", status: "pending" },
          { content: "Update call sites", status: "pending" },
          { content: "Run tests", status: "pending" },
        ],
      });

      e({ type: "tool_start", id: id("read"), name: "Read", title: "src/lib/retry.ts", input: { file_path: "src/lib/retry.ts" } });
      await this.step(450);
      e({ type: "tool_end", id: id("read"), ok: true, output: "12 lines" });

      e({ type: "tool_start", id: id("grep"), name: "Grep", title: "retry\\(", input: { pattern: "retry\\(" } });
      await this.step(380);
      e({
        type: "tool_end",
        id: id("grep"),
        ok: true,
        output: "src/api/client.ts:41:  return retry(() => fetchModels(), 3);\nsrc/api/usage.ts:18:  const u = await retry(poll, 5);",
      });

      e({ type: "task", id: id("agent"), tool_id: id("ag"), title: "Find retry-related tests", agent: "Explore", tools: 0, status: "running" });
      e({ type: "tool_start", id: id("ag"), name: "Agent", title: "Find retry-related tests", input: { subagent_type: "Explore" } });
      for (let n = 1; n <= 4; n++) {
        await this.step(350);
        e({ type: "task", id: id("agent"), tools: n, last: ["Glob **/*.test.ts", "Grep retry", "Read retry.test.ts", "Read client.test.ts"][n - 1] });
      }
      e({ type: "task", id: id("agent"), status: "done" });
      e({ type: "tool_end", id: id("ag"), ok: true, output: "src/lib/retry.test.ts:1-58 covers success, failure, exhaustion" });
      this.ctx += 9_000;
      this.usage();

      // A background agent: its tool call returns at once, the agent keeps working past the turn.
      if (t === 1) {
        e({ type: "tool_start", id: id("bga"), name: "Agent", title: "Audit other retry-like loops (background)", input: { subagent_type: "general-purpose", run_in_background: true } });
        e({ type: "task", id: "bg-agent", tool_id: id("bga"), title: "Audit other retry-like loops", description: "Looks for hand-rolled retry loops that should use retry()", agent: "general-purpose", tools: 0, status: "running" });
        await this.step(250);
        e({ type: "tool_end", id: id("bga"), ok: true, output: "Started in the background — it reports back when it's done." });
        void this.backgroundAgent();
      }

      e({
        type: "todos",
        todos: [
          { content: "Read retry helper and callers", status: "completed" },
          { content: "Add backoff + AbortSignal", status: "in_progress" },
          { content: "Update call sites", status: "pending" },
          { content: "Run tests", status: "pending" },
        ],
      });

      e({ type: "tool_start", id: id("edit"), name: "Edit", title: "src/lib/retry.ts", input: { file_path: "src/lib/retry.ts" } });
      const ok = await this.approval(id("ap"), "Edit", "Edit src/lib/retry.ts", "Add RetryOptions with exponential backoff and AbortSignal", DIFF);
      if (this.cancelled) throw new Error("cancelled");
      if (!ok) {
        e({ type: "tool_end", id: id("edit"), ok: false, output: "Declined by user" });
        await this.stream(id("x"), "Okay, I left `retry.ts` untouched. Tell me what you'd like instead.", "text_delta");
        return;
      }
      await this.step(300);
      e({ type: "tool_end", id: id("edit"), ok: true, output: "+15 −3", diff: DIFF });

      e({
        type: "todos",
        todos: [
          { content: "Read retry helper and callers", status: "completed" },
          { content: "Add backoff + AbortSignal", status: "completed" },
          { content: "Update call sites", status: "completed" },
          { content: "Run tests", status: "in_progress" },
        ],
      });

      e({ type: "tool_start", id: id("sh"), name: "Bash", title: "pnpm vitest run retry", input: { command: "pnpm vitest run retry" } });
      const lines = [
        "\n RUN  v5.0.4 /Users/you/winfer\n\n",
        " ✓ src/lib/retry.test.ts > returns first success  2ms\n",
        " ✓ src/lib/retry.test.ts > retries until success  3ms\n",
        " ✓ src/lib/retry.test.ts > backs off exponentially  604ms\n",
        " ✓ src/lib/retry.test.ts > throws last error  1ms\n",
        " ✓ src/lib/retry.test.ts > stops on abort  1ms\n",
        " ✓ src/api/client.test.ts > passes signal  4ms\n",
        "\n Test Files  2 passed (2)\n      Tests  6 passed (6)\n   Duration  1.12s\n",
      ];
      for (const l of lines) {
        await this.step(220);
        e({ type: "tool_output", id: id("sh"), chunk: l });
      }
      e({ type: "tool_end", id: id("sh"), ok: true, output: "" });
      e({
        type: "todos",
        todos: [
          { content: "Read retry helper and callers", status: "completed" },
          { content: "Add backoff + AbortSignal", status: "completed" },
          { content: "Update call sites", status: "completed" },
          { content: "Run tests", status: "completed" },
        ],
      });

      if (t === 1) {
        // A background shell: the dev server keeps running after the turn.
        e({ type: "tool_start", id: id("dev"), name: "Bash", title: "Vite dev server", input: { command: "pnpm dev", description: "Vite dev server", run_in_background: true } });
        this.devStart = Date.now();
        e({ type: "task", id: "bg-dev", tool_id: id("dev"), output_file: "/tmp/winfer-bg/bg-dev.log", title: "Vite dev server", description: "Vite dev server to try the change", tools: null, status: "running", last: "starting…" });
        await this.step(600);
        e({ type: "tool_end", id: id("dev"), ok: true, output: "Running in the background (task bg-dev)" });
        e({ type: "task", id: "bg-dev", last: "VITE v8.3.4 ready · http://localhost:5173/" });
      }

      await this.stream(id("txt"), FINAL, "text_delta", 10);
      this.ctx += 6_500;
      this.usage();
    } catch {
      e({ type: "notice", text: "Interrupted" });
    } finally {
      e({ type: "turn_done" });
    }
  }
}
