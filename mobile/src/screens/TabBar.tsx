import type { DesktopClient } from "../client";
import { go, haptic, useClient, type Route } from "../hooks";
import { Icon, linkState, type IconName } from "./common";

const TABS: { route: Route["name"]; path: string; label: string; icon: IconName }[] = [
  { route: "home", path: "/home", label: "Home", icon: "home" },
  { route: "inbox", path: "/", label: "Agents", icon: "messages" },
  { route: "panel", path: "/panel", label: "Panel", icon: "panel" },
  { route: "settings", path: "/settings", label: "Settings", icon: "gear" },
];

/** Sculpted frosted-glass navigation pill paired with a detached Nebula `+` action button. */
export function TabBar({ client, active }: { client: DesktopClient; active: Route["name"] }) {
  const state = useClient(client);
  // Home and Agents each flag their own panes.
  const badge = (home: boolean) => {
    const panes = state.panes.filter((p) => !!p.home === home);
    const waiting = panes.filter((p) => p.status === "needs-input" || p.status === "error").length;
    if (waiting) return <span className="tab-badge">{waiting}</span>;
    return panes.some((p) => p.status === "working") ? <span className="tab-workdot" aria-hidden /> : null;
  };
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
              aria-label={t.label}
              title={t.label}
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
                <Icon name={t.icon} size={22} />
                {(t.route === "home" || t.route === "inbox") && badge(t.route === "home")}
              </span>
            </button>
          ))}
        </div>
        {/* Only the Agents tab offers `+`; kept mounted elsewhere so it can animate in and out. */}
        {canStart && (
          <button
            className="tabbar-fab nebula"
            data-mode={online ? "send" : "idle"}
            data-shown={active === "inbox"}
            aria-hidden={active !== "inbox" || undefined}
            tabIndex={active === "inbox" ? undefined : -1}
            disabled={!online || active !== "inbox"}
            aria-label="New session"
            title="New session"
            onClick={() => {
              haptic(6);
              go("/new");
            }}
          >
            <span className="nebula-face" aria-hidden />
            <Icon name="plus" size={25} className="nebula-icon" />
          </button>
        )}
      </nav>
    </div>
  );
}
