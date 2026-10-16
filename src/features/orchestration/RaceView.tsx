import { recordMerge, snapshotForHistory } from "@/features/history/history";
import { ask, message } from "@tauri-apps/plugin-dialog";
import { BarChart3, Flag, FlaskConical, GitBranch, Minus, Plus, RefreshCw, Trophy, X } from "lucide-react";
import { useCallback, useEffect, useState } from "react";
import { execute } from "@/commands/bus";
import { ProviderIcon } from "@/features/providers/ProviderIcon";
import { StatusDot } from "@/features/terminal/StatusDot";
import { closeTerminal } from "@/features/terminal/xtermRegistry";
import { commitWorktree, mergeWorktree, removeWorktree, worktreeDiff } from "@/features/worktree/worktree";
import { providerName, useActiveWorkspace, useWorkspaces, type Pane } from "@/stores/workspaces";
import { logActivity } from "@/stores/activity";
import { cn } from "@/lib/cn";
import { openRaceResults } from "./resultsStore";
import { isFinished } from "./runTimes";

interface Stats {
  files: number;
  adds: number;
  dels: number;
  untracked: number;
  ahead: number;
  dirty: boolean;
}

function diffStats(diff: string): Pick<Stats, "files" | "adds" | "dels"> {
  let files = 0, adds = 0, dels = 0;
  for (const line of diff.split("\n")) {
    if (line.startsWith("diff --git ")) files++;
    else if (line.startsWith("+") && !line.startsWith("+++")) adds++;
    else if (line.startsWith("-") && !line.startsWith("---")) dels++;
  }
  return { files, adds, dels };
}

function StartForm({ ab, onStarted }: { ab: boolean; onStarted: () => void }) {
  const providers = useWorkspaces((s) => s.providers).filter((p) => p.installed && p.id !== "shell");
  const [prompt, setPrompt] = useState("");
  const [abProvider, setAbProvider] = useState(() => providers[0]?.id ?? "");
  const [counts, setCounts] = useState<Record<string, number>>(() => Object.fromEntries(providers.slice(0, 2).map((p) => [p.id, 1])));
  const total = Object.values(counts).reduce((a, b) => a + b, 0);

  const bump = (id: string, d: number) => setCounts((c) => ({ ...c, [id]: Math.min(3, Math.max(0, (c[id] ?? 0) + d)) }));

  return (
    <section className="space-y-3">
      <textarea
        autoFocus
        value={prompt}
        onChange={(e) => setPrompt(e.target.value)}
        rows={3}
        placeholder="Task for every agent, e.g. “Make the cancelled-tier guard return 403 and add a regression test”"
        className="w-full resize-none rounded-md border border-border bg-panel p-2.5 font-mono text-xs text-fg outline-none focus:border-accent"
      />
      {ab ? (
        <div className="flex flex-wrap items-center gap-2">
          {providers.map((p) => (
            <button
              key={p.id}
              onClick={() => setAbProvider(p.id)}
              className={cn(
                "flex items-center gap-1.5 rounded-md border px-2 py-1 text-xs",
                abProvider === p.id ? "border-accent bg-accent-soft text-accent" : "border-border text-fg hover:bg-hover",
              )}
            >
              <ProviderIcon provider={p.id} size={12} />
              {p.name}
            </button>
          ))}
          <button
            disabled={!prompt.trim() || !abProvider}
            onClick={() => {
              execute({ type: "abtest.start", provider: abProvider, prompt: prompt.trim() });
              onStarted();
            }}
            className="ml-auto flex items-center gap-1.5 rounded-md bg-accent px-2.5 py-1 text-xs font-medium text-on-accent hover:opacity-90 disabled:opacity-40"
          >
            <FlaskConical size={12} /> Start A/B test
          </button>
        </div>
      ) : (
      <div className="flex flex-wrap items-center gap-2">
        {providers.map((p) => (
          <div key={p.id} className="flex items-center gap-1.5 rounded-md border border-border px-2 py-1 text-xs text-fg">
            <ProviderIcon provider={p.id} size={12} />
            <span>{p.name}</span>
            <button onClick={() => bump(p.id, -1)} className="rounded p-0.5 text-fg-subtle hover:bg-hover hover:text-fg">
              <Minus size={11} />
            </button>
            <span className="w-3 text-center font-mono">{counts[p.id] ?? 0}</span>
            <button onClick={() => bump(p.id, 1)} className="rounded p-0.5 text-fg-subtle hover:bg-hover hover:text-fg">
              <Plus size={11} />
            </button>
          </div>
        ))}
        <button
          disabled={!prompt.trim() || total < 2}
          onClick={() => {
            execute({ type: "race.start", counts, prompt: prompt.trim() });
            onStarted();
          }}
          className="ml-auto flex items-center gap-1.5 rounded-md bg-accent px-2.5 py-1 text-xs font-medium text-on-accent hover:opacity-90 disabled:opacity-40"
          title={total < 2 ? "Pick at least two agents" : undefined}
        >
          <Flag size={12} /> Start race ({total})
        </button>
      </div>
      )}
      <p className="text-[11px] text-fg-subtle">
        {ab
          ? "Starts the agent twice on the same task, each in its own git worktree: A as usual, with winfer's tools, the browser and your MCP servers and skills; B plain, without them. Compare time, tokens, cost and code written once both finish."
          : "Each agent gets its own git worktree and branch, so they can't overwrite each other."}
      </p>
    </section>
  );
}

