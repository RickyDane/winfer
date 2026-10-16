import { ChevronRight, Gauge } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { cn } from "@/lib/cn";
import { ProviderIcon } from "@/features/providers/ProviderIcon";
import { providerName, useWorkspaces } from "@/stores/workspaces";
import { setUsageTab, type ChartKind, type UsageTab } from "./controls";
import { contextColor, duration, leftNow, levelColor, runsOutAt, tightest, tokens, useUsage, when, windowKey, type UsageWindow } from "./usage";
import { UsageSummary } from "./UsageHistory";

/** Ring that drains as the limit is used up, with the provider's icon inside. */
export function UsageRing({ provider, size = 22, className }: { provider: string; size?: number; className?: string }) {
  const snapshot = useUsage((s) => s.snapshots[provider]);
  const window = tightest(snapshot);
  if (!window) return <ProviderIcon provider={provider} size={Math.round(size * 0.6)} />;
  const left = leftNow(window);
  const stroke = Math.max(2, size / 9);
  const r = (size - stroke) / 2;
  const circumference = 2 * Math.PI * r;
  return (
    <span className={cn("relative inline-flex shrink-0 items-center justify-center", className)} style={{ width: size, height: size }}>
      <svg width={size} height={size} className="absolute inset-0 -rotate-90">
        <circle cx={size / 2} cy={size / 2} r={r} fill="none" stroke="var(--color-border-strong)" strokeWidth={stroke} />
        <circle
          cx={size / 2}
          cy={size / 2}
          r={r}
          fill="none"
          stroke={levelColor(left)}
          strokeWidth={stroke}
          strokeLinecap="round"
          strokeDasharray={circumference}
          strokeDashoffset={circumference * (1 - left / 100)}
          className="transition-[stroke-dashoffset,stroke] duration-700 ease-out"
        />
      </svg>
      <ProviderIcon provider={provider} size={Math.round(size * 0.45)} />
    </span>
  );
}

/** "62%" next to a provider in pickers; dimmed reset hint when it's nearly out. */
export function UsageHint({ provider }: { provider: string }) {
  const snapshot = useUsage((s) => s.snapshots[provider]);
  const window = tightest(snapshot);
  if (!window) return null;
  const left = Math.round(leftNow(window));
  return (
    <span
      className="font-mono text-[10px]"
      style={{ color: left < 30 ? levelColor(left) : "var(--color-fg-subtle)" }}
      title={`${left}% of the ${window.label.toLowerCase()} limit left`}
    >
      {left < 10 && window.resetsAt ? `${left}% · resets ${when(window.resetsAt)}` : `${left}%`}
    </span>
  );
}

function WindowBar({ provider, window }: { provider: string; window: UsageWindow }) {
  const left = leftNow(window);
  const outAt = runsOutAt(provider, window);
  return (
    <div className="space-y-1">
      <div className="flex items-baseline gap-2 text-xs">
        <span className="w-24 shrink-0 text-fg-muted">{window.label}</span>
        <span className="h-1.5 flex-1 overflow-hidden rounded-full bg-hover">
          <span
            className="block h-full rounded-full transition-[width,background-color] duration-700 ease-out"
            style={{ width: `${left}%`, background: levelColor(left) }}
          />
        </span>
        <span className="w-16 shrink-0 text-right font-mono text-[11px] text-fg">{Math.round(left)}% left</span>
      </div>
      <div className="pl-26 text-[11px] text-fg-subtle">
        {window.resetsAt && (window.resetsAt > Date.now() ? `resets ${when(window.resetsAt)}` : "reset — full again")}
        {outAt && <span className="text-warning"> · at this pace runs out {when(outAt)}</span>}
      </div>
    </div>
  );
}

/** How long each kind of window runs, to place its start on the time axis. */
const SPAN: Record<string, number> = { session: 5 * 3600_000, week: 7 * 86400_000, "month-credit": 30 * 86400_000 };

const clock = (t: number, long: boolean) =>
  new Date(t).toLocaleString(undefined, long ? { weekday: "short", day: "numeric", month: "short", hour: "2-digit", minute: "2-digit" } : { hour: "2-digit", minute: "2-digit" });

