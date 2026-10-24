import { useEffect, useMemo, useRef, type CSSProperties } from "react";
import { useWorkspaces, type Pane } from "@/stores/workspaces";
import { themeById } from "@/features/theme/themes";
import { createCodeStore, useCode } from "./store";
import { MockTransport } from "./mock";
import { CODE_WEB_DEMO, stepTextScale, useCodeWeb } from "./styles";
import { PtyTransport } from "./transport";
import { CodeView } from "./CodeView";
import { desktopLinks } from "./desktopLinks";

/** Keeps the pane's status dot in step with the session (working while a turn runs). */
function StatusSync({ paneId }: { paneId: string }) {
  const busy = useCode((s) => s.busy);
  const asking = useCode((s) => s.approvals.length > 0);
  useEffect(() => {
    useWorkspaces
      .getState()
      .setPaneStatus(
        paneId,
        asking ? "needs-input" : busy ? "working" : "idle",
      );
  }, [paneId, busy, asking]);
  return null;
}

const stores = new Map<string, ReturnType<typeof createCodeStore>>();

/** Ends a pane's session view (the PTY itself is killed with the pane, see closeTerminal). */
export function disposeCodePane(paneId: string) {
  stores.delete(paneId);
}

/** One store per pane that outlives remounts (moving tiles, switching workspaces). */
export function storeFor(pane: Pane) {
  let s = stores.get(pane.id);
  if (!s) {
    s = createCodeStore();
    stores.set(pane.id, s);
    if (CODE_WEB_DEMO) {
      // The browser preview: a scripted session.
      s.getState().attach(new MockTransport());
      const prompt =
        pane.prompt ??
        "Add exponential backoff to retry() and make it cancellable";
      setTimeout(() => s!.getState().submit(prompt), 3500);
    } else {
      s.getState().attach(new PtyTransport(pane));
    }
  }
  return s;
}

/** winfer code as a web UI: what a winfer pane shows instead of the terminal TUI. */
export function CodePane({ pane, focused }: { pane: Pane; focused: boolean }) {
  const store = storeFor(pane);
  const dark = useWorkspaces((s) => themeById(s.theme).mode) !== "light";
  const glass = useWorkspaces((s) => s.glassPanes);
  const textScale = useCodeWeb((s) => s.textScale);
  const root = useRef<HTMLDivElement>(null);

  // Ctrl/⌘ + wheel (and trackpad pinch, which arrives as Ctrl + wheel) resizes the text.
  useEffect(() => {
    const el = root.current;
    if (!el) return;
    let pinch = 0;
    const onWheel = (e: WheelEvent) => {
      if (!e.ctrlKey && !e.metaKey) return;
      e.preventDefault();
      e.stopPropagation();
      pinch -= e.deltaY;
      if (Math.abs(pinch) < 40) return;
      stepTextScale(Math.sign(pinch));
      pinch = 0;
    };
    el.addEventListener("wheel", onWheel, { passive: false, capture: true });
    return () => el.removeEventListener("wheel", onWheel, { capture: true });
  }, []);

  useEffect(() => {
    if (focused) root.current?.querySelector("textarea")?.focus();
  }, [focused]);

  const links = useMemo(() => desktopLinks(pane.id), [pane.id]);
  return (
    <CodeView
      store={store}
      links={links}
      dark={dark}
      glass={glass}
      focused={focused}
      branch={pane.worktree?.branch}
      rootRef={root}
      style={{ "--cw-text": textScale } as CSSProperties}
    >
      {CODE_WEB_DEMO && <StatusSync paneId={pane.id} />}
    </CodeView>
  );
}
