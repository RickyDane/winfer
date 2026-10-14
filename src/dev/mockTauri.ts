/**
 * Dev-only fake backend so the UI can run in a plain browser (`pnpm dev`) for quick
 * iteration and automated UI checks. Never loaded inside the Tauri app or in builds.
 */
import type { Channel } from "@tauri-apps/api/core";
import { emit } from "@tauri-apps/api/event";
import { mockIPC, mockWindows } from "@tauri-apps/api/mocks";

const encoder = new TextEncoder();
const send = (channel: Channel<ArrayBuffer>, text: string) => channel.onmessage(encoder.encode(text).buffer as ArrayBuffer);

const providers = [
  { id: "claude", name: "Claude Code", command: "claude", args: [], resumeArgs: ["--continue"], aliases: [], statusHooks: true, promptArg: true, approveInput: null, modelArg: null, modelCommand: null, installed: true },
  { id: "codex", name: "Codex", command: "codex", args: [], resumeArgs: ["resume", "--last"], aliases: [], statusHooks: true, promptArg: true, approveInput: null, modelArg: null, modelCommand: null, installed: true },
  { id: "agy", name: "Antigravity", command: "agy", args: [], resumeArgs: [], aliases: [], statusHooks: false, promptArg: false, approveInput: null, modelArg: null, modelCommand: null, installed: true },
  { id: "grok", name: "Grok Build", command: "grok", args: [], resumeArgs: [], aliases: [], statusHooks: false, promptArg: false, approveInput: null, modelArg: null, modelCommand: null, installed: false },
  { id: "shell", name: "Shell", command: null, args: [], resumeArgs: [], aliases: [], statusHooks: false, promptArg: false, approveInput: null, modelArg: null, modelCommand: null, installed: true },
];

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
type MockTab = { id: string; url: string; title: string; byAgent: boolean };
const browserTabs = new Map<string, MockTab[]>();
const screencasts = new Map<string, { tabId: string; channel: Channel<ArrayBuffer>; viewport: any }>();
const inputs: unknown[] = [];
const writes: { paneId: string; data: string }[] = [];
let picking: { workspaceId: string; targetId: string } | null = null;

// Fake voice: tests set what was "said" and what Clef "answers"; requests are kept for inspection.
const voice = { said: "", mode: "command", answers: {} as Record<string, unknown>, requests: [] as unknown[] };
let tabCounter = 0;

async function renderFrame(workspaceId: string) {
  const sc = screencasts.get(workspaceId);
  const tab = browserTabs.get(workspaceId)?.find((t) => t.id === sc?.tabId);
  if (!sc || !tab) return;
  const { width, height, deviceScaleFactor } = sc.viewport;
  const canvas = new OffscreenCanvas(width * deviceScaleFactor, height * deviceScaleFactor);
  const ctx = canvas.getContext("2d")!;
  ctx.scale(deviceScaleFactor, deviceScaleFactor);
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
      case "state_load":
        return JSON.parse(localStorage.getItem("winfer-mock-state") ?? '{"workspaces":[],"ui":null}');
      case "state_save":
        localStorage.setItem("winfer-mock-state", JSON.stringify(a.snapshot));
        return null;
      case "pty_spawn": {
        const { paneId, providerId, cwd, resume, prompt } = a.req;
        channels.set(paneId, a.onData);
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
      case "worktree_diff":
        return { diff: SAMPLE_DIFF, untracked: ["test/subscription.e2e-spec.ts"], ahead: 2, dirty: false };
      case "worktree_merge":
        return "Merge made by the 'ort' strategy.";
      case "worktree_remove":
        return null;
      case "browser_status":
        return { executable: { path: "mock-chrome", source: "system" } };
      case "browser_open": {
        if (!browserTabs.has(a.workspaceId)) browserTabs.set(a.workspaceId, [{ id: `t-${++tabCounter}`, url: "about:blank", title: "", byAgent: false }]);
        return browserTabs.get(a.workspaceId);
      }
      case "browser_new_tab": {
        const tab = { id: `t-${++tabCounter}`, url: a.url, title: "", byAgent: false };
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
        return null;
      case "browser_selection":
        return "";
      case "voice_status":
        return {
          models: [
            { id: "base.en", file: "ggml-base.en.bin", size: 147964211, label: "Base (English, 148 MB) — fast", installed: true },
            { id: "small.en", file: "ggml-small.en.bin", size: 487614201, label: "Small (English, 488 MB) — more accurate", installed: false },
          ],
          sttModel: "base.en",
          language: "en",
          clefModel: "clef-flash",
          credentials: true,
          inputDevice: null,
          shortcutError: null,
        };
      case "voice_input_devices":
        return { devices: ["MacBook Pro Microphone", "AirPods Pro"], default: "MacBook Pro Microphone" };
      case "voice_test_microphone":
        return { device: "MacBook Pro Microphone", peak: 0.08 };
      case "voice_configure":
      case "voice_set_credentials":
      case "voice_clear_credentials":
        return null;
      case "voice_test_connection":
        return 187;
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
        return new Promise((resolve) => setTimeout(() => resolve(`[written] ${String(a.req.user).match(/Instruction as heard: "([^"]*)"/)?.[1] ?? ""}`), 300));
      case "voice_decide":
        voice.requests.push(a.body);
        return new Promise((resolve) => setTimeout(() => resolve({ answers: voice.answers }), 200));
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
  inputs,
  writes,
  voice,
  /** Simulates an agent opening a tab through CDP. */
  agentTab: (workspaceId: string, url: string) => {
    browserTabs.get(workspaceId)?.push({ id: `t-${++tabCounter}`, url, title: "Dashboard", byAgent: true });
    emitTabs(workspaceId);
  },
  status: (paneId: string, status: string, message?: string) => emit("agent://status", { paneId, status, message }),
};
