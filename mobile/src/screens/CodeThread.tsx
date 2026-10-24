import { useEffect, useMemo, useState } from "react";
import { ChevronLeft, X } from "lucide-react";
import { createCodeStore } from "@/features/code/store";
import { CodeView } from "@/features/code/CodeView";
import { applyTheme, themeById } from "@/features/theme/themes";
import { ProviderIcon } from "@/features/providers/ProviderIcon";
import type { LinkHost } from "@/features/code/Links";
import type { DesktopClient } from "../client";
import { RelayCodeTransport } from "../codeTransport";
import { go, useClient } from "../hooks";
import "../code-mobile.css";

/** On the phone, web links open in a new tab; file paths stay text (the files are on the desktop). */
const PHONE_LINKS: LinkHost = {
  resolve: (paths) => Promise.resolve(paths.map(() => null)),
  openWeb: (_e, url) => void window.open(url, "_blank", "noopener"),
  openFile: () => {},
  fileHint: (abs) => abs,
  webHint: "Open in a new tab",
};

/**
 * A winfer code pane on the phone: the desktop's own web UI (src/features/code), mirroring the
 * desktop pane's session, under a header like the desktop pane's.
 */
export function CodeThread({ client, paneId }: { client: DesktopClient; paneId: string }) {
  const state = useClient(client);
  const pane = state.panes.find((p) => p.id === paneId);
  const store = useMemo(() => createCodeStore({ remote: true }), [paneId]);
  const [theme, setTheme] = useState<string | null>(null);
  const [branch, setBranch] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => store.getState().attach(new RelayCodeTransport(client, paneId, store, (t, b) => {
          setTheme(t ?? null);
          setBranch(b ?? null);
        }, setError)), [client, paneId, store]);

  // The desktop's theme while the pane is open, so it looks the same.
  useEffect(() => {
    if (!theme) return;
    applyTheme(theme);
    const meta = document.querySelector('meta[name="theme-color"]');
    const before = meta?.getAttribute("content");
    meta?.setAttribute("content", themeById(theme).ui.panel);
    return () => {
      const root = document.documentElement;
      root.removeAttribute("style");
      delete root.dataset.theme;
      delete root.dataset.mode;
      if (before) meta?.setAttribute("content", before);
    };
  }, [theme]);

  const dark = theme ? themeById(theme).mode !== "light" : true;
  const offline = state.socket !== "open" || state.desktopOnline === false;

  return (
    <div className="cw-phone">
      <header className="cw-phone-head">
        <button className="cw-phone-btn" aria-label="Back" onClick={() => go("/")}>
          <ChevronLeft size={18} />
        </button>
        <ProviderIcon provider="winfer" size={14} />
        <span className="cw-phone-title">{pane?.title ?? "winfer code"}</span>
        <span className="cw-phone-ws">{pane?.workspace}</span>
        <span className="cw-phone-dot" data-status={pane?.status ?? "idle"} />
        <button
          className="cw-phone-btn"
          aria-label="Close pane"
          onClick={() => {
            if (!confirm("Close this pane on your desktop?")) return;
            void client.request({ t: "pane.close", paneId }).then(() => go("/"));
          }}
        >
          <X size={15} />
        </button>
      </header>
      {(offline || error) && (
        <div className="cw-phone-notice" role="status">
          {offline ? (state.desktopOnline === false ? "Your desktop is offline." : "Connecting…") : error}
          {offline && (
            <button onClick={() => client.reconnect()} className="cw-btn">
              Retry
            </button>
          )}
        </div>
      )}
      <CodeView store={store} links={PHONE_LINKS} dark={dark} focused={false} branch={branch} />
    </div>
  );
}