/** What's left of a window over its run so far, from the readings kept for the run-out projection. */
function WindowLine({ provider, window: w }: { provider: string; window: UsageWindow }) {
  const samples = useUsage((s) => s.samples[windowKey(provider, w)]);
  const [hover, setHover] = useState<number | null>(null);
  const now = Date.now();
  const left = leftNow(w, now);
  const live = w.resetsAt !== null && w.resetsAt > now;
  const outAt = runsOutAt(provider, w);
  const span = SPAN[w.kind];
  // Readings as [at, % left], ending at the current value now.
  const points: [number, number][] = live ? (samples ?? []).filter(([t]) => t <= now).map(([t, used]) => [t, 100 - used]) : [];
  const windowStart = live && span ? w.resetsAt! - span : null;
  const start = Math.min(windowStart ?? Infinity, points[0]?.[0] ?? Infinity, now - 3600_000);
  // A window starts full.
  if (windowStart !== null && (points[0]?.[0] ?? now) > windowStart) points.unshift([windowStart, 100]);
  if (!points.length) points.push([start, left]);
  points.push([now, left]);
  const end = live ? w.resetsAt! : now;
  const x = (t: number) => ((t - start) / (end - start)) * 1000;
  const line = points.map(([t, v]) => `${x(t)},${100 - v}`).join(" ");
  const long = end - start > 86400_000;
  // Value at a time: the last reading at or before it.
  const at = (t: number) => [...points].reverse().find(([pt]) => pt <= t)?.[1] ?? points[0][1];
  const hoverAt = hover !== null ? start + hover * (end - start) : null;
  const color = levelColor(left);

  return (
    <div className="space-y-1">
      <div className="flex items-baseline gap-2 text-xs">
        <span className="flex-1 text-fg-muted">{w.label}</span>
        <span className="font-mono text-[11px] text-fg">{Math.round(left)}% left</span>
      </div>
      <div
        className="relative h-16"
        onMouseMove={(e) => {
          const r = e.currentTarget.getBoundingClientRect();
          const f = (e.clientX - r.left) / r.width;
          // Only the past has readings.
          setHover(Math.max(0, Math.min(f, (now - start) / (end - start))));
        }}
        onMouseLeave={() => setHover(null)}
      >
        {[0, 50, 100].map((t) => (
          <div key={t} className={cn("absolute inset-x-0 border-t", t === 100 ? "border-border-strong" : "border-dashed border-border")} style={{ top: `${t}%` }} />
        ))}
        {live && <div className="absolute inset-y-0 border-l border-dashed border-border-strong" style={{ left: `${x(now) / 10}%` }} title="Now" />}
        <svg className="absolute inset-0 h-full w-full overflow-visible" viewBox="0 0 1000 100" preserveAspectRatio="none">
          <polygon points={`${x(points[0][0])},100 ${line} ${x(now)},100`} fill={color} opacity={0.12} />
          <polyline points={line} fill="none" stroke={color} strokeWidth={1.75} strokeLinejoin="round" strokeLinecap="round" vectorEffect="non-scaling-stroke" />
          {outAt && (
            <line x1={x(now)} y1={100 - left} x2={x(outAt)} y2={100} stroke="var(--color-warning)" strokeWidth={1.5} strokeDasharray="4 3" vectorEffect="non-scaling-stroke" />
          )}
        </svg>
        <span className="absolute size-2 -translate-1/2 rounded-full border-2 border-panel-raised" style={{ left: `${x(now) / 10}%`, top: `${100 - left}%`, background: color }} />
        {hoverAt !== null && (
          <>
            <div className="absolute inset-y-0 border-l border-fg-subtle" style={{ left: `${hover! * 100}%` }} />
            <div
              className="pointer-events-none absolute -top-1 z-10 w-max -translate-y-full rounded-md border border-border-strong bg-panel px-2 py-1 text-[11px] shadow-lg"
              style={hover! < 0.5 ? { left: `${hover! * 100}%` } : { right: `${(1 - hover!) * 100}%` }}
            >
              <span className="text-fg-muted">{clock(hoverAt, long)}</span> <span className="font-mono text-fg">{Math.round(at(hoverAt))}% left</span>
            </div>
          </>
        )}
      </div>
      <div className="flex justify-between font-mono text-[10px] text-fg-subtle">
        <span>{clock(start, long)}</span>
        <span>{live ? clock(end, long) : "now"}</span>
      </div>
      <div className="text-[11px] text-fg-subtle">
        {w.resetsAt && (live ? `resets ${when(w.resetsAt)}` : "reset — full again")}
        {outAt && <span className="text-warning"> · at this pace runs out {when(outAt)}</span>}
      </div>
    </div>
  );
}

