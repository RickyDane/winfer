import { useEffect, useRef } from "react";
import type { Pane } from "@/stores/workspaces";
import { attachTerminal, detachTerminal, focusTerminal, pasteText } from "./xtermRegistry";
import { isTauri } from "@tauri-apps/api/core";
import { getCurrentWebview } from "@tauri-apps/api/webview";

/** Slot that the pane's long-lived xterm instance is attached to. */
export function TerminalHost({ pane, focused }: { pane: Pane; focused: boolean }) {
  const slot = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!isTauri()) return;
    let disposed = false;
    const listener = getCurrentWebview().onDragDropEvent(({ payload }) => {
      if (disposed || payload.type !== "drop") return;
      const rect = slot.current?.getBoundingClientRect();
      const x = payload.position.x / window.devicePixelRatio;
      const y = payload.position.y / window.devicePixelRatio;
      if (!rect || x < rect.left || x > rect.right || y < rect.top || y > rect.bottom) return;
      // Bracketed paste keeps a drop in the prompt instead of submitting a turn.
      pasteText(pane.id, payload.paths.map((path) => `"${path.replaceAll('"', '\\"')}"`).join(" "));
      focusTerminal(pane.id);
    });
    return () => { disposed = true; void listener.then((unlisten) => unlisten()).catch(() => {}); };
  }, [pane.id]);

  useEffect(() => {
    attachTerminal(pane, slot.current!);
    return () => detachTerminal(pane.id);
    // The pane object changes on status updates; only the id matters for attachment.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [pane.id]);

  useEffect(() => {
    if (focused) focusTerminal(pane.id);
  }, [focused, pane.id]);

  return <div ref={slot} className="h-full w-full overflow-hidden" />;
}
