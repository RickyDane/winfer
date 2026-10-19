import { useLayoutEffect, useRef, useState, type CSSProperties } from "react";
import type { DesktopClient } from "../client";
import { go, haptic, useClient, type Route } from "../hooks";
import { Icon, type IconName } from "./common";

const TABS: { route: Route["name"]; path: string; label: string; icon: IconName }[] = [
  { route: "inbox", path: "/", label: "Agents", icon: "sparkle" },
  { route: "settings", path: "/settings", label: "Settings", icon: "gear" },
];

/** Floating glass tab bar; a pill slides to the active tab. Stays put across screen transitions. */
export function TabBar({ client, active }: { client: DesktopClient; active: Route["name"] }) {
  const state = useClient(client);
  const waiting = state.panes.filter((p) => p.status === "needs-input").length;
  const ref = useRef<HTMLElement>(null);
  const [pill, setPill] = useState<CSSProperties>({ opacity: 0 });
  useLayoutEffect(() => {
    const el = ref.current?.querySelector<HTMLElement>(`[data-tab="${active}"]`);
    if (el) setPill({ left: el.offsetLeft, width: el.offsetWidth });
  }, [active]);

  return (
    <nav className="tabbar" ref={ref} aria-label="Main">
      <span className="tab-pill" style={pill} />
      {TABS.map((t) => (
        <button
          key={t.route}
          data-tab={t.route}
          className="tab"
          aria-current={t.route === active ? "page" : undefined}
          onClick={() => {
            if (t.route === active) return;
            haptic(6);
            go(t.path);
          }}
        >
          <span className="tab-icon">
            <Icon name={t.icon} size={22} />
            {t.route === "inbox" && waiting > 0 && <span className="tab-badge">{waiting}</span>}
          </span>
          <span className="tab-label">{t.label}</span>
        </button>
      ))}
    </nav>
  );
}
