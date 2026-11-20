import { CornerDownLeft, GitBranch, Search, Sparkles, Zap } from "lucide-react";
import { useState } from "react";
import { execute } from "@/commands/bus";
import { keysFor } from "@/commands/keymap";
import { ProviderIcon } from "@/features/providers/ProviderIcon";
import { defaultProviderId, providerName, useActiveWorkspace, useWorkspaces } from "@/stores/workspaces";

/**
 * The empty workspace (Graphite "Welcome"): a task box that starts the default agent, the
 * installed agents to pick from, and a few next steps. Port of the native `welcome.rs`.
 */
export function Welcome() {
  const workspace = useActiveWorkspace();
  const providers = useWorkspaces((s) => s.providers);
  const defaultId = useWorkspaces((s) => defaultProviderId(s));
  const [task, setTask] = useState("");
  const agents = providers.filter((p) => p.id !== "shell" && p.installed);
  const missing = providers.filter((p) => p.id !== "shell" && !p.installed).length;

  const start = () => {
    const text = task.trim();
    if (!text) return execute({ type: "ui.overlay", overlay: "newPane" });
    setTask("");
    execute({ type: "pane.spawn", provider: "default", prompt: text });
  };

  const steps = [
    { icon: GitBranch, title: "Open a branch", text: "Start an agent on an existing branch or worktree.", keys: keysFor({ type: "ui.overlay", overlay: "branch" }), run: () => execute({ type: "ui.overlay", overlay: "branch" }) },
    { icon: Search, title: "Commands", text: "Everything winfer can do, by name.", keys: keysFor({ type: "ui.palette" }), run: () => execute({ type: "ui.palette" }) },
    { icon: Zap, title: "Shortcuts", text: "See and change every shortcut.", keys: keysFor({ type: "ui.cheatSheet" }), run: () => execute({ type: "ui.cheatSheet" }) },
  ];

  return (
    <div className="flex h-full w-full items-center justify-center rounded-[var(--radius-pane)] border border-border bg-panel">
      <div className="flex w-[620px] max-w-[92%] flex-col items-center gap-5">
        <span className="grid size-11 place-items-center rounded-xl bg-accent-soft text-accent">
          <Sparkles size={20} />
        </span>
        <div className="text-center">
          <h2 className="text-xl font-semibold text-fg">What should agents do in {workspace?.name ?? "this workspace"}?</h2>
          <p className="mt-1 text-[13px] text-fg-muted">One line starts an agent in its own worktree. Or pick an agent below.</p>
        </div>
        <label className="flex h-[46px] w-full items-center gap-2 rounded-xl border border-border-strong bg-panel-raised pr-1.5 pl-3.5 focus-within:border-accent focus-within:ring-3 focus-within:ring-accent-soft">
          <ProviderIcon provider={defaultId} size={16} />
          <input
            value={task}
            onChange={(e) => setTask(e.target.value)}
            onKeyDown={(e) => e.key === "Enter" && start()}
            placeholder="Describe a task — Enter starts an agent on it"
            aria-label="Task for a new agent"
            className="min-w-0 flex-1 bg-transparent text-sm text-fg outline-none placeholder:text-fg-subtle"
          />
          <button onClick={start} className="flex h-8 shrink-0 items-center gap-1.5 rounded-lg bg-accent px-3 text-[12.5px] font-medium text-on-accent hover:opacity-90">
            Start {providerName(providers, defaultId)}
            <CornerDownLeft size={13} />
          </button>
        </label>
        <div className="flex flex-wrap justify-center gap-2">
          {agents.map((p) => (
            <button
              key={p.id}
              onClick={() => execute({ type: "ui.overlay", overlay: "newPane", provider: p.id })}
              className="flex h-8 items-center gap-2 rounded-lg border border-border bg-panel px-3 text-[12.5px] text-fg hover:border-border-strong hover:bg-hover"
            >
              <ProviderIcon provider={p.id} size={14} />
              {p.name}
            </button>
          ))}
          {missing > 0 && (
            <button onClick={() => execute({ type: "ui.settings", page: "agents" })} className="h-8 px-2 text-xs text-fg-subtle hover:text-fg">
              + {missing} more agents you can install
            </button>
          )}
        </div>
        <div className="flex w-full gap-2.5">
          {steps.map((s) => (
            <button key={s.title} onClick={s.run} className="flex min-w-0 flex-1 flex-col gap-1 rounded-lg border border-border bg-panel p-3 text-left hover:border-border-strong hover:bg-hover">
              <span className="flex items-center gap-2 text-[12.5px] text-fg">
                <s.icon size={14} className="text-accent" />
                <span className="flex-1">{s.title}</span>
                {s.keys && <kbd className="rounded border border-border px-1 font-mono text-[10px] text-fg-subtle">{s.keys}</kbd>}
              </span>
              <span className="text-[11.5px] text-fg-subtle">{s.text}</span>
            </button>
          ))}
        </div>
      </div>
    </div>
  );
}
