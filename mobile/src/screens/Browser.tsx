// The workspace's browser on the phone: its tabs, the shown tab live (screenshots of its viewport),
// taps as clicks, drags as scrolling, typing, the address bar, back/forward/reload and tabs.
import { useEffect, useState } from "react";
import type { DesktopClient } from "../client";
import { haptic, useClient } from "../hooks";
import { forMachine } from "../multi";
import type { BrowserOp, BrowserState, BrowserTab } from "../protocol";
import { Icon, linkState } from "./common";
import { LiveView, type Unit } from "./LiveView";
import { BarButton, ComposeBar, Notice, PanelBar, call, useFrames, usePoll } from "./panelKit";
import { Sheet } from "./Pickers";

/** CDP key events for the keys the phone offers. */
const KEYS: Record<string, { code: string; vk: number; text?: string }> = {
  Enter: { code: "Enter", vk: 13, text: "\r" },
  Backspace: { code: "Backspace", vk: 8 },
  Tab: { code: "Tab", vk: 9 },
  Escape: { code: "Escape", vk: 27 },
  ArrowLeft: { code: "ArrowLeft", vk: 37 },
  ArrowUp: { code: "ArrowUp", vk: 38 },
  ArrowRight: { code: "ArrowRight", vk: 39 },
  ArrowDown: { code: "ArrowDown", vk: 40 },
};

const host = (url: string) => {
  try {
    const u = new URL(url);
    return u.protocol === "about:" ? url : u.host + (u.pathname === "/" ? "" : u.pathname);
  } catch {
    return url;
  }
};

