import { useEffect, useRef } from "react";
import type { Pane } from "@/stores/workspaces";
import { attachTerminal, detachTerminal, focusTerminal } from "./xtermRegistry";

/** Slot that the pane's long-lived xterm instance is attached to. */
export function TerminalHost({ pane, focused }: { pane: Pane; focused: boolean }) {
  const slot = useRef<HTMLDivElement>(null);

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