/** Providers with known limits, and whether OpenCode's spend is shown; re-renders as countdowns move. */
function useLimited() {
  const snapshots = useUsage((s) => s.snapshots);
  const opencode = useUsage((s) => s.opencode);
  const providers = useWorkspaces((s) => s.providers);
  const [, tick] = useState(0);

  useEffect(() => {
    // Countdowns and resets move on their own.
    const timer = window.setInterval(() => tick((n) => n + 1), 30_000);
    return () => window.clearInterval(timer);
  }, []);

  const installed = (id: string) => providers.some((p) => p.id === id && p.installed);
  const limited = Object.values(snapshots).filter((s) => installed(s.provider) && s.windows.length > 0);
  const showOpencode = installed("opencode") && opencode !== null;
  return { limited, showOpencode };
}

const CARD = "w-[360px] space-y-4 rounded-lg border border-border-strong bg-panel-raised p-4 shadow-xl";

/** Every agent's limits as bars or lines, plus OpenCode's spend today. */
export function Limits({ chart = "bars" }: { chart?: ChartKind }) {
  const snapshots = useUsage((s) => s.snapshots);
  const opencode = useUsage((s) => s.opencode);
  const providers = useWorkspaces((s) => s.providers);
  const { limited, showOpencode } = useLimited();
  const claudeInstalled = providers.some((p) => p.id === "claude" && p.installed);
  const Window = chart === "line" ? WindowLine : WindowBar;
  return (
    <div className="space-y-4">
      {limited.map((s) => (
        <section key={s.provider} className="space-y-2">
          <header className="flex items-center gap-2 text-[13px]">
            <ProviderIcon provider={s.provider} />
            <span className="flex-1 font-medium text-fg">{providerName(providers, s.provider)}</span>
            <span className="text-[10px] text-fg-subtle" title={new Date(s.at).toLocaleString()}>
              {Date.now() - s.at < 120_000 ? "just now" : `${duration(Date.now() - s.at)} ago`}
            </span>
          </header>
          {s.windows.map((w) => (
            <Window key={w.kind} provider={s.provider} window={w} />
          ))}
        </section>
      ))}
      {showOpencode && opencode && (
        <section className="flex items-center gap-2 text-[13px]">
          <ProviderIcon provider="opencode" />
          <span className="flex-1 font-medium text-fg">{providerName(providers, "opencode")}</span>
          <span className="font-mono text-[11px] text-fg-muted">
            today {tokens(opencode.tokens)} tokens · ${opencode.cost.toFixed(2)}
          </span>
        </section>
      )}
      {limited.length === 0 && !showOpencode && <p className="text-[11px] text-fg-subtle">No usage reported yet. Limits show up once an agent has answered a message.</p>}
      {!claudeInstalled || snapshots.claude ? null : (
        <p className="text-[11px] text-fg-subtle">Claude Code reports its limits once a session in winfer has answered a message.</p>
      )}
    </div>
  );
}

const Heading = ({ children }: { children: React.ReactNode }) => <div className="text-[11px] font-medium tracking-wide text-fg-subtle uppercase">{children}</div>;

/** The title bar's popover: every agent's limits. */
export function UsageDetails({ className }: { className?: string }) {
  return (
    <div className={cn(CARD, className)}>
      <Heading>Usage limits</Heading>
      <Limits />
    </div>
  );
}

/** Title bar: one ring per agent with known limits; click for the details. */
export function UsageMeter() {
  const providers = useWorkspaces((s) => s.providers);
  const { limited, showOpencode } = useLimited();
  const [open, setOpen] = useState(false);
  const root = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!open) return;
    const close = (e: MouseEvent) => !root.current?.contains(e.target as Node) && setOpen(false);
    const esc = (e: KeyboardEvent) => e.key === "Escape" && setOpen(false);
    window.addEventListener("mousedown", close);
    window.addEventListener("keydown", esc);
    return () => {
      window.removeEventListener("mousedown", close);
      window.removeEventListener("keydown", esc);
    };
  }, [open]);

  if (limited.length === 0 && !showOpencode) return null;

  const summary = limited
    .map((s) => {
      const w = tightest(s)!;
      return `${providerName(providers, s.provider)}: ${Math.round(leftNow(w))}% of ${w.label.toLowerCase()} left`;
    })
    .join("\n");

  return (
    <div ref={root} className="relative">
      <button
        onClick={() => setOpen((o) => !o)}
        title={summary || "Usage"}
        className={cn("flex items-center gap-1 rounded-md px-1 py-0.5 hover:bg-hover", open && "bg-hover")}
      >
        {limited.length > 0 ? limited.map((s) => <UsageRing key={s.provider} provider={s.provider} />) : <Gauge size={14} />}
      </button>
      {open && <UsageDetails className="anim-menu absolute right-0 z-50 mt-1" />}
    </div>
  );
}

