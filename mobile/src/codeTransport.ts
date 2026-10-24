/**
 * winfer code on the phone: a Transport for the desktop's web UI components (src/features/code)
 * that mirrors a desktop pane through the relay (docs/mobile/protocol.md "winfer code"). It
 * attaches, loads the session state from `code.snapshot` and then applies `code.ev`; commands go
 * back as `code.cmd`. It re-attaches whenever the desktop comes back.
 */
import type { StoreApi } from "zustand";
import type { Cmd, Ev, Transport } from "@/features/code/protocol";
import type { CodeSnapshot, CodeState } from "@/features/code/store";
import type { DesktopClient } from "./client";

/** Commands bigger than this (images, mostly) wouldn't fit through the relay. */
const MAX_CMD = 600_000;

export class RelayCodeTransport implements Transport {
  private emit: (ev: Ev) => void = () => {};

  constructor(
    private client: DesktopClient,
    private paneId: string,
    private store: StoreApi<CodeState>,
    /** The desktop's theme and the pane's worktree branch, with each snapshot. */
    private onLook?: (theme?: string, branch?: string | null) => void,
    private onError?: (error: string | null) => void,
  ) {}

  connect(onEvent: (ev: Ev) => void) {
    this.emit = onEvent;
    let parts: string[] = [];
    let got = 0;
    let loaded = false;
    let early: Ev[] = [];
    const off = this.client.listen((msg) => {
      if (msg.t === "code.snapshot" && msg.paneId === this.paneId) {
        if (msg.part === 0) {
          parts = [];
          got = 0;
          loaded = false;
          early = [];
          this.onLook?.(msg.theme, msg.branch);
        }
        if (parts[msg.part] === undefined) got++;
        parts[msg.part] = msg.data;
        if (got < msg.parts) return;
        try {
          this.store.getState().load(JSON.parse(parts.join("")) as CodeSnapshot);
        } catch {
          return this.onError?.("Couldn't read the session from your desktop.");
        }
        loaded = true;
        for (const ev of early.splice(0)) onEvent(ev);
        parts = [];
      } else if (msg.t === "code.ev" && msg.paneId === this.paneId) {
        // Events from before the snapshot finished arriving apply after it.
        for (const ev of msg.evs) (loaded ? onEvent(ev) : early.push(ev));
      }
    });

    // (Re)attach whenever the desktop is reachable again: the snapshot catches the phone up.
    let online = false;
    const check = () => {
      const s = this.client.getState();
      const now = s.socket === "open" && s.desktopOnline === true;
      if (now && !online) {
        this.client
          .request({ t: "code.attach", paneId: this.paneId })
          .then(() => this.onError?.(null))
          .catch((e: Error) => this.onError?.(e.message));
      }
      online = now;
    };
    const unsub = this.client.subscribe(check);
    check();

    return () => {
      off();
      unsub();
      this.client.request({ t: "code.detach", paneId: this.paneId }).catch(() => {});
    };
  }

  send(cmd: Cmd) {
    if (JSON.stringify(cmd).length > MAX_CMD) {
      this.emit({ type: "error", text: "That's too big to send from your phone (try fewer or smaller images)." });
      return;
    }
    this.client.request({ t: "code.cmd", paneId: this.paneId, cmd }).catch((e: Error) => this.emit({ type: "error", text: e.message }));
  }
}
