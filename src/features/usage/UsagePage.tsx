import { ChartColumn, Gauge, Wrench } from "lucide-react";
import { cn } from "@/lib/cn";
import { ChartKindToggle, USAGE_TAB_KEY, useSaved, type ChartKind, type UsageTab } from "./controls";
import { ToolStats } from "./ToolStatsTab";
import { UsageHistory } from "./UsageHistory";
import { Limits } from "./UsageMeter";

const TABS: { id: UsageTab; label: string; icon: typeof Gauge }[] = [
  { id: "usage", label: "Usage", icon: ChartColumn },
  { id: "limits", label: "Limits", icon: Gauge },
  { id: "tools", label: "Tools", icon: Wrench },
];

/** The agents' plan limits over the current windows, as bars or lines. */
function LimitsTab() {
  const [chart, setChart] = useSaved<ChartKind>("winfer.limitsChart", ["bars", "line"], "line");
  return (
    <div className="space-y-4">
      <div className="flex items-start gap-3">
        <p className="flex-1 text-xs text-fg-muted">
          How much of each plan's usage limits is left, as the title bar's rings show it. An agent with several accounts counts each as 100% and shows their average. The lines follow each window since it started; a dashed line is where it
          runs out at the current pace.
        </p>
        <ChartKindToggle value={chart} onChange={setChart} />
      </div>
      <Limits chart={chart} accounts />
    </div>
  );
}

/** Settings → Usage: token use and API cost over time, the plan limits, or the agents' tool calls. `children` goes on the usage tab. */
export function UsageSettings({ children }: { children?: React.ReactNode }) {
  const [tab, setTab] = useSaved<UsageTab>(USAGE_TAB_KEY, ["usage", "limits", "tools"], "usage");
  return (
    <div className="space-y-6">
      <div role="tablist" className="flex w-max items-center gap-0.5 rounded-lg border border-border bg-panel p-0.5">
        {TABS.map((t) => (
          <button
            key={t.id}
            role="tab"
            aria-selected={tab === t.id}
            onClick={() => setTab(t.id)}
            className={cn(
              "flex items-center gap-1.5 rounded-md px-2.5 py-1 text-xs transition-colors duration-150",
              tab === t.id ? "bg-active font-medium text-fg" : "text-fg-muted hover:bg-hover hover:text-fg",
            )}
          >
            <t.icon size={12} />
            {t.label}
          </button>
        ))}
      </div>
      {tab === "usage" ? (
        <UsageHistory>{children}</UsageHistory>
      ) : tab === "tools" ? (
        <ToolStats />
      ) : (
        <LimitsTab />
      )}
    </div>
  );
}