/** A part of the sidebar preview; its heading opens that tab of the Usage page. */
function PreviewSection({ title, onOpen, className, children }: { title: string; onOpen: () => void; className?: string; children: React.ReactNode }) {
  return (
    <section className={cn("space-y-3", className)}>
      <button onClick={onOpen} className="group flex w-full items-center gap-1 text-left" title={`Open ${title.toLowerCase()} in settings`}>
        <Heading>{title}</Heading>
        <ChevronRight size={11} className="text-fg-subtle opacity-0 transition-opacity group-hover:opacity-100" />
      </button>
      {children}
    </section>
  );
}

/** Sidebar footer: opens the Usage page in settings; hovering previews the usage and the limits. */
export function UsageButton({ onOpen, title }: { onOpen: () => void; title: string }) {
  // Anchor point (button's top-left) while the preview is shown.
  const [hover, setHover] = useState<{ left: number; bottom: number } | null>(null);
  const timer = useRef<number | undefined>(undefined);
  const anchor = useRef<HTMLDivElement>(null);
  useEffect(() => () => window.clearTimeout(timer.current), []);
  const enter = () => {
    window.clearTimeout(timer.current);
    timer.current = window.setTimeout(() => {
      const r = anchor.current?.getBoundingClientRect();
      if (r) setHover({ left: r.left, bottom: window.innerHeight - r.top });
    }, 250);
  };
  const leave = () => {
    window.clearTimeout(timer.current);
    setHover(null);
  };
  const openTab = (tab: UsageTab) => {
    setUsageTab(tab);
    leave();
    onOpen();
  };
  return (
    <div ref={anchor} className="relative" onMouseEnter={enter} onMouseLeave={leave}>
      <button
        onClick={() => {
          leave();
          onOpen();
        }}
        aria-label={title}
        className="rounded p-1 text-fg-subtle hover:bg-hover hover:text-fg"
      >
        <Gauge size={14} />
      </button>
      {hover &&
        // Portaled so the sidebar can't clip it or the main area paint over it; still a React child,
        // so hovering it keeps the preview open. pb bridges the gap to the button.
        createPortal(
          <div className="fixed z-[1000] pb-1" style={{ left: hover.left, bottom: hover.bottom }}>
            <div className={cn(CARD, "anim-menu")}>
              <PreviewSection title="Usage" onOpen={() => openTab("usage")}>
                <UsageSummary />
              </PreviewSection>
              <PreviewSection title="Limits" onOpen={() => openTab("limits")} className="border-t border-border pt-3">
                <Limits />
              </PreviewSection>
            </div>
          </div>,
          document.body,
        )}
    </div>
  );
}

/** Pane header: how full the agent's context window is, as a short bar and a percentage. */
export function ContextMeter({ paneId }: { paneId: string }) {
  const context = useUsage((s) => s.context[paneId]);
  if (!context) return null;
  const used = Math.min(100, Math.max(0, context.usedPercent));
  return (
    <span
      className="flex shrink-0 items-center gap-1"
      title={`Context: ${tokens(context.tokens)} of ${tokens(context.window)} tokens (${Math.round(used)}% used)`}
    >
      <span className="h-1 w-8 overflow-hidden rounded-full bg-hover">
        <span
          className="block h-full rounded-full transition-[width,background-color] duration-700 ease-out"
          style={{ width: `${Math.max(used, 3)}%`, background: contextColor(used) }}
        />
      </span>
      <span className="hidden font-mono text-[10px] @[15rem]:inline" style={{ color: used >= 60 ? contextColor(used) : "var(--color-fg-subtle)" }}>
        {Math.round(used)}%
      </span>
    </span>
  );
}
