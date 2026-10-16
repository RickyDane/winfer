/**
 * Dev-only fake backend so the UI can run in a plain browser (`pnpm dev`) for quick
 * iteration and automated UI checks. Never loaded inside the Tauri app or in builds.
 */
import type { Channel } from "@tauri-apps/api/core";
import { emit } from "@tauri-apps/api/event";
import { mockIPC, mockWindows } from "@tauri-apps/api/mocks";
import pkg from "../../package.json";
import { drawApp, SHOWCASE, seedShowcase, showcaseState, stageScene, transcript } from "./showcase";
import { KB_DEMO, knowledgeBaseExec, knowledgeBaseFile } from "./knowledgeBase";
import { XP_DEMO, crossProjectExec, seedCrossProject } from "./crossProject";
import { DB_CONNECTION, DB_DEMO, dbExplorerExec } from "./dbExplorer";

const encoder = new TextEncoder();
const send = (channel: Channel<ArrayBuffer>, text: string) => channel.onmessage(encoder.encode(text).buffer as ArrayBuffer);

const providers = [
  { id: "claude", name: "Claude Code", command: "claude", args: [], resumeArgs: ["--continue"], resumeSessionArgs: ["--resume", "{session}"], aliases: [], statusHooks: true, promptArg: true, approveInput: null, modelArg: null, modelCommand: null, autoApproveArgs: ["--dangerously-skip-permissions"], installed: true },
  { id: "codex", name: "Codex", command: "codex", args: [], resumeArgs: ["resume", "--last"], resumeSessionArgs: ["resume", "{session}"], aliases: [], statusHooks: true, promptArg: true, approveInput: null, modelArg: null, modelCommand: null, autoApproveArgs: ["--dangerously-bypass-approvals-and-sandbox"], installed: true },
  { id: "agy", name: "Antigravity", command: "agy", args: [], resumeArgs: [], aliases: [], statusHooks: false, promptArg: false, approveInput: null, modelArg: null, modelCommand: null, installed: true },
  { id: "grok", name: "Grok Build", command: "grok", args: [], resumeArgs: [], aliases: [], statusHooks: false, promptArg: false, approveInput: null, modelArg: null, modelCommand: null, installed: false },
  { id: "opencode", name: "OpenCode", command: "opencode", args: [], resumeArgs: ["--continue"], resumeSessionArgs: ["--session", "{session}"], aliases: [], statusHooks: !!SHOWCASE, promptArg: false, approveInput: null, modelArg: null, modelCommand: null, autoApproveArgs: ["--auto"], installed: true },
  { id: "shell", name: "Shell", command: null, args: [], resumeArgs: [], aliases: [], statusHooks: false, promptArg: false, approveInput: null, modelArg: null, modelCommand: null, installed: true },
];

// Fake plugins: the store is the repo's plugins/ folder; one local plugin is broken.
const storeManifests = import.meta.glob<Record<string, unknown>>("../../plugins/*/plugin.json", { eager: true, import: "default" });
const storeSources = import.meta.glob<string>("../../plugins/*/*.js", { eager: true, query: "?raw", import: "default" });
type MockPlugin = { id: string; dir: string; enabled: boolean; fromStore: boolean; error: string | null; manifest: Record<string, unknown> | null };
const mockPlugins: MockPlugin[] = [
  { id: "broken", dir: "~/Library/Application Support/dev.winfer.app/plugins/broken", enabled: false, fromStore: false, error: "invalid plugin.json: expected value at line 1 column 3", manifest: null },
];
function installMockPlugin(id: string) {
  const manifest = storeManifests[`../../plugins/${id}/plugin.json`];
  if (!manifest) throw new Error(`“${id}” isn't in the plugin store`);
  const plugin = { id, dir: `~/Library/Application Support/dev.winfer.app/plugins/${id}`, enabled: true, fromStore: true, error: null, manifest: { providers: [], recipes: [], tools: [], ...manifest } };
  const i = mockPlugins.findIndex((p) => p.id === id);
  if (i >= 0) mockPlugins[i] = plugin;
  else mockPlugins.push(plugin);
}
// `?kb` starts with the Knowledge Base plugin installed (see ./knowledgeBase.ts).
if (KB_DEMO) installMockPlugin("knowledge-base");
// `?xp` starts with the Cross-Project Knowledge plugin and three linked demo projects (see ./crossProject.ts).
if (XP_DEMO) {
  installMockPlugin("cross-project");
  seedCrossProject();
}
// Fake keychain: the vault keeps values here.
const vault = new Map<string, { info: { key: string; meta: Record<string, unknown>; updatedAt: number }; value: string }>();
// `?db` starts with Databases (one fake Postgres shop, see ./dbExplorer.ts) and Prompt Library.
if (DB_DEMO) {
  installMockPlugin("db-explorer");
  installMockPlugin("prompt-library");
  vault.set(DB_CONNECTION.key, { info: { key: DB_CONNECTION.key, meta: DB_CONNECTION.meta, updatedAt: Date.now() }, value: DB_CONNECTION.value });
}

