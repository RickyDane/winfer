import { ProviderIcon } from "@/features/providers/ProviderIcon";
import { providerName, useWorkspaces } from "@/stores/workspaces";
import { useDrag } from "./drag";

/** Ghost of the dragged pane and a highlight of where it will land. */
export function DragLayer() {
  const { paneId, x, y, target } = useDrag();
  const { workspaces, providers } = useWorkspaces();
  if (!paneId) return null;
  const pane = workspaces.flatMap((w) => w.panes).find((p) => p.id === paneId);

  let zone: { left: number; top: number; width: number; height: number } | null = null;
  if (target) {
    const r = target.rect;
    const halfW = r.width / 2;
    const halfH = r.height / 2;
    zone = {
      left: { left: r.left, top: r.top, width: halfW, height: r.height },
      right: { left: r.left + halfW, top: r.top, width: halfW, height: r.height },
      top: { left: r.left, top: r.top, width: r.width, height: halfH },
      bottom: { left: r.left, top: r.top + halfH, width: r.width, height: halfH },
      center: { left: r.left + 6, top: r.top + 6, width: r.width - 12, height: r.height - 12 },
    }[target.side];
  }

  return (
    <div className="pointer-events-none fixed inset-0 z-[60]">
      {zone && (
        <div
          className="absolute rounded-[var(--radius-pane)] border-2 border-accent bg-accent/15 transition-all duration-75"
          style={zone}
        >
          <span className="absolute top-2 left-2 rounded bg-accent px-1.5 py-0.5 text-[11px] font-medium text-on-accent">
            {target?.side === "center" ? "Swap" : `Place ${target?.side}`}
          </span>
        </div>
      )}
      {pane && (
        <div
          className="absolute flex items-center gap-1.5 rounded-md border border-border-strong bg-panel-raised px-2 py-1 text-xs text-fg shadow-xl"
          style={{ left: x + 12, top: y + 12 }}
        >
          <ProviderIcon provider={pane.provider} size={12} />
          {providerName(providers, pane.provider)}
        </div>
      )}
    </div>
  );
}
