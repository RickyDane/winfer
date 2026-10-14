import { Gauge } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import { cn } from "@/lib/cn";
import { ProviderIcon } from "@/features/providers/ProviderIcon";
import { providerName, useWorkspaces } from "@/stores/workspaces";
import { duration, leftNow, levelColor, runsOutAt, tightest, useUsage, when, type UsageWindow } from "./usage";

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

function compact(n: number) {
  return n >= 1_000_000 ? `${(n / 1_000_000).toFixed(1)}M` : n >= 1000 ? `${Math.round(n / 1000)}k` : String(n);
}

/** Title bar: one ring per agent with known limits; click for the details. */
export function UsageMeter() {
  const snapshots = useUsage((s) => s.snapshots);
  const opencode = useUsage((s) => s.opencode);
  const providers = useWorkspaces((s) => s.providers);
  const [open, setOpen] = useState(false);
  const root = useRef<HTMLDivElement>(null);
  const [, tick] = useState(0);

  useEffect(() => {
    // Countdowns and resets move on their own.
    const timer = window.setInterval(() => tick((n) => n + 1), 30_000);
    return () => window.clearInterval(timer);
  }, []);

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

  const installed = (id: string) => providers.some((p) => p.id === id && p.installed);
  const limited = Object.values(snapshots).filter((s) => installed(s.provider) && s.windows.length > 0);
  const showOpencode = installed("opencode") && opencode !== null;
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
      {open && (
        <div className="anim-menu absolute right-0 z-50 mt-1 w-[360px] space-y-4 rounded-lg border border-border-strong bg-panel-raised p-4 shadow-xl">
          <div className="text-[11px] font-medium tracking-wide text-fg-subtle uppercase">Usage limits</div>
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
                <WindowBar key={w.kind} provider={s.provider} window={w} />
              ))}
            </section>
          ))}
          {showOpencode && (
            <section className="flex items-center gap-2 text-[13px]">
              <ProviderIcon provider="opencode" />
              <span className="flex-1 font-medium text-fg">{providerName(providers, "opencode")}</span>
              <span className="font-mono text-[11px] text-fg-muted">
                today {compact(opencode.tokens)} tokens · ${opencode.cost.toFixed(2)}
              </span>
            </section>
          )}
          {!installed("claude") || snapshots.claude ? null : (
            <p className="text-[11px] text-fg-subtle">Claude Code reports its limits once a session in winfer has answered a message.</p>
          )}
        </div>
      )}
    </div>
  );
}