const SAMPLE_DIFF = `diff --git a/src/common/guards/subscription.guard.ts b/src/common/guards/subscription.guard.ts
index 3f2a1c0..9b8e7d1 100644
--- a/src/common/guards/subscription.guard.ts
+++ b/src/common/guards/subscription.guard.ts
@@ -38,7 +38,8 @@ export class SubscriptionGuard implements CanActivate {
     const subscription = await this.subscriptions.findActive(user.id);
-    if (!subscription) {
+    if (!subscription || subscription.status !== 'active') {
+      throw new ForbiddenException('Subscription inactive');
     }
     return true;
   }`;

const channels = new Map<string, Channel<ArrayBuffer>>();
const lines = new Map<string, string>();

// Fake browser: one tab list per workspace, frames rendered on an OffscreenCanvas.
type MockTab = { id: string; url: string; title: string };
const browserTabs = new Map<string, MockTab[]>();
const screencasts = new Map<string, { tabId: string; channel: Channel<ArrayBuffer>; viewport: any }>();
const inputs: unknown[] = [];
const writes: { paneId: string; data: string }[] = [];
let picking: { workspaceId: string; targetId: string } | null = null;

// Fake voice: tests set what was "said" and the planner's actions ({ tool, args }); the planner
// prompts are kept for inspection.
const voice = { said: "", mode: "command", plan: [] as unknown[], requests: [] as unknown[] };
let tabCounter = 0;
let androidStopped = false;
const androidInputs: unknown[] = [];
// eslint-disable-next-line @typescript-eslint/no-explicit-any
let mergeHistory: any[] = [];

async function renderFrame(workspaceId: string) {
  const sc = screencasts.get(workspaceId);
  const tab = browserTabs.get(workspaceId)?.find((t) => t.id === sc?.tabId);
  if (!sc || !tab) return;
  const { width, height, deviceScaleFactor } = sc.viewport;
  const canvas = new OffscreenCanvas(width * deviceScaleFactor, height * deviceScaleFactor);
  const ctx = canvas.getContext("2d")!;
  ctx.scale(deviceScaleFactor, deviceScaleFactor);
  if (SHOWCASE) {
    drawApp(ctx, width, height);
    const blob = await canvas.convertToBlob({ type: "image/jpeg", quality: 0.92 });
    sc.channel.onmessage(await blob.arrayBuffer());
    return;
  }
  ctx.fillStyle = "#ffffff";
  ctx.fillRect(0, 0, width, height);
  ctx.fillStyle = "#4f46e5";
  ctx.fillRect(0, 0, width, 56);
  ctx.fillStyle = "#ffffff";
  ctx.font = "600 18px system-ui";
  ctx.fillText("◆ orbit", 20, 35);
  ctx.fillStyle = "#111827";
  ctx.font = "600 24px system-ui";
  ctx.fillText(tab.title || "Customer portal", 20, 110);
  ctx.fillStyle = "#6b7280";
  ctx.font = "14px ui-monospace, monospace";
  ctx.fillText(tab.url, 20, 140);
  ctx.fillText(`${width}×${height} · ${inputs.length} input events`, 20, 166);
  const blob = await canvas.convertToBlob({ type: "image/jpeg", quality: 0.85 });
  sc.channel.onmessage(await blob.arrayBuffer());
}

