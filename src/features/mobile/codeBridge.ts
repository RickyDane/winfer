/**
 * winfer code's web UI on the phone (docs/mobile/protocol.md "winfer code"): the phone renders
 * the same components as the desktop pane (src/features/code), mirroring the pane's store. It
 * attaches with `code.attach` and gets the session state (`code.snapshot`, in parts that fit the
 * relay's frames), then every event the store applies (`code.ev`); its commands come back as
 * `code.cmd` and go through the desktop's store, so both stay in step.
 */
import { invoke } from "@tauri-apps/api/core";
import { useWorkspaces, type Pane } from "@/stores/workspaces";
import { storeFor } from "@/features/code/CodePane";
import { paneUsesWeb } from "@/features/code/styles";
import type { Cmd, Ev } from "@/features/code/protocol";
import type { Attachment } from "@/features/code/store";

/** Characters per message: well under the relay's 1 MiB frame once encrypted and encoded. */
export const CHUNK = 200_000;
/** Big tool output in a live event is cut to its tail (the desktop keeps all of it). */
const MAX_FIELD = 150_000;

const send = (device: string, msg: unknown) => invoke("mobile_send", { device, msg }).catch(() => {});

/** Whether a pane shows winfer code's web UI, which the phone can mirror. */
export const isCodeWeb = (pane: Pane) => pane.provider === "winfer" && paneUsesWeb(pane.id);

/** `text` in parts of at most `size` characters. */
export function chunks(text: string, size = CHUNK): string[] {
  const out: string[] = [];
  for (let i = 0; i < text.length; i += size) out.push(text.slice(i, i + size));
  return out.length ? out : [""];
}

/** An event small enough for one frame: long text fields keep their tail. */
export function fitEvent(ev: Ev): Ev {
  if (JSON.stringify(ev).length <= CHUNK) return ev;
  const cut = (s: string) => (s.length > MAX_FIELD ? `…${s.slice(-MAX_FIELD)}` : s);
  const out: Record<string, unknown> = { ...ev };
  for (const k of ["output", "chunk", "text", "diff", "answer"]) if (typeof out[k] === "string") out[k] = cut(out[k] as string);
  if (Array.isArray(out.images)) delete out.images;
  return out as Ev;
}

type Sub = { off: () => void; queue: Ev[]; timer: ReturnType<typeof setTimeout> | null };
/** Phones watching a pane, by `device\0paneId`. */
const subs = new Map<string, Sub>();

export function detach(device: string, paneId?: string) {
  for (const [k, sub] of subs) {
    if (!k.startsWith(`${device}\0`) || (paneId && k !== `${device}\0${paneId}`)) continue;
    sub.off();
    if (sub.timer) clearTimeout(sub.timer);
    subs.delete(k);
  }
}

/** The pane's state for the phone, then its events, batched a little to save frames. */
export function attach(device: string, pane: Pane, reqId?: string) {
  detach(device, pane.id);
  const store = storeFor(pane);
  const data = JSON.stringify(store.getState().snapshot());
  const parts = chunks(data);
  // The desktop's theme comes along, so the phone looks the same.
  const theme = useWorkspaces.getState().theme;
  parts.forEach(
    (part, i) =>
      void send(device, { t: "code.snapshot", id: i === parts.length - 1 ? reqId : undefined, paneId: pane.id, part: i, parts: parts.length, data: part, ...(i === 0 && { theme, branch: pane.worktree?.branch ?? null }) }),
  );
  const key = `${device}\0${pane.id}`;
  const sub: Sub = { off: () => {}, queue: [], timer: null };
  const flush = () => {
    sub.timer = null;
    const evs = sub.queue.splice(0);
    // A batch that would be too big goes one event per frame.
    if (JSON.stringify(evs).length <= CHUNK) void send(device, { t: "code.ev", paneId: pane.id, evs });
    else for (const ev of evs) void send(device, { t: "code.ev", paneId: pane.id, evs: [ev] });
  };
  sub.off = store.getState().listen((ev) => {
    sub.queue.push(fitEvent(ev));
    sub.timer ??= setTimeout(flush, 40);
  });
  subs.set(key, sub);
}

/** A command from the phone, run as if it was given in the desktop's pane. */
export function command(pane: Pane, cmd: Cmd) {
  const store = storeFor(pane).getState();
  // Asking for something only the phone shows: the desktop's panels stay as they are.
  if (PHONE_ONLY.has(cmd.type)) return store.transport?.send(cmd);
  if (cmd.type === "send" || cmd.type === "steer") {
    // Through submit, so the desktop shows the turn (and steers if a turn is running).
    const attachments: Attachment[] = (cmd.images ?? []).map((im, i) => ({
      id: `phone-${Date.now()}-${i}`,
      url: `data:${im.mime};base64,${im.data}`,
      mime: im.mime,
      name: "image from phone",
    }));
    return store.submit(cmd.text, attachments);
  }
  store.send(cmd);
}

const PHONE_ONLY = new Set<Cmd["type"]>(["btw", "sessions", "task_log", "diff", "export"]);
