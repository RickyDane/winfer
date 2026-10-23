/**
 * Talks to a real winfer code session: `winfer code --serve` in the pane's PTY (spawned like
 * the TUI would be, with winfer's tools, hooks and worktree), JSON lines both ways.
 */
import { Channel, invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { hiddenToolsOf, systemPromptOf, useWorkspaces, type Pane } from "@/stores/workspaces";
import { createWorktree } from "@/features/worktree/worktree";
import type { Cmd, Ev, Transport } from "./protocol";

export class PtyTransport implements Transport {
  private queue: string[] = [];
  private live = false;
  private writing: Promise<unknown> = Promise.resolve();

  constructor(private pane: Pane) {}

  connect(onEvent: (ev: Ev) => void) {
    const decoder = new TextDecoder();
    let buf = "";
    const channel = new Channel<ArrayBuffer>();
    channel.onmessage = (data) => {
      buf += decoder.decode(new Uint8Array(data), { stream: true });
      let nl: number;
      while ((nl = buf.search(/[\r\n]/)) >= 0) {
        const line = buf.slice(0, nl).trim();
        buf = buf.slice(nl + 1);
        if (!line.startsWith("{")) {
          // Anything else is the process talking past the protocol (a crash, a warning).
          if (line) onEvent({ type: "notice", text: line });
          continue;
        }
        try {
          onEvent(JSON.parse(line) as Ev);
        } catch {
          onEvent({ type: "notice", text: line });
        }
      }
    };
    let pid: number | null | undefined;
    const unlisten = listen<{ paneId: string; code: number; pid: number | null }>("pty://exit", ({ payload }) => {
      if (payload.paneId !== this.pane.id || (payload.pid != null && pid != null && payload.pid !== pid)) return;
      this.live = false;
      onEvent({ type: "exited", text: `winfer code exited (code ${payload.code})` });
      useWorkspaces.getState().setPaneStatus(this.pane.id, payload.code === 0 ? "done" : "error");
    });

    void (async () => {
      let pane = this.pane;
      if (pane.wantsWorktree && !pane.worktree) {
        try {
          const worktree = await createWorktree(pane.cwd, pane.provider);
          const patch = worktree ? { worktree, cwd: worktree.path, wantsWorktree: false } : { wantsWorktree: false };
          useWorkspaces.getState().updatePane(pane.id, patch);
          pane = { ...pane, ...patch };
          if (worktree) onEvent({ type: "notice", text: `Worktree ${worktree.branch} at ${worktree.path}` });
        } catch (err) {
          useWorkspaces.getState().updatePane(pane.id, { wantsWorktree: false });
          onEvent({ type: "error", text: `Couldn't create a worktree, starting in ${pane.cwd}: ${err}` });
        }
      }
      const s = useWorkspaces.getState();
      try {
        const res = await invoke<{ pid: number | null }>("pty_spawn", {
          req: {
            paneId: pane.id,
            providerId: pane.provider,
            cwd: pane.cwd,
            workspaceId: pane.workspaceId,
            resume: pane.resume,
            session: pane.resume ? pane.sessionId : null,
            prompt: pane.prompt,
            model: pane.model,
            autoApprove: s.autoApprove.includes(pane.provider),
            systemPrompt: systemPromptOf(s),
            plain: pane.plain,
            lean: false,
            hiddenTools: hiddenToolsOf(s),
            serve: true,
            cols: 200,
            rows: 50,
          },
          onData: channel,
        });
        pid = res.pid;
        this.live = true;
        for (const line of this.queue.splice(0)) this.write(line);
      } catch (err) {
        onEvent({ type: "exited", text: `Couldn't start winfer code: ${err}` });
        useWorkspaces.getState().setPaneStatus(pane.id, "error");
      }
    })();

    return () => {
      void unlisten.then((f) => f());
    };
  }

  private write(line: string) {
    // One write at a time keeps commands in order.
    this.writing = this.writing.then(() => invoke("pty_write", { paneId: this.pane.id, data: line }).catch(() => {}));
  }

  send(cmd: Cmd) {
    const line = JSON.stringify(cmd) + "\n";
    if (this.live) this.write(line);
    else this.queue.push(line);
  }
}