/** Merges the winner of a race and closes the other racers, deleting their worktrees. */
export async function pickRaceWinner(winner: Pane, panes: Pane[]): Promise<boolean> {
  const losers = panes.filter((p) => p.id !== winner.id);
  if (!winner.worktree) return false;
  const ok = await ask(
    `Merge ${winner.worktree.branch} into ${winner.worktree.base}, then close the other ${losers.length} agent${losers.length === 1 ? "" : "s"} and delete their worktrees?`,
    { title: "Pick winner", kind: "warning", okLabel: "Merge and clean up" },
  );
  if (!ok) return false;
  const snapshot = await snapshotForHistory(winner as Pane & { worktree: NonNullable<Pane["worktree"]> });
  try {
    // Agents often stop without committing; their work would otherwise be left behind.
    await commitWorktree(winner.worktree, `Changes by ${providerName(useWorkspaces.getState().providers, winner.provider)}`);
    await mergeWorktree(winner.worktree);
  } catch (err) {
    await message(String(err), { title: "Merge not possible", kind: "error" });
    return false;
  }
  void recordMerge(snapshot);
  const s = useWorkspaces.getState();
  for (const loser of losers) {
    closeTerminal(loser.id);
    s.removePane(loser.id);
    if (loser.worktree) await removeWorktree(loser.worktree, true).catch(() => {});
  }
  s.updatePane(winner.id, { raceId: null });
  logActivity(winner.workspaceId, `Race won by ${providerName(s.providers, winner.provider)} (${winner.worktree.branch})`, winner.provider);
  return true;
}