export function Browser({ client, workspaceId }: { client: DesktopClient; workspaceId: string }) {
  const state = useClient(client);
  const ws = state.workspaces.find((w) => w.id === workspaceId);
  // The workspace's own machine, when several are paired.
  const online = linkState(forMachine(state, ws?.machine)) === "online";
  const [bs, setBs] = useState<BrowserState | null>(null);
  const [err, setErr] = useState<string | null>(null);
  const [picked, setPicked] = useState<string | null>(null);
  const [tabsOpen, setTabsOpen] = useState(false);
  const [typing, setTyping] = useState(false);
  const [address, setAddress] = useState<string | null>(null);
  const { frame, error: frameError, drawn } = useFrames(client, "browser");

  const op = <T,>(o: BrowserOp, extra: Record<string, unknown> = {}) => call<T>(client, { t: "browser.call", op: o, workspaceId, ...extra });
  const refresh = (retry = false) =>
    op<BrowserState>("state", { retry })
      .then((s) => {
        setBs(s);
        setErr(null);
      })
      .catch((e: Error) => setErr(e.message));
  usePoll(() => online && void refresh(), 2000, [workspaceId, online]);

  const tabs = bs?.tabs ?? [];
  const shown: BrowserTab | undefined = tabs.find((t) => t.id === picked) ?? tabs.find((t) => t.id === bs?.active) ?? tabs[tabs.length - 1];
  const tab = shown?.id ?? null;

  // Stream the shown tab; again after the phone comes back (the desktop drops quiet phones).
  const maxWidth = Math.round(Math.min(1600, innerWidth * Math.min(2, devicePixelRatio || 1)));
  useEffect(() => {
    if (!tab || !online) return;
    const watch = () => document.visibilityState === "visible" && op("watch", { tab, maxWidth }).catch(() => {});
    watch();
    document.addEventListener("visibilitychange", watch);
    return () => document.removeEventListener("visibilitychange", watch);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [tab, online, workspaceId]);
  useEffect(() => () => client.fire({ t: "browser.call", op: "unwatch", workspaceId }), [client, workspaceId]);

  const input = (method: string, params: Record<string, unknown>) => tab && client.fire({ t: "browser.call", op: "input", workspaceId, tab, method, params });
  const page = (at: Unit) => ({ x: Math.round(at.x * (frame?.width ?? 0)), y: Math.round(at.y * (frame?.height ?? 0)) });
  const click = (at: Unit) => {
    const p = page(at);
    haptic(4);
    input("Input.dispatchMouseEvent", { type: "mouseMoved", ...p });
    input("Input.dispatchMouseEvent", { type: "mousePressed", ...p, button: "left", buttons: 1, clickCount: 1 });
    input("Input.dispatchMouseEvent", { type: "mouseReleased", ...p, button: "left", buttons: 0, clickCount: 1 });
  };
  const scroll = (at: Unit, dx: number, dy: number) => input("Input.dispatchMouseEvent", { type: "mouseWheel", ...page(at), deltaX: dx, deltaY: dy });
  const key = (name: string) => {
    const k = KEYS[name];
    input("Input.dispatchKeyEvent", { type: k.text ? "keyDown" : "rawKeyDown", key: name, code: k.code, windowsVirtualKeyCode: k.vk, nativeVirtualKeyCode: k.vk, ...(k.text && { text: k.text, unmodifiedText: k.text }) });
    input("Input.dispatchKeyEvent", { type: "keyUp", key: name, code: k.code, windowsVirtualKeyCode: k.vk, nativeVirtualKeyCode: k.vk });
  };

  const act = async (o: BrowserOp, extra: Record<string, unknown> = {}) => {
    haptic(6);
    try {
      const v = await op<unknown>(o, extra);
      await refresh();
      return v;
    } catch (e) {
      setErr((e as Error).message);
    }
  };
  const go = (url: string) => {
    setAddress(null);
    (document.activeElement as HTMLElement | null)?.blur();
    if (!url.trim()) return;
    if (tab) void act("navigate", { tab, url });
    else void act("newTab", { url }).then((id) => typeof id === "string" && setPicked(id));
  };

  const body = () => {
    if (!online) return <Notice icon="globe" title="Desktop offline" text="The browser shows again when the desktop is back." />;
    if (!bs) return err ? <Notice icon="alert" title="No browser" text={err} action={{ label: "Retry", onClick: () => void refresh(true) }} /> : <Notice icon="globe" title="Starting the browser…" />;
    if (bs.needsInstall)
      return (
        <Notice
          icon="globe"
          title="No browser on the desktop yet"
          text="winfer downloads its own Chromium (about 100 MB) for agents and this panel."
          action={{ label: bs.installing ? "Downloading…" : "Download it", busy: bs.installing, onClick: () => void act("install") }}
        />
      );
    if (bs.error) return <Notice icon="alert" title="The browser didn't start" text={bs.error} action={{ label: "Retry", onClick: () => void refresh(true) }} />;
    if (!bs.ready) return <Notice icon="globe" title="Starting the browser…" />;
    if (!tab) return <Notice icon="globe" title="No tabs" action={{ label: "New tab", onClick: () => void act("newTab").then((id) => typeof id === "string" && setPicked(id)) }} />;
    return (
      <LiveView
        mode="page"
        frame={frame}
        drawn={drawn}
        onTap={click}
        onScroll={scroll}
        placeholder={<Notice icon="globe" title={frameError ? "Can't show this tab" : "Loading the page…"} text={frameError ?? undefined} />}
      />
    );
  };

  return (
    <div className="screen panel-screen">
      <PanelBar title={shown?.title || (shown ? host(shown.url) : "Browser")} subtitle={ws?.name}>
        <button className="icon-btn tabs-count" aria-label="Tabs" title="Tabs" disabled={!bs?.ready} onClick={() => setTabsOpen(true)}>
          <span>{tabs.length}</span>
        </button>
      </PanelBar>
      <div className="browser-nav">
        <BarButton icon="left" label="Back" disabled={!tab} onClick={() => void act("history", { tab, delta: -1 })} />
        <BarButton icon="forward" label="Forward" disabled={!tab} onClick={() => void act("history", { tab, delta: 1 })} />
        <form
          className="address"
          onSubmit={(e) => {
            e.preventDefault();
            go(address ?? "");
          }}
        >
          {shown?.favicon ? <img src={shown.favicon} alt="" /> : <Icon name="globe" size={14} />}
          <input
            value={address ?? (shown ? (shown.url === "about:blank" ? "" : shown.url) : "")}
            placeholder="Search or enter address"
            inputMode="url"
            enterKeyHint="go"
            autoCapitalize="off"
            autoCorrect="off"
            spellCheck={false}
            onFocus={(e) => {
              setAddress(shown?.url === "about:blank" ? "" : (shown?.url ?? ""));
              requestAnimationFrame(() => e.target.select());
            }}
            onBlur={() => setAddress(null)}
            onChange={(e) => setAddress(e.target.value)}
          />
        </form>
        <BarButton icon="refresh" label="Reload" disabled={!tab} onClick={() => void act("history", { tab, delta: 0 })} />
        <BarButton icon="keyboard" label="Type" active={typing} disabled={!tab} onClick={() => setTyping((t) => !t)} />
      </div>
      {err && bs && <p className="inline-error panel-error">{err}</p>}
      <div className="panel-stage">{body()}</div>
      {typing && tab && (
        <ComposeBar
          placeholder="Type into the page"
          onText={(text, enter) => {
            if (text) input("Input.insertText", { text });
            if (enter) key("Enter");
          }}
          keys={[
            { label: <Icon name="del" size={16} />, title: "Backspace", onPress: () => key("Backspace") },
            { label: "Tab", title: "Tab", onPress: () => key("Tab") },
            { label: "Esc", title: "Escape", onPress: () => key("Escape") },
            { label: "←", title: "Left", onPress: () => key("ArrowLeft") },
            { label: "↑", title: "Up", onPress: () => key("ArrowUp") },
            { label: "↓", title: "Down", onPress: () => key("ArrowDown") },
            { label: "→", title: "Right", onPress: () => key("ArrowRight") },
          ]}
        />
      )}
      {tabsOpen && (
        <Sheet title="Tabs" sub={ws?.name} label="Tabs" onClose={() => setTabsOpen(false)}>
          <div className="sheet-body">
            {tabs.map((t) => (
              <div key={t.id} className="pick-row tab-row" data-on={t.id === tab || undefined}>
                <button
                  className="tab-row-main"
                  onClick={() => {
                    setPicked(t.id);
                    setTabsOpen(false);
                    void op("select", { tab: t.id }).catch(() => {});
                  }}
                >
                  {t.favicon ? <img src={t.favicon} alt="" /> : <Icon name="globe" size={15} />}
                  <span className="row-main">
                    <b>{t.title || host(t.url) || "New tab"}</b>
                    <span>{host(t.url)}</span>
                  </span>
                </button>
                <button className="icon-btn" aria-label="Close tab" onClick={() => void act("closeTab", { tab: t.id })}>
                  <Icon name="x" size={15} />
                </button>
              </div>
            ))}
            <button
              className="pick-row"
              onClick={() => {
                setTabsOpen(false);
                void act("newTab").then((id) => typeof id === "string" && setPicked(id));
              }}
            >
              <Icon name="plus" size={16} />
              <span className="row-main">
                <b>New tab</b>
              </span>
            </button>
          </div>
        </Sheet>
      )}
    </div>
  );
}
