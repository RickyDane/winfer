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
  { id: "claude", name: "Claude Code", command: "claude", args: [], resumeArgs: ["--continue"], aliases: [], statusHooks: true, installed: true },
  { id: "codex", name: "Codex", command: "codex", args: [], resumeArgs: ["resume", "--last"], aliases: [], statusHooks: true, installed: true },
  { id: "agy", name: "Antigravity", command: "agy", args: [], resumeArgs: [], aliases: [], statusHooks: false, installed: true },
  { id: "grok", name: "Grok Build", command: "grok", args: [], resumeArgs: [], aliases: [], statusHooks: false, installed: false },
  { id: "shell", name: "Shell", command: null, args: [], resumeArgs: [], aliases: [], statusHooks: false, installed: true },
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
        const { paneId, providerId, cwd, resume } = a.req;
        channels.set(paneId, a.onData);
        send(a.onData, `\x1b[2m[mock] ${providerId}${resume ? " (resumed)" : ""} in ${cwd}\x1b[0m\r\n$ `);
        return { attached: false, pid: 1 };
      }
      case "pty_write": {
        const channel = channels.get(a.paneId);
        if (channel) send(channel, String(a.data).replace(/\r/g, "\r\n$ ").replace(/\x7f/g, "\b \b"));
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
  status: (paneId: string, status: string, message?: string) => emit("agent://status", { paneId, status, message }),
};
