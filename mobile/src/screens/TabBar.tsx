import type { DesktopClient } from "../client";
import { go, haptic, useClient, type Route } from "../hooks";
import { Icon, type IconName } from "./common";

const TABS: { route: Route["name"]; path: string; label: string; icon: IconName }[] = [
  { route: "inbox", path: "/", label: "Agents", icon: "sparkle" },
  { route: "settings", path: "/settings", label: "Settings", icon: "gear" },
];

/** Flat bottom tab bar. Stays put across screen transitions. */
export function TabBar({ client, active }: { client: DesktopClient; active: Route["name"] }) {
  const state = useClient(client);
  const waiting = state.panes.filter((p) => p.status === "needs-input").length;

  return (
    <nav className="tabbar" aria-label="Main">
      {TABS.map((t) => (
        <button
          key={t.route}
          className="tab"
          aria-current={t.route === active ? "page" : undefined}
          onClick={() => {
            if (t.route === active) return;
            haptic(6);
            go(t.path);
          }}
        >
          <span className="tab-icon">
            <Icon name={t.icon} size={20} />
            {t.route === "inbox" && waiting > 0 && <span className="tab-badge">{waiting}</span>}
          </span>
          <span className="tab-label">{t.label}</span>
        </button>
      ))}
    </nav>
  );
}
