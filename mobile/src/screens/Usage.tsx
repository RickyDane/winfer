import { useEffect, useState } from "react";
import type { DesktopClient } from "../client";
import { go, haptic, useTick } from "../hooks";
import type { UsageProvider, UsageWindow } from "../protocol";
import { Icon, ProviderMark } from "./common";

/** Meter color: green while plenty is left, through amber to red (the desktop's levels). */
const level = (left: number) => (left <= 10 ? "var(--danger)" : left < 30 ? "var(--warn)" : left < 60 ? "color-mix(in srgb, var(--warn) 50%, var(--ok))" : "var(--ok)");

function dur(ms: number): string {
  const m = Math.max(1, Math.round(ms / 60_000));
  const [d, h, mm] = [Math.floor(m / 1440), Math.floor((m % 1440) / 60), m % 60];
  if (d) return h ? `${d}d ${h}h` : `${d}d`;
  if (h) return mm ? `${h}h ${mm}m` : `${h}h`;
  return `${mm}m`;
}

const when = (ms: number) => new Date(ms).toLocaleString([], { weekday: "short", hour: "2-digit", minute: "2-digit" });

function age(at: number) {
  const a = Date.now() - at;
  return { stale: a > 600_000, text: `${a > 600_000 ? "Stale · " : ""}${a < 120_000 ? "just now" : `${dur(a)} ago`}` };
}

function Meter({ left }: { left: number }) {
  return (
    <div className="usage-meter" role="meter" aria-valuenow={Math.round(left)} aria-valuemin={0} aria-valuemax={100} style={{ ["--lvl" as string]: level(left) }}>
      <i style={{ width: `${Math.max(left, left > 0 ? 2 : 0)}%` }} />
    </div>
  );
}

function Window({ w }: { w: UsageWindow }) {
  const now = Date.now();
  const reset = w.resetsAt && w.resetsAt > now ? w.resetsAt : null;
  return (
    <div>
      <div className="usage-win-top">
        <b>{w.label}</b>
        <span className="pct" style={{ color: level(w.left) }}>{Math.round(w.left)}% left</span>
      </div>
      <Meter left={w.left} />
      <div className="usage-meta">
        {reset && <span>Resets {when(reset)} · in {dur(reset - now)}</span>}
        {w.runsOutAt && <span className="warn">At this pace, runs out {when(w.runsOutAt)}</span>}
      </div>
    </div>
  );
}

function Provider({ p }: { p: UsageProvider }) {
  const multi = p.accounts.length > 1;
  const main = multi && p.pooled ? p.pooled : p.accounts[0];
  const a = age(main.at);
  return (
    <section>
      <div className="usage-head">
        <ProviderMark provider={p.id} size={22} />
        <span>{p.name}</span>
        {multi ? <span className="usage-badge">{p.accounts.length} accounts</span> : p.accounts[0].key.includes(":") && <span className="age">· {p.accounts[0].label}</span>}
        <span className="grow" />
        <span className="age" data-stale={a.stale}>{a.text}</span>
      </div>
      <div className="usage-card">
        <div className="usage-wins">
          {main.windows.map((w) => (
            <Window key={w.kind + w.label} w={w} />
          ))}
        </div>
        {multi && (
          <div className="usage-accounts">
            {p.accounts.map((acc) => {
              const aa = age(acc.at);
              return (
                <div className="usage-acct" key={acc.key}>
                  <div className="usage-acct-head">
                    {acc.label}
                    <span data-stale={aa.stale}>{aa.text}</span>
                  </div>
                  <div className="usage-mini">
                    {acc.windows.map((w) => (
                      <MiniRow key={w.kind + w.label} w={w} />
                    ))}
                  </div>
                </div>
              );
            })}
          </div>
        )}
      </div>
    </section>
  );
}

function MiniRow({ w }: { w: UsageWindow }) {
  return (
    <>
      <span>{w.label}</span>
      <span className="pct">{Math.round(w.left)}%</span>
      <Meter left={w.left} />
    </>
  );
}

export function Usage({ client }: { client: DesktopClient }) {
  const [providers, setProviders] = useState<UsageProvider[] | null>(null);
  const [err, setErr] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  useTick(30_000);

  async function load() {
    setBusy(true);
    try {
      const r = await client.request({ t: "usage.get" });
      if (r.t === "usage") setProviders(r.providers);
      else if (r.t === "error") throw new Error(r.error);
      setErr(null);
    } catch (e) {
      setErr((e as Error).message);
    } finally {
      setBusy(false);
    }
  }

  useEffect(() => {
    void load();
    const t = setInterval(() => void load(), 60_000);
    return () => clearInterval(t);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [client]);

  return (
    <div className="screen">
      <header className="bar">
        <button className="icon-btn" aria-label="Back" onClick={() => go("/settings")}>
          <Icon name="back" size={19} />
        </button>
        <div className="bar-title">
          <b>Usage &amp; limits</b>
          <span>{busy ? "Reading…" : "Plan limits per agent"}</span>
        </div>
        <button
          className="icon-btn"
          aria-label="Refresh"
          onClick={() => {
            haptic(6);
            void load();
          }}
        >
          <Icon name="refresh" size={18} />
        </button>
      </header>
      <div className="scroll">
        <main className="usage">
          <p className="usage-intro">What's left of each plan's limits, like the rings on your desktop. Dashed projections become a warning when the current pace would run out before the reset.</p>
          {err && <div className="inline-error">{err}</div>}
          {providers?.map((p) => <Provider key={p.id} p={p} />)}
          {providers && !providers.length && <div className="usage-empty">No usage reported yet. Open winfer on your desktop and refresh to check your signed-in accounts.</div>}
          {!providers && !err && <div className="usage-empty">Loading…</div>}
        </main>
      </div>
    </div>
  );
}