/** A fake app screenshot for review cards. */
function frameDataUrl(): string {
  const canvas = document.createElement("canvas");
  canvas.width = 640;
  canvas.height = 400;
  const ctx = canvas.getContext("2d")!;
  ctx.fillStyle = "#ffffff";
  ctx.fillRect(0, 0, 640, 400);
  ctx.fillStyle = "#4f46e5";
  ctx.fillRect(0, 0, 640, 48);
  ctx.fillStyle = "#ffffff";
  ctx.font = "600 18px system-ui";
  ctx.fillText("◆ orbit", 18, 31);
  ctx.fillStyle = "#111827";
  ctx.font = "600 26px system-ui";
  ctx.fillText("Subscription cancelled", 18, 110);
  ctx.fillStyle = "#6b7280";
  ctx.font = "15px system-ui";
  ctx.fillText("Your plan ends on 1 November.", 18, 140);
  return canvas.toDataURL("image/jpeg", 0.8);
}

if (SHOWCASE) {
  window.setTimeout(async () => {
    await seedShowcase();
    window.setTimeout(() => void stageScene(), 1500);
  }, 1200);
}

// Limit windows reset at fixed times for the session, so their readings line up into a history.
const LOADED = Date.now();
const RESETS = {
  claude: { session: LOADED + 108 * 60_000, week: LOADED + 2.5 * 86400_000 },
  codex: { session: LOADED + 48 * 60_000, week: LOADED + 5 * 86400_000 },
};

/** Readings from the window's start until now, climbing to `used` with some idle stretches. */
function limitSamples(span: number, resetsAt: number, used: number): [number, number][] {
  const start = resetsAt - span;
  const steps = 24;
  return Array.from({ length: steps }, (_, i) => {
    const t = start + ((LOADED - 60_000 - start) * (i + 1)) / steps;
    const f = (i + 1) / steps;
    return [t, Math.round(used * (f + Math.sin(i * 1.3) * 0.04 * (1 - f)) * 10) / 10] as [number, number];
  });
}

void import("@/features/usage/usage").then(({ useUsage }) => {
  const h = 3600_000;
  const d = 24 * h;
  useUsage.setState({
    samples: {
      [`claude:session:${RESETS.claude.session}`]: limitSamples(5 * h, RESETS.claude.session, 38),
      [`claude:week:${RESETS.claude.week}`]: limitSamples(7 * d, RESETS.claude.week, 69),
      [`codex:session:${RESETS.codex.session}`]: limitSamples(5 * h, RESETS.codex.session, 81),
      [`codex:week:${RESETS.codex.week}`]: limitSamples(7 * d, RESETS.codex.week, 6),
    },
  });
});

// Claude Code reports its limits through the status line once a session answers.
window.setTimeout(() => {
  void emit("usage://report", {
    provider: "claude",
    report: {
      windows: [
        { kind: "session", label: "5-hour", usedPercent: 38, resetsAt: RESETS.claude.session },
        { kind: "week", label: "Weekly", usedPercent: 69, resetsAt: RESETS.claude.week },
      ],
    },
  });
}, 2500);

function emitTabs(workspaceId: string) {
  void emit("browser://tabs", { workspaceId, tabs: browserTabs.get(workspaceId) ?? [] });
  void renderFrame(workspaceId);
}
let worktreeCounter = 0;

