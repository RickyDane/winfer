import type { DesktopClient } from "../client";
import { go, haptic, useClient, type Route } from "../hooks";
import { Icon, linkState, type IconName } from "./common";

const TABS: { route: Route["name"]; path: string; label: string; icon: IconName }[] = [
  { route: "inbox", path: "/", label: "Agents", icon: "sparkle" },
  { route: "panel", path: "/panel", label: "Panel", icon: "panel" },
  { route: "settings", path: "/settings", label: "Settings", icon: "gear" },
];

/** Sculpted frosted-glass navigation pill paired with a detached Nebula `+` action button. */
export function TabBar({ client, active }: { client: DesktopClient; active: Route["name"] }) {
  const state = useClient(client);
  const waiting = state.panes.filter((p) => p.status === "needs-input" || p.status === "error").length;
  const working = state.panes.some((p) => p.status === "working");
  const canStart = state.workspaces.length > 0 && state.providers.length > 0;
  const online = linkState(state) === "online";

  return (
    <div className="tabbar-shell">
      <div className="tabbar-scrim" aria-hidden />
      <nav className="tabbar" aria-label="Main">
        <div className="tabbar-pill" data-active={active}>
          <span className="tabbar-slider" aria-hidden />
          {TABS.map((t) => (
            <button
              key={t.route}
              className="tab"
              aria-current={t.route === active ? "page" : undefined}
              onClick={() => {
                if (t.route === active) {
                  document.querySelector(".scroll.tabbed")?.scrollTo({ top: 0, behavior: "smooth" });
                  return;
                }
                haptic(6);
                go(t.path);
              }}
            >
              <span className="tab-icon">
                <Icon name={t.icon} size={18} />
              </span>
              <span className="tab-label">{t.label}</span>
              {t.route === "inbox" &&
                (waiting > 0 ? <span className="tab-badge">{waiting}</span> : working ? <span className="tab-workdot" aria-hidden /> : null)}
            </button>
          ))}
        </div>
        {canStart && (
          <button
            className="tabbar-fab nebula"
            data-mode={online ? "send" : "idle"}
            disabled={!online}
            aria-label="New session"
            title="New session"
            onClick={() => {
              haptic(6);
              go("/new");
            }}
          >
            <span className="nebula-face" aria-hidden />
            <Icon name="plus" size={22} className="nebula-icon" />
          </button>
        )}
      </nav>
    </div>
  );
}