function RaceBoard({ raceId, panes }: { raceId: string; panes: Pane[] }) {
  const providers = useWorkspaces((s) => s.providers);
  const [stats, setStats] = useState<Record<string, Stats | string>>({});

  const load = useCallback(() => {
    for (const pane of panes) {
      if (!pane.worktree) continue;
      worktreeDiff(pane.worktree).then(
        (d) => setStats((s) => ({ ...s, [pane.id]: { ...diffStats(d.diff), untracked: d.untracked.length, ahead: d.ahead, dirty: d.dirty } })),
        (e) => setStats((s) => ({ ...s, [pane.id]: String(e) })),
      );
    }
  }, [panes]);

  // Worktrees are created as each agent starts, so reload when their paths appear.
  const worktrees = panes.map((p) => p.worktree?.path ?? "").join("|");
  useEffect(load, [raceId, worktrees]); // eslint-disable-line react-hooks/exhaustive-deps

  const isolated = panes.every((p) => p.worktree);
  const ab = raceId.startsWith("ab-");
  const finished = panes.every(isFinished);
  return (
    <section className="space-y-2">
      <div className="flex items-center gap-2 text-xs text-fg-muted">
        {ab ? <FlaskConical size={12} className="text-accent" /> : <Flag size={12} className="text-accent" />}
        <span className="flex-1 truncate">{panes[0]?.prompt ?? (ab ? "A/B test" : "Race")}</span>
        <button
          onClick={() => openRaceResults(raceId)}
          className={cn(
            "flex items-center gap-1 rounded-md px-2 py-0.5",
            finished ? "bg-accent text-on-accent hover:opacity-90" : "border border-border text-fg-subtle hover:bg-hover hover:text-fg",
          )}
          title={finished ? "Time, tokens, cost and code written, side by side" : "The agents are still working; the numbers so far"}
        >
          <BarChart3 size={11} /> Results
        </button>
        <button onClick={load} className="rounded p-1 text-fg-subtle hover:bg-hover hover:text-fg" title="Refresh changes">
          <RefreshCw size={12} />
        </button>
      </div>
      {!isolated && <p className="text-[11px] text-warning">Some agents have no worktree, so they weren't started: races and A/B tests need the folder to be a git repository.</p>}
      <div className="divide-y divide-border rounded-lg border border-border">
        {panes.map((pane) => {
          const st = stats[pane.id];
          return (
            <div key={pane.id} className="flex items-center gap-3 px-3 py-2 text-xs">
              <StatusDot status={pane.status} />
              <ProviderIcon provider={pane.provider} size={13} />
              <span className="w-28 truncate text-fg">{providerName(providers, pane.provider)}</span>
              {ab && (
                <span className={cn("w-16 shrink-0 rounded px-1 text-center text-[10px]", pane.plain ? "bg-hover text-fg-muted" : "bg-accent-soft text-accent")}>
                  {pane.plain ? "plain" : "with tools"}
                </span>
              )}
              <span className="flex w-32 items-center gap-1 truncate font-mono text-[11px] text-fg-subtle">
                <GitBranch size={11} /> {pane.worktree?.branch.replace(/^winfer\//, "") ?? "—"}
              </span>
              <span className="flex-1 font-mono text-[11px]">
                {typeof st === "string" ? (
                  <span className="text-danger">{st}</span>
                ) : st ? (
                  <>
                    <span className="text-fg-muted">{st.files + st.untracked} files </span>
                    <span className="text-success">+{st.adds} </span>
                    <span className="text-danger">−{st.dels}</span>
                    <span className="text-fg-subtle">
                      {" "}· {st.ahead} commit{st.ahead === 1 ? "" : "s"}
                      {st.dirty && " · uncommitted"}
                    </span>
                  </>
                ) : (
                  <span className="text-fg-subtle">…</span>
                )}
              </span>
              <button onClick={() => execute({ type: "pane.focus", target: { id: pane.id } })} className="rounded px-1.5 py-0.5 text-fg-subtle hover:bg-hover hover:text-fg">
                Focus
              </button>
              <button
                disabled={!pane.worktree}
                onClick={() => execute({ type: "pane.showChanges", target: { id: pane.id } })}
                className="rounded px-1.5 py-0.5 text-fg-subtle hover:bg-hover hover:text-fg disabled:opacity-40"
              >
                Diff
              </button>
              <button
                disabled={!pane.worktree}
                onClick={() => void pickRaceWinner(pane, panes)}
                className="flex items-center gap-1 rounded-md bg-accent-soft px-2 py-0.5 text-accent hover:bg-accent hover:text-on-accent disabled:opacity-40"
              >
                <Trophy size={11} /> Pick
              </button>
            </div>
          );
        })}
      </div>
    </section>
  );
}

/** ⌘⇧R: start a race (or an A/B test) and compare the competing agents' changes. */
export function RaceView() {
  const { overlay, setOverlay } = useWorkspaces();
  const workspace = useActiveWorkspace();
  const [showForm, setShowForm] = useState(false);

  if ((overlay !== "race" && overlay !== "abtest") || !workspace) return null;
  const ab = overlay === "abtest";
  const races = new Map<string, Pane[]>();
  for (const pane of workspace.panes) {
    if (pane.raceId) races.set(pane.raceId, [...(races.get(pane.raceId) ?? []), pane]);
  }

  return (
    <div className="anim-backdrop fixed inset-0 z-40 flex justify-center bg-black/40 pt-[10vh]" onMouseDown={() => setOverlay(null)}>
      <div className="anim-dialog h-fit max-h-[80vh] w-[760px] space-y-5 overflow-y-auto rounded-xl border border-border-strong bg-panel-raised p-5 shadow-2xl" onMouseDown={(e) => e.stopPropagation()}>
        <header className="flex items-center gap-2">
          {ab ? <FlaskConical size={14} className="text-accent" /> : <Flag size={14} className="text-accent" />}
          <h1 className="text-sm font-semibold text-fg">{ab ? "A/B test" : "Race mode"}</h1>
          <div className="ml-2 flex flex-1 gap-0.5">
            {(["race", "abtest"] as const).map((o) => (
              <button
                key={o}
                onClick={() => setOverlay(o)}
                className={cn("rounded px-1.5 py-0.5 text-[11px]", overlay === o ? "bg-hover text-fg" : "text-fg-subtle hover:text-fg")}
              >
                {o === "race" ? "Race" : "A/B test"}
              </button>
            ))}
          </div>
          {races.size > 0 && !showForm && (
            <button onClick={() => setShowForm(true)} className="rounded-md border border-border-strong px-2 py-1 text-xs text-fg hover:bg-hover">
              {ab ? "New A/B test" : "New race"}
            </button>
          )}
          <button onClick={() => setOverlay(null)} className="rounded p-1 text-fg-subtle hover:bg-hover hover:text-fg">
            <X size={14} />
          </button>
        </header>
        {(races.size === 0 || showForm) && <StartForm ab={ab} onStarted={() => setShowForm(false)} />}
        {[...races].map(([raceId, panes]) => (
          <RaceBoard key={raceId} raceId={raceId} panes={panes} />
        ))}
      </div>
    </div>
  );
}