mockWindows("main");
mockIPC(
  (cmd, args) => {
    const a = args as Record<string, any>;
    switch (cmd) {
      case "providers_list":
        return providers;
      case "editors_list":
        return [{ id: "vscode", name: "VS Code" }, { id: "cursor", name: "Cursor" }, { id: "zed", name: "Zed" }];
      case "editor_open":
        return null;
      case "plugins_list":
        return structuredClone(mockPlugins);
      case "plugins_catalog":
        return new Promise((resolve) => setTimeout(() => resolve(Object.values(storeManifests).map((m) => ({ description: "", tags: [], version: "", ...m }))), 300));
      case "plugins_install":
        return new Promise((resolve, reject) => setTimeout(() => {
          try {
            installMockPlugin(a.id);
            resolve(null);
          } catch (err) {
            reject(String(err));
          }
        }, 500));
      case "plugins_uninstall": {
        const i = mockPlugins.findIndex((p) => p.id === a.id);
        if (i >= 0) mockPlugins.splice(i, 1);
        return null;
      }
      case "vault_list":
        return [...vault.values()].map((v) => v.info).sort((x, y) => x.key.localeCompare(y.key));
      case "vault_set": {
        if (!/^[A-Z_][A-Z0-9_]*$/.test(a.key)) throw new Error(`“${a.key}” isn't a valid name`);
        const old = vault.get(a.key);
        if (!old && a.value == null) throw new Error(`${a.key} needs a value`);
        const info = { key: a.key, meta: a.meta ?? {}, updatedAt: a.value == null ? old!.info.updatedAt : Date.now() };
        vault.set(a.key, { info, value: a.value ?? old!.value });
        return info;
      }
      case "vault_delete":
        vault.delete(a.key);
        return null;
      case "vault_reveal":
        return vault.get(a.key)?.value ?? "";
      case "vault_exec":
        if (a.plugin === "knowledge-base") return knowledgeBaseExec(a.command);
        if (a.plugin === "cross-project") return crossProjectExec(a.command, a.cwd);
        if (a.plugin === "db-explorer") return dbExplorerExec(a.command);
        return { code: 0, timedOut: false, output: `$ ${a.command}\n${a.keys.map((k: string) => `${k}=[secret ${k}]`).join("\n")}` };
      case "plugins_read_file":
        return knowledgeBaseFile(a.path);
      case "plugins_write_file":
        return "/Users/you/Movies/winfer/recording.mp4";
      case "browser_fill_secret":
        return null;
      case "plugins_set_enabled": {
        const plugin = mockPlugins.find((p) => p.id === a.id);
        if (plugin) plugin.enabled = a.enabled;
        return null;
      }
      case "plugins_dir":
        return "~/Library/Application Support/dev.winfer.app/plugins";
      case "plugins_source": {
        const manifest = storeManifests[`../../plugins/${a.id}/plugin.json`] as { main?: string } | undefined;
        return storeSources[`../../plugins/${a.id}/${manifest?.main}`];
      }
      case "state_load":
        if (SHOWCASE) return showcaseState();
        return JSON.parse(localStorage.getItem("winfer-mock-state") ?? '{"workspaces":[],"ui":null}');
      case "state_save":
        if (SHOWCASE) return null;
        localStorage.setItem("winfer-mock-state", JSON.stringify(a.snapshot));
        return null;
      case "pty_spawn": {
        const { paneId, providerId, cwd, resume, prompt } = a.req;
        channels.set(paneId, a.onData);
        if (SHOWCASE && transcript(paneId)) {
          send(a.onData, transcript(paneId)!);
          return { attached: false, pid: 1 };
        }
        send(a.onData, `\x1b[2m[mock] ${providerId}${resume ? " (resumed)" : ""} in ${cwd}${prompt ? ` · prompt: ${prompt}` : ""}\x1b[0m\r\n$ `);
        return { attached: false, pid: 1 };
      }
      case "pty_write": {
        writes.push({ paneId: a.paneId, data: String(a.data) });
        const channel = channels.get(a.paneId);
        if (!channel) return null;
        const line = (lines.get(a.paneId) ?? "") + String(a.data);
        if (!String(a.data).includes("\r")) {
          lines.set(a.paneId, line);
          send(channel, String(a.data).replace(/\x7f/g, "\b \b"));
          return null;
        }
        lines.set(a.paneId, "");
        send(channel, "\r\n");
        // Typing "npm run dev" pretends to start a Vite dev server.
        if (/run dev/.test(line)) send(channel, "\r\n  \x1b[32mVITE\x1b[0m ready in 312 ms\r\n\r\n  ➜  Local:   \x1b[36mhttp://localhost:\x1b[1m5173\x1b[22m/\x1b[0m\r\n");
        send(channel, "$ ");
        return null;
      }
      case "pty_resize":
      case "pty_kill":
        return null;
      case "worktree_create": {
        if (a.dir === "~") return null;
        worktreeCounter += 1;
        return { repo: a.dir, path: `${a.dir}/.worktrees/${a.provider}-${worktreeCounter}`, branch: `winfer/${a.provider}-${worktreeCounter}`, base: "main" };
      }
      case "history_add":
        mergeHistory.unshift({ ...a.merge, commitSha: "3f9c2e1a7b" });
        return null;
      case "history_search": {
        const words = String(a.query).toLowerCase().split(/\s+/).filter(Boolean);
        return mergeHistory.filter(
          (m) => (!a.workspaceId || m.workspaceId === a.workspaceId) && words.every((w) => JSON.stringify(m).toLowerCase().includes(w)),
        );
      }
      case "history_delete":
        mergeHistory = mergeHistory.filter((m) => m.id !== a.id);
        return null;
      case "usage_codex": {
        const now = Date.now();
        return {
          provider: "codex",
          at: now - 60_000,
          windows: [
            { kind: "session", label: "5-hour", usedPercent: 81, resetsAt: RESETS.codex.session },
            { kind: "week", label: "Weekly", usedPercent: 6, resetsAt: RESETS.codex.week },
          ],
        };
      }
      case "usage_codex_context": {
        // Deterministic per folder, so each mock Codex pane shows its own fill.
        const used = 20 + ([...String(a.cwd)].reduce((n, c) => n + c.charCodeAt(0), 0) % 70);
        return { usedPercent: used, tokens: Math.round(475_000 * used / 100), window: 475_000, at: Date.now(), source: "mock.jsonl" };
      }
      case "usage_opencode_context":
        return { usedPercent: 31, tokens: 81_265, window: 262_144, at: Date.now(), source: "mock" };
      case "usage_context_breakdown": {
        // Shares of the reported total, like the real estimate.
        const total = Number(a.tokens);
        const shares: [string, string, number][] = [
          ["system", "System prompt", 0.05],
          ["tools", "Tools", 0.22],
          ["mcp", "MCP tools", 0.08],
          ["instructions", "Memory & instructions", 0.1],
          ["messages", "Your messages", 0.04],
          ["replies", "Replies", 0.06],
          ["toolCalls", "Tool calls", 0.12],
          ["toolResults", "Tool results", 0.33],
        ];
        return shares.map(([kind, label, f]) => ({ kind, label, tokens: Math.round(total * f) }));
      }
      case "usage_dirs":
        return (a.dirs as string[]).map((dir, i) => {
          const f = 1 + ((dir.length * 7 + i) % 5) / 4;
          const model = { provider: "claude", model: "claude-opus-5-5", input: Math.round(42_000 * f), output: Math.round(9_800 * f), cacheRead: Math.round(610_000 * f), cacheWrite: Math.round(38_000 * f), cost: 0.84 * f, priced: true };
          const tokens = model.input + model.output + model.cacheRead + model.cacheWrite;
          return { dir, cost: model.cost, priced: true, tokens, calls: Math.round(31 * f), firstAt: Date.now() - 9 * 60e3 * f, lastAt: Date.now() - 60e3, models: [model] };
        });
      case "usage_opencode_model":
        return { provider: "openai", model: "gpt-5.5", plan: true };
      case "usage_opencode":
        return { tokens: 412_300, cost: 0.83, sessions: 14 };
      case "usage_history": {
        // Hourly rows for the range: busier in working hours, quiet weekends, deterministic.
        const hour = 3600_000;
        const end = Math.floor(Date.now() / hour) * hour;
        const rows = [];
        const models = [
          { provider: "claude", model: "claude-opus-5-5", price: [4, 20, 0.2, 5], scale: 1 },
          { provider: "codex", model: "gpt-6.1-sol", price: [2, 10, 0.1, 2.5], scale: 0.6 },
          { provider: "opencode", model: "kimi-k3", price: [0.6, 2.5, 0.15, 0], scale: 0.25 },
        ];
        for (let at = end - Number(a.days) * 24 * hour; at <= end; at += hour) {
          const d = new Date(at);
          const work = d.getHours() >= 9 && d.getHours() < 20 && d.getDay() % 6 !== 0;
          for (const [i, m] of models.entries()) {
            const wave = (Math.sin(at / (hour * 7) + i * 2) + 1.2) / 2.2;
            if (!work && wave < 0.8) continue;
            const input = Math.round(40_000 * wave * m.scale);
            const output = Math.round(9_000 * wave * m.scale);
            const cacheRead = Math.round(900_000 * wave * m.scale);
            const cacheWrite = Math.round(60_000 * wave * m.scale);
            const [pi, po, pr, pw] = m.price;
            const cost = (input * pi + output * po + cacheRead * pr + cacheWrite * pw) / 1e6;
            rows.push({ hour: at, provider: m.provider, model: m.model, input, output, cacheRead, cacheWrite, cost, priced: true });
          }
        }
        return rows;
      }
      case "android_status":
        return { adb: true, devices: [{ serial: "emulator-5554", state: "device", model: "Pixel 7 (test stream)", emulator: true }], avds: ["Pixel_7_API_35"] };
      case "android_mirror_start": {
        // Plays a real H.264 Annex B stream in adb-sized chunks. Not checked in; make one with
        // ffmpeg -f lavfi -i testsrc2=size=576x1280:rate=30 -t 4 -c:v libx264 -profile:v baseline -f h264 src/dev/test-stream.h264
        const channel = a.onData;
        androidStopped = false;
        return fetch("/src/dev/test-stream.h264").then((stream) => {
          if (!stream.ok || stream.headers.get("content-type")?.includes("html")) throw "no test stream (src/dev/test-stream.h264)";
          void stream.arrayBuffer().then(async (buf) => {
            const bytes = new Uint8Array(buf);
            for (let at = 0; at < bytes.length && !androidStopped; at += 16384) {
              channel.onmessage(bytes.slice(at, at + 16384).buffer);
              await new Promise((r) => setTimeout(r, 30));
            }
          });
          return { width: 1080, height: 2400, videoWidth: 576, videoHeight: 1280 };
        });
      }
      case "android_mirror_stop":
        androidStopped = true;
        return null;
      case "android_screenshot":
        return null;
      case "android_input":
        androidInputs.push(a.input);
        return null;
      case "worktree_branches":
        return [
          { name: "main", remote: false, lastCommitAt: Date.now() - 3600e3, checkedOutAt: a.dir },
          { name: "feature/billing-webhooks", remote: false, lastCommitAt: Date.now() - 2 * 86400e3, checkedOutAt: null },
          { name: "hotfix/login-redirect", remote: false, lastCommitAt: Date.now() - 40 * 60e3, checkedOutAt: "/Users/demo/hotfix-login" },
          { name: "origin/feature/plan-upgrades", remote: true, lastCommitAt: Date.now() - 5 * 86400e3, checkedOutAt: null },
        ];
      case "worktree_fetch":
        return new Promise((resolve) => setTimeout(() => resolve(null), 600));
      case "worktree_from_branch": {
        const branch = a.newBranch ?? String(a.branch).replace(/^origin\//, "");
        return { repo: a.dir, path: `${a.dir}/.worktrees/${branch.replace(/\//g, "-")}`, branch, base: "main" };
      }
      case "worktree_list":
        return String(a.dir).includes("bridgemind")
          ? [
              { info: { repo: a.dir, path: a.dir, branch: "main", base: "main" }, byWinfer: false, prunable: false, createdAt: null, lastCommitAt: Date.now() - 3600e3, ahead: 0, dirty: true, merged: false, isMain: true },
              { info: { repo: a.dir, path: `${a.dir}/.worktrees/claude-7`, branch: "winfer/claude-7", base: "main" }, byWinfer: true, prunable: false, createdAt: Date.now() - 5 * 86400e3, lastCommitAt: Date.now() - 4 * 86400e3, ahead: 0, dirty: false, merged: true },
              { info: { repo: a.dir, path: "/Users/demo/hotfix-login", branch: "hotfix/login-redirect", base: "main" }, byWinfer: false, prunable: false, createdAt: Date.now() - 2 * 3600e3, lastCommitAt: Date.now() - 40 * 60e3, ahead: 3, dirty: true, merged: false },
            ]
          : [];
      case "worktree_diff":
        return { diff: SAMPLE_DIFF, untracked: ["test/subscription.e2e-spec.ts"], untrackedLines: 42, ahead: 2, dirty: false };
      case "worktree_merge":
        return "Merge made by the 'ort' strategy.";
      case "worktree_remove":
      case "worktree_commit":
        return null;
      case "review_test_command":
        return "pnpm test";
      case "review_run_tests":
        return new Promise((resolve) =>
          setTimeout(() => resolve({ command: "pnpm test", ok: false, output: " FAIL  test/subscription.spec.ts > cancelled tier\n   expected 403, received 200", ms: 8400 }), 1200),
        );
      case "browser_screenshot":
        return new Promise((resolve) => setTimeout(() => resolve(frameDataUrl()), 500));
      case "browser_status":
        return { executable: { path: "mock-chrome", source: "system" } };
      case "browser_open": {
        if (!browserTabs.has(a.workspaceId)) browserTabs.set(a.workspaceId, [{ id: `t-${++tabCounter}`, url: "about:blank", title: "" }]);
        return browserTabs.get(a.workspaceId);
      }
      case "browser_new_tab": {
        const tab = { id: `t-${++tabCounter}`, url: a.url, title: "" };
        browserTabs.get(a.workspaceId)!.push(tab);
        emitTabs(a.workspaceId);
        return tab.id;
      }
      case "browser_close_tab":
        browserTabs.set(a.workspaceId, browserTabs.get(a.workspaceId)!.filter((t) => t.id !== a.targetId));
        emitTabs(a.workspaceId);
        return null;
      case "browser_navigate": {
        const tab = browserTabs.get(a.workspaceId)!.find((t) => t.id === a.targetId)!;
        tab.url = a.url;
        tab.title = new URL(a.url).host;
        emitTabs(a.workspaceId);
        // Pretend the page logs an error and a failing API call.
        const entry = (text: string, source: string | null) => ({ level: "error", text, source, timestamp: Date.now() });
        void emit("browser://console", { workspaceId: a.workspaceId, targetId: tab.id, entry: entry("TypeError: Cannot read properties of undefined (reading 'plan')", `${a.url}assets/billing.js:42`) });
        void emit("browser://console", { workspaceId: a.workspaceId, targetId: tab.id, entry: entry(`HTTP 500 ${a.url}api/billing (Fetch)`, null) });
        return null;
      }
      case "browser_attach":
        screencasts.set(a.workspaceId, { tabId: a.targetId, channel: a.onFrame, viewport: a.viewport });
        void renderFrame(a.workspaceId);
        return null;
      case "browser_resize": {
        const sc = screencasts.get(a.workspaceId);
        if (sc) sc.viewport = a.viewport;
        void renderFrame(a.workspaceId);
        return null;
      }
      case "browser_pick_start":
        picking = { workspaceId: a.workspaceId, targetId: a.targetId };
        return null;
      case "browser_pick_cancel":
        picking = null;
        return null;
      case "browser_console":
        return [];
      case "browser_console_clear":
        return null;
      case "browser_input":
        inputs.push(a);
        if (picking && a.params?.type === "mousePressed") {
          const tab = browserTabs.get(picking.workspaceId)?.find((t) => t.id === picking!.targetId);
          picking = null;
          void emit("browser://picked", {
            workspaceId: a.workspaceId,
            url: tab?.url ?? "",
            selector: "main > section.billing > button.upgrade",
            html: '<button class="upgrade" disabled>Upgrade to Pro</button>',
            text: "Upgrade to Pro",
            screenshot: "/tmp/winfer/picks/element-1.png",
          });
        }
        if (a.params?.type === "mousePressed") void renderFrame(a.workspaceId);
        return null;
      case "browser_history":
      case "browser_detach":
      case "browser_log":
        return null;
      case "browser_selection":
        return "";
      case "plugin:app|version":
        return SHOWCASE ? pkg.version : "dev";
      case "voice_status":
        return {
          models: [
            { id: "base.en", file: "ggml-base.en.bin", size: 147964211, label: "Base (English, 148 MB) — fast", installed: true },
            { id: "small.en", file: "ggml-small.en.bin", size: 487614201, label: "Small (English, 488 MB) — more accurate", installed: false },
          ],
          sttModel: "base.en",
          language: "en",
          credentials: true,
          inputDevice: null,
          shortcutError: null,
        };
      case "voice_input_devices":
        return { devices: ["MacBook Pro Microphone", "AirPods Pro"], default: "MacBook Pro Microphone" };
      case "voice_test_microphone":
        return { device: "MacBook Pro Microphone", peak: 0.08 };
      case "voice_configure":
      case "chatgpt_sign_out":
      case "chatgpt_cancel_sign_in":
        return null;
      case "chatgpt_status":
      case "chatgpt_sign_in":
        return { signedIn: true, email: "you@example.com" };
      case "chatgpt_models":
        return ["gpt-6-luna", "gpt-6.1-sol"];
      case "voice_start":
        voice.mode = a.mode;
        void emit("voice://state", { state: "listening", mode: a.mode });
        void emit("voice://level", 0.05);
        return null;
      case "voice_stop":
        void emit("voice://state", { state: "transcribing", mode: voice.mode });
        setTimeout(() => void emit("voice://transcript", { mode: voice.mode, text: voice.said, ms: 142 }), 150);
        return null;
      case "llm_compose":
        if (String(a.req.system).startsWith("You control winfer")) {
          voice.requests.push(a.req.user);
          return new Promise((resolve) => setTimeout(() => resolve(JSON.stringify({ actions: voice.plan })), 200));
        }
        if (String(a.req.system).startsWith("You estimate"))
          return new Promise((resolve) =>
            setTimeout(() => resolve(JSON.stringify({ percent: 20 + Math.floor(Math.random() * 60), step: "Adding the 403 guard test", title: "Cancelled-tier 403" })), 400),
          );
        if (String(a.req.system).startsWith("You review"))
          return new Promise((resolve) =>
            setTimeout(() => resolve("Makes the cancelled-tier guard return 403 instead of 200 and adds an e2e test for it. The old 200 path is deleted, so check that no client relies on it."), 600),
          );
        return new Promise((resolve) => setTimeout(() => resolve(`[written] ${String(a.req.user).match(/Instruction as heard: "([^"]*)"/)?.[1] ?? ""}`), 300));
      case "plugin:dialog|open":
        return "/Users/demo/Projects/bridgemind-api";
      case "plugin:dialog|message": {
        // ask/confirm resolve through `message`: answer with the affirmative button.
        const buttons = a.buttons;
        if (buttons?.OkCancelCustom) return buttons.OkCancelCustom[0];
        if (buttons?.OkCustom) return buttons.OkCustom;
        return buttons === "YesNo" || buttons === "YesNoCancel" ? "Yes" : "Ok";
      }
      case "plugin:notification|is_permission_granted":
        return true;
      default:
        console.debug("[mock] unhandled", cmd, args);
        return null;
    }
  },
  { shouldMockEvents: true },
);

/** Lets automated checks simulate hook events: `__winferMock.status("p-…", "needs-input")`. */
(window as any).__winferMock = {
  androidInputs,
  inputs,
  writes,
  voice,
  /** Simulates an agent opening a tab through CDP. */
  agentTab: (workspaceId: string, url: string) => {
    browserTabs.get(workspaceId)?.push({ id: `t-${++tabCounter}`, url, title: "Dashboard" });
    emitTabs(workspaceId);
  },
  status: (paneId: string, status: string, message?: string) => emit("agent://status", { paneId, status, message }),
};
