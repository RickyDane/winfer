import { useEffect, useState } from "react";
import { ProviderIcon } from "@/features/providers/ProviderIcon";
import { relativeTime, useActivity } from "@/stores/activity";
import { useWorkspaces } from "@/stores/workspaces";

/** "Latest" list at the bottom of the sidebar: what agents and you did recently in this workspace. */
export function ActivityFeed() {
  const workspaceId = useWorkspaces((s) => s.activeWorkspaceId);
  const events = useActivity((s) => s.events).filter((e) => e.workspaceId === workspaceId);
  const [expanded, setExpanded] = useState(false);
  const [, tick] = useState(0);

  useEffect(() => {
    const id = window.setInterval(() => tick((n) => n + 1), 30_000); // refresh relative times
    return () => window.clearInterval(id);
  }, []);

  if (events.length === 0) return null;
  const shown = expanded ? events.slice(0, 40) : events.slice(0, 4);

  return (
    <section className="border-t border-border px-2 pt-2 pb-1">
      <div className="flex items-center justify-between px-2 pb-1">
        <span className="text-xs font-medium tracking-wide text-fg-subtle">Latest</span>
        {events.length > 4 && (
          <button onClick={() => setExpanded((e) => !e)} className="text-[11px] text-accent hover:underline">
            {expanded ? "Show less" : "View all"}
          </button>
        )}
      </div>
      <div className={expanded ? "max-h-72 overflow-y-auto" : ""}>
        {shown.map((e) => (
          <div key={e.id} className="flex items-start gap-2 rounded-md px-2 py-1.5 text-[12px]" title={e.text}>
            <span className="mt-0.5 shrink-0">{e.provider ? <ProviderIcon provider={e.provider} size={12} /> : <span className="block size-3 rounded-full bg-border-strong" />}</span>
            <span className="line-clamp-2 flex-1 text-fg-muted">{e.text}</span>
            <span className="shrink-0 text-[11px] text-fg-subtle">{relativeTime(e.at)}</span>
          </div>
        ))}
      </div>
    </section>
  );
}
