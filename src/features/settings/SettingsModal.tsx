import { invoke } from "@tauri-apps/api/core";
import { usePlanSidebar } from "@/features/code/planSidebar";
import { ArchiveRestore, ArrowUpCircle, Bot, Users, ChartColumn, Check, ChevronDown, CloudOff, Copy, Download, FolderOpen, Keyboard, Mic, PackageCheck, Palette, Plus, Puzzle, RefreshCw, Search, SlidersHorizontal, Smartphone, Sparkles, SquareTerminal, Store, Trash2, TriangleAlert, Wand2, X, type LucideIcon } from "lucide-react";
import { Spinner } from "@/components/Spinner";
import { useEffect, useMemo, useState } from "react";
import { OPTIONAL_TOOLS, DEFAULT_SYSTEM_PROMPT, DEFAULT_TERMINAL_FONT_SIZE, defaultProviderId, suspendSaving, MAX_TERMINAL_FONT_SIZE, MIN_TERMINAL_FONT_SIZE, useWorkspaces, useWorkspacesPick } from "@/stores/workspaces";
import { IS_MAC, keysFor } from "@/commands/keymap";
import { CHATGPT_VOICES, DEFAULT_CHATGPT_VOICE, DEFAULT_LIVE_MODEL, DEFAULT_LIVE_VOICE, LIVE_VOICES } from "@/features/voice/live/session";
import { LIVE_ROLE } from "@/features/voice/live/instructions";
import { timed, trace } from "@/features/voice/live/trace";
import { availableLlms, LLM_PROVIDERS, llmProvider, type LlmProvider } from "@/features/llm/llm";
import { ask, open as openDialog, save as saveDialog } from "@tauri-apps/plugin-dialog";
import { relaunch } from "@tauri-apps/plugin-process";
import { exportData, inspectExport, stageImport, type ExportManifest } from "@/features/settings/transfer";
import { revealItemInDir } from "@tauri-apps/plugin-opener";
import { fetchCatalog, installPlugin, loadPlugins, matchesSearch, pluginsDir, setPluginEnabled, uninstallPlugin, usePlugins, type StoreEntry, type PluginInfo } from "@/features/plugins/plugins";
import { toast } from "@/stores/toasts";
import { cancelChatGptSignIn, signInWithChatGpt, signOutOfChatGpt, useChatGpt } from "@/features/llm/chatgpt";
import { checkForUpdates, installUpdate, setUpdateChannel, showReleaseNotes, type UpdateChannel, useUpdates } from "@/features/updates/updater";
import { UsageSettings } from "@/features/usage/UsagePage";
import { AccountsSettings } from "@/features/accounts/AccountsSettings";
import { checkProviderUpdates, installLabel, updateProvider, useProviderUpdates } from "@/features/providers/updates";
import { THEMES } from "@/features/theme/themes";
import { loadEditors, preferredEditor, useEditors } from "@/features/editors/editors";
import { cn } from "@/lib/cn";
import { MAX_TEXT_SCALE, MIN_TEXT_SCALE, setTextScale, TEXT_SCALE_STEP, useCodeWeb } from "@/features/code/styles";
import { mobileStatus, onMobileChange, revokeDevice, setRelayUrl, startPairing, type MobileStatus, type Pairing } from "@/features/mobile/mobile";
import {
  copyFile,
  copyLabel,
  copySkill,
  customSources,
  FEATURED_SOURCES,
  fetchInstalledSkills,
  fetchRepoSkills,
  fetchTargets,
  homeSkillCopies,
  installSkill,
  isOutdated,
  normalizeRepo,
  saveCustomSources,
  savedTargets,
  saveTargets,
  skillOrigin,
  uninstallSkill,
  type InstalledSkill,
  type RepoListing,
  type RepoSkill,
  type SkillTarget,
} from "@/features/skills/skills";

/* Building blocks: a titled card of rows, each row a label with its hint on the left and the
   control on the right, like macOS System Settings. */

const Card = ({ title, description, children }: { title: string; description?: React.ReactNode; children: React.ReactNode }) => (
  <section className="space-y-2">
    <div className="px-1">
      <h3 className="text-[13px] font-medium text-fg">{title}</h3>
      {description && <p className="mt-0.5 text-xs leading-relaxed text-fg-muted">{description}</p>}
    </div>
    <div className="divide-y divide-border overflow-hidden rounded-lg border border-border bg-panel">{children}</div>
  </section>
);

const Row = ({ label, hint, children, stacked }: { label: React.ReactNode; hint?: React.ReactNode; children?: React.ReactNode; stacked?: boolean }) => (
  <div className={cn("px-3.5 py-3", stacked ? "space-y-2.5" : "flex items-center gap-4")}>
    <div className="min-w-0 flex-1">
      <div className="text-[13px] text-fg">{label}</div>
      {hint && <div className="mt-0.5 text-xs leading-relaxed text-fg-subtle">{hint}</div>}
    </div>
    {children && <div className={cn("flex items-center gap-2", !stacked && "shrink-0")}>{children}</div>}
  </div>
);

/** Feedback under a card: green when it worked, red when it didn't. */
const Result = ({ result }: { result: { ok: boolean; text: string } | null }) =>
  result && <p className={cn("anim-fade px-1 text-xs", result.ok ? "text-success" : "text-danger")}>{result.text}</p>;

function Toggle({ checked, onChange, label }: { checked: boolean; onChange: (checked: boolean) => void; label: string }) {
  return (
    <button
      role="switch"
      aria-checked={checked}
      aria-label={label}
      onClick={() => onChange(!checked)}
      className={cn(
        "relative h-5 w-9 shrink-0 rounded-full transition-colors focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-accent",
        checked ? "bg-accent" : "bg-active ring-1 ring-border-strong ring-inset",
      )}
    >
      <span className={cn("absolute top-0.5 left-0.5 size-4 rounded-full bg-white shadow transition-transform duration-150", checked && "translate-x-4")} />
    </button>
  );
}

function Select({ className, children, ...props }: React.SelectHTMLAttributes<HTMLSelectElement>) {
  return (
    <span className={cn("relative inline-flex", className)}>
      <select
        {...props}
        className="h-7 w-full min-w-0 appearance-none truncate rounded-md border border-border-strong bg-panel-raised pr-7 pl-2.5 text-xs text-fg outline-none hover:bg-hover focus:border-accent disabled:opacity-50"
      >
        {children}
      </select>
      <ChevronDown size={12} className="pointer-events-none absolute top-1/2 right-2 -translate-y-1/2 text-fg-subtle" />
    </span>
  );
}

const buttonClass =
  "flex h-7 shrink-0 items-center gap-1.5 rounded-md border border-border-strong bg-panel-raised px-2.5 text-xs text-fg hover:bg-hover disabled:opacity-50";
const inputClass = "h-7 rounded-md border border-border-strong bg-panel-raised px-2.5 font-mono text-xs text-fg outline-none focus:border-accent";

/* General */

/** Color theme: a swatch per theme, showing its panel, text and terminal colors. */
function AppearanceSettings() {
  const { theme, setTheme } = useWorkspacesPick("theme", "setTheme");
  return (
    <Card
      title="Theme"
      description={<>Also from the command palette ({keysFor({ type: "ui.palette" })}, “Theme:”). The sun/moon button in the sidebar switches to the light or dark counterpart.</>}
    >
      <div className="grid grid-cols-3 gap-2 p-3">
        {THEMES.map((t) => (
          <button
            key={t.id}
            onClick={() => setTheme(t.id)}
            title={t.name}
            className={cn(
              "overflow-hidden rounded-md border text-left transition-colors",
              t.id === theme ? "border-accent ring-1 ring-accent" : "border-border hover:border-border-strong",
            )}
          >
            <div className="flex items-center gap-1 px-2 py-2" style={{ background: t.ui.panel }}>
              <span className="font-mono text-[11px]" style={{ color: t.ui.fg }}>
                Aa
              </span>
              <span className="ml-auto flex gap-0.5">
                {[t.ansi[1], t.ansi[2], t.ansi[3], t.ansi[4], t.ansi[5]].map((c, i) => (
                  <span key={i} className="size-2 rounded-full" style={{ background: c }} />
                ))}
              </span>
              <span className="h-2 w-3 rounded-sm" style={{ background: t.ui.accent }} />
            </div>
            <div className="flex items-center gap-1 border-t border-border bg-panel-raised px-2 py-1 text-[11px] text-fg-muted">
              <span className="flex-1 truncate">{t.name}</span>
              {t.id === theme && <Check size={11} className="shrink-0 text-accent" />}
            </div>
          </button>
        ))}
      </div>
    </Card>
  );
}

/** App version and installing a newer release from GitHub, pinned to the bottom of the settings sidebar. */
function UpdateFooter() {
  const { version, channel, checking, installing, available, installed } = useUpdates();
  const small = "flex h-7 w-full items-center justify-center gap-1.5 rounded-md px-2.5 text-xs disabled:opacity-50";
  return (
    <div className="mt-auto space-y-1.5 border-t border-border px-0.5 pt-3 pb-1">
      <Select
        aria-label="Update channel"
        title="Stable: tested releases. Nebula: every change as soon as it lands."
        value={channel}
        onChange={(e) => setUpdateChannel(e.target.value as UpdateChannel)}
        disabled={installing}
        className="w-full"
      >
        <option value="stable">Stable updates</option>
        <option value="nebula">Nebula updates</option>
      </Select>
      <div className="px-2 text-[11px] text-fg-subtle">
        {available ? (
          <>
            <span className="font-medium text-fg">winfer {available.version}</span> is available
            <div>You have {version ?? "?"}</div>
          </>
        ) : (
          <>
            <span className="font-mono">winfer {version ?? ""}</span>
            {channel === "stable" && version?.includes("-") && <div>Moves to stable with the next stable release</div>}
          </>
        )}
      </div>
      {available ? (
        <button onClick={() => void installUpdate()} disabled={installing} className={cn(small, "bg-accent font-medium text-on-accent hover:opacity-90")}>
          {installing ? <Spinner size={12} /> : <Download size={12} />}
          Install and restart
        </button>
      ) : (
        <button onClick={() => void checkForUpdates()} disabled={checking} className={cn(small, "border border-border text-fg-muted hover:bg-hover hover:text-fg")}>
          {checking && <Spinner size={12} />}
          Check for updates
        </button>
      )}
      {(available || installed?.version === version) && (
        <button onClick={() => showReleaseNotes(available ? "available" : "installed")} className={cn(small, "text-fg-muted hover:bg-hover hover:text-fg")}>
          What's new
        </button>
      )}
    </div>
  );
}

/** Text size of every terminal. */
function TerminalSettings() {
  const { terminalFontSize, setTerminalFontSize } = useWorkspacesPick("terminalFontSize", "setTerminalFontSize");
  return (
    <Card title="Terminals">
      <Row
        label="Text size"
        hint={
          <>
            Also {keysFor({ type: "terminal.zoom", delta: 1 })} / {keysFor({ type: "terminal.zoom", delta: -1 })} /{" "}
            {keysFor({ type: "terminal.zoom", delta: 0 })}, or {IS_MAC ? "⌘" : "Ctrl"} + scroll (pinch on a trackpad) over a terminal.
          </>
        }
        stacked
      >
        <span className="font-mono text-[10px] text-fg-subtle">A</span>
        <input
          type="range"
          min={MIN_TERMINAL_FONT_SIZE}
          max={MAX_TERMINAL_FONT_SIZE}
          step={0.5}
          value={terminalFontSize}
          onChange={(e) => setTerminalFontSize(Number(e.target.value))}
          className="flex-1 accent-[var(--color-accent)]"
        />
        <span className="font-mono text-sm text-fg-subtle">A</span>
        <span className="w-12 text-right font-mono text-xs text-fg-muted">{terminalFontSize}px</span>
        <button
          disabled={terminalFontSize === DEFAULT_TERMINAL_FONT_SIZE}
          onClick={() => setTerminalFontSize(DEFAULT_TERMINAL_FONT_SIZE)}
          className="rounded-md px-2 py-1 text-xs text-fg-muted hover:bg-hover hover:text-fg disabled:opacity-40 disabled:hover:bg-transparent"
        >
          Reset
        </button>
      </Row>
    </Card>
  );
}

/** Which code editor "Open in editor" uses. */
function EditorSettings() {
  const { editor, setEditor } = useWorkspacesPick("editor", "setEditor");
  const editors = useEditors((s) => s.editors);
  // Picks up editors installed while winfer was running.
  useEffect(() => void loadEditors(), []);
  const current = preferredEditor(editors, editor);
  return (
    <Card title="Code editor">
      <Row
        label="Open projects and worktrees in"
        hint={
          editors.length
            ? `From a workspace's, agent's or worktree's right-click menu, or ${keysFor({ type: "editor.open", target: "focused" })} for the focused pane.`
            : "No supported editor found (VS Code, Cursor, Zed, Visual Studio, JetBrains IDEs, Sublime Text, Xcode…)."
        }
      >
        <Select value={current?.id ?? ""} onChange={(e) => setEditor(e.target.value)} disabled={!editors.length} className="w-44">
          {editors.length ? editors.map((e) => <option key={e.id} value={e.id}>{e.name}</option>) : <option value="">None installed</option>}
        </Select>
      </Row>
    </Card>
  );
}

function ShortcutSettings() {
  return (
    <Card title="Keyboard">
      <Row label="Keyboard shortcuts" hint="See every shortcut and change any of them.">
        <button onClick={() => useWorkspaces.getState().setCheatSheet(true)} className={buttonClass}>
          <Keyboard size={12} />
          Edit shortcuts
          <kbd className="font-mono text-[10px] text-fg-subtle">{keysFor({ type: "ui.cheatSheet" })}</kbd>
        </button>
      </Row>
    </Card>
  );
}

/* Agents */

/** Which agent ⌘T, splits and "open a new window" start, and the model each agent starts with. */
function AgentSettings() {
  const { providers, defaultProvider, setDefaultProvider, models, agentModels, setAgentModel, loadModels, workspaces, activeWorkspaceId, updateActive } = useWorkspacesPick("providers", "defaultProvider", "setDefaultProvider", "models", "agentModels", "setAgentModel", "loadModels", "workspaces", "activeWorkspaceId", "updateActive");
  const active = workspaces.find((w) => w.id === activeWorkspaceId);
  const current = defaultProviderId({ providers, defaultProvider });
  const agents = providers.filter((p) => p.installed && p.modelArg);
  useEffect(() => agents.forEach((p) => void loadModels(p.id)), [agents.length]); // eslint-disable-line react-hooks/exhaustive-deps
  return (
    <>
      <Card title="New panes">
        <Row label="Default agent" hint={`${keysFor({ type: "ui.overlay", overlay: "newPane" })}, split and “open a new window” start this agent.`}>
          <Select value={current} onChange={(e) => setDefaultProvider(e.target.value)} className="w-44">
            {providers.map((p) => (
              <option key={p.id} value={p.id} disabled={!p.installed}>
                {p.name}
                {p.installed ? "" : " (not installed)"}
              </option>
            ))}
          </Select>
        </Row>
        {active && (
          <Row
            label="Always open new agents in their own worktree"
            hint={`Applies to the workspace “${active.name}”. ${keysFor({ type: "pane.spawn", provider: "default", worktree: false })} opens one in the project folder anyway.`}
          >
            <Toggle
              label="Always open new agents in their own worktree"
              checked={active.useWorktrees}
              onChange={(useWorktrees) => updateActive(() => ({ useWorktrees }))}
            />
          </Row>
        )}
      </Card>
      {agents.length > 0 && (
        <Card title="Models" description="The model each agent starts with in new panes.">
          {agents.map((p) => {
            const list = models[p.id] ?? [];
            return (
              <Row key={p.id} label={p.name}>
                {list.length ? (
                  <Select value={agentModels[p.id] ?? ""} onChange={(e) => setAgentModel(p.id, e.target.value || null)} className="w-56">
                    <option value="">CLI default</option>
                    {list.map((m) => (
                      <option key={m.id} value={m.id}>
                        {m.label}
                      </option>
                    ))}
                  </Select>
                ) : (
                  <input
                    defaultValue={agentModels[p.id] ?? ""}
                    onBlur={(e) => setAgentModel(p.id, e.target.value.trim() || null)}
                    placeholder="CLI default (type a model name)"
                    className={cn(inputClass, "w-56")}
                  />
                )}
              </Row>
            );
          })}
        </Card>
      )}
      <VersionSettings />
      <AutoApproveSettings />
      <SystemPromptSettings />
      <AgentToolSettings />
    </>
  );
}

/** Each installed agent's version, and updating it to the newest one. */
function VersionSettings() {
  const { providers } = useWorkspacesPick("providers");
  const { versions, checking, updating, checkedAt } = useProviderUpdates();
  const agents = providers.filter((p) => p.installed && p.command);
  // Opening the page checks unless a check ran in the last few minutes.
  useEffect(() => {
    if (!checkedAt || Date.now() - checkedAt > 5 * 60_000) void checkProviderUpdates(true);
  }, []); // eslint-disable-line react-hooks/exhaustive-deps
  if (!agents.length) return null;
  return (
    <Card title="Versions" description="Updating changes the CLI on disk; running panes keep their version until restarted.">
      {agents.map((p) => {
        const v = versions[p.id];
        const busy = updating.includes(p.id);
        return (
          <Row
            key={p.id}
            label={
              <>
                {p.name}
                {v?.current && <span className="ml-1.5 font-mono text-[11px] text-fg-subtle">{v.current}</span>}
              </>
            }
            hint={
              !v
                ? checking
                  ? "Checking…"
                  : undefined
                : v.outdated
                  ? `${v.latest} is available${v.updatable ? `, updates via ${installLabel(v)}` : ""}.`
                  : v.latest
                    ? "Up to date."
                    : v.updatable
                      ? `Can't tell the newest version; updating asks ${installLabel(v)}.`
                      : undefined
            }
          >
            {busy && <Spinner size={13} className="text-fg-subtle" />}
            {v?.updatable && (v.outdated || !v.latest) && (
              <button
                disabled={busy}
                onClick={() => void updateProvider(p.id)}
                className={v.outdated ? "flex h-7 shrink-0 items-center gap-1.5 rounded-md bg-accent px-2.5 text-xs font-medium text-on-accent hover:brightness-110 disabled:opacity-50" : buttonClass}
              >
                <Download size={12} />
                {v.outdated ? `Update to ${v.latest}` : "Update"}
              </button>
            )}
          </Row>
        );
      })}
      <Row label="Check for agent updates" hint="winfer also checks every few hours and tells you when one is out.">
        <button onClick={() => void checkProviderUpdates()} disabled={checking} className={buttonClass}>
          {checking ? <Spinner size={12} /> : <RefreshCw size={12} />}
          Check now
        </button>
      </Row>
    </Card>
  );
}

/** Agents that start with their permission prompts skipped (each provider's `autoApproveArgs`). */
function AutoApproveSettings() {
  const { providers, autoApprove, setAutoApprove } = useWorkspacesPick("providers", "autoApprove", "setAutoApprove");
  const agents = providers.filter((p) => p.installed && p.autoApproveArgs?.length);
  if (!agents.length) return null;
  return (
    <Card title="Permissions" description="Applies to agents started (or restarted) after the change.">
      {agents.map((p) => (
        <Row key={p.id} label={`Start ${p.name} in auto-approve mode`} hint="Runs tools and edits without asking, except what its config explicitly denies.">
          <Toggle label={`Start ${p.name} in auto-approve mode`} checked={autoApprove.includes(p.id)} onChange={(on) => setAutoApprove(p.id, on)} />
        </Row>
      ))}
    </Card>
  );
}

/** Text added to every agent's system prompt when it starts (empty adds nothing). */
function SystemPromptSettings() {
  const { systemPrompt, setSystemPrompt } = useWorkspacesPick("systemPrompt", "setSystemPrompt");
  const saved = systemPrompt ?? DEFAULT_SYSTEM_PROMPT;
  const [draft, setDraft] = useState(saved);
  useEffect(() => setDraft(saved), [saved]);
  const save = () => setSystemPrompt(draft.trim() === DEFAULT_SYSTEM_PROMPT ? null : draft.trim());
  return (
    <Card
      title="System prompt"
      description="Given to every agent when it starts (or restarts). Claude Code and Codex get it as a system prompt; other agents get it in front of their first message. Leave it empty to add nothing."
    >
      <Row
        stacked
        label="Instructions for every agent"
        hint={systemPrompt !== null && (
          <button onClick={() => setSystemPrompt(null)} className="text-accent hover:underline">
            Reset to default
          </button>
        )}
      >
        <textarea
          value={draft}
          onChange={(e) => setDraft(e.target.value)}
          onBlur={save}
          rows={4}
          aria-label="System prompt"
          className="w-full resize-y rounded-md border border-border-strong bg-panel-raised px-2.5 py-2 text-xs leading-relaxed text-fg outline-none focus:border-accent"
        />
      </Row>
    </Card>
  );
}

/** The rarely used MCP tools agents start without, to turn back on one by one. */
function AgentToolSettings() {
  const { enabledTools, setToolEnabled } = useWorkspacesPick("enabledTools", "setToolEnabled");
  const plugins = usePlugins((s) => s.plugins);
  const tools = OPTIONAL_TOOLS.filter((t) => !t.plugin || plugins.some((p) => p.id === t.plugin && p.enabled && !p.error && p.manifest));
  return (
    <Card
      title="Agent tools"
      description="Tools agents rarely use are left out, since every tool an agent is offered takes up context in each request. Turn on the ones you want. Applies to agents started (or restarted) after the change."
    >
      {tools.map((t) => (
        <Row key={t.name} label={`${t.server}: ${t.name}`} hint={t.hint}>
          <Toggle label={t.name} checked={enabledTools.includes(t.name)} onChange={(on) => setToolEnabled(t.name, on)} />
        </Row>
      ))}
    </Card>
  );
}

/** What the review inbox does on its own when an agent finishes. */
function ReviewSettings() {
  const { reviewConfig, setReviewConfig } = useWorkspacesPick("reviewConfig", "setReviewConfig");
  return (
    <Card title="Review inbox" description="Summaries use the LLM under AI; with it off, cards show the agent's last output instead.">
      <Row label="Run tests when an agent finishes" hint="Runs the project's tests in the agent's worktree.">
        <Toggle label="Run tests when an agent finishes" checked={reviewConfig.autoTests ?? false} onChange={(autoTests) => setReviewConfig({ autoTests })} />
      </Row>
      <Row label="Screenshot the dev server" hint="Shows the agent's running app on its review card.">
        <Toggle label="Screenshot the dev server" checked={reviewConfig.screenshots ?? true} onChange={(screenshots) => setReviewConfig({ screenshots })} />
      </Row>
    </Card>
  );
}

/** Opt-in preview of the glass pane design. */
function PaneDesignSettings() {
  const glassPanes = useWorkspaces((s) => s.glassPanes);
  return (
    <Card title="Panes">
      <Row label="New pane design (preview)" hint="Glass panes with a floating two-row header: title and actions on top, branch or worktree, model, dev server links, cost and context below.">
        <Toggle label="New pane design (preview)" checked={glassPanes} onChange={(on) => useWorkspaces.getState().setGlassPanes(on)} />
      </Row>
    </Card>
  );
}

/** Opt-in: each agent pane's task cost in its header. */
function PaneCostSettings() {
  const showPaneCost = useWorkspaces((s) => s.showPaneCost);
  return (
    <Card title="On panes">
      <Row label="Show task cost on panes" hint="Each agent shows what its task has cost so far, at API prices, next to its title: its worktree's spend, or its session's when it works in a shared folder.">
        <Toggle label="Show task cost on panes" checked={showPaneCost} onChange={(on) => useWorkspaces.getState().setShowPaneCost(on)} />
      </Row>
    </Card>
  );
}

/** How often the LLM re-estimates working agents' progress. */
function ProgressSettings() {
  const { progressConfig, setProgressConfig } = useWorkspacesPick("progressConfig", "setProgressConfig");
  const enabled = progressConfig.enabled ?? true;
  return (
    <Card title="Session progress" description="Uses the LLM under AI. Agents whose output hasn't changed aren't re-checked.">
      <Row label="Estimate how far working agents are">
        <Toggle label="Estimate progress" checked={enabled} onChange={(enabled) => setProgressConfig({ enabled })} />
      </Row>
      {enabled && (
        <Row label="Check every">
          <Select value={progressConfig.intervalSec ?? 60} onChange={(e) => setProgressConfig({ intervalSec: Number(e.target.value) })} className="w-36">
            <option value={30}>30 seconds</option>
            <option value={60}>minute</option>
            <option value={120}>2 minutes</option>
            <option value={300}>5 minutes</option>
          </Select>
        </Row>
      )}
    </Card>
  );
}

/* Voice */

/** Microphone for live voice, from the webview's device list (labels show once access is granted). */
function Microphone() {
  const selected = useWorkspaces((s) => s.voiceConfig.inputDevice ?? "");
  const [devices, setDevices] = useState<string[]>([]);
  const [level, setLevel] = useState<number | null>(null);
  const [result, setResult] = useState<{ ok: boolean; text: string } | null>(null);

  const load = () =>
    void navigator.mediaDevices
      ?.enumerateDevices()
      .then((all) => setDevices([...new Set(all.filter((d) => d.kind === "audioinput" && d.label && d.deviceId !== "default").map((d) => d.label))]), () => {});
  useEffect(load, []);

  /** Listens for two seconds and reports how loud it was. */
  const test = async () => {
    setResult(null);
    setLevel(0);
    try {
      trace("mic test", { selected });
      const all = await timed("enumerateDevices", () => navigator.mediaDevices.enumerateDevices());
      const match = selected ? all.find((d) => d.kind === "audioinput" && d.label === selected) : undefined;
      const stream = await timed("getUserMedia", () => navigator.mediaDevices.getUserMedia({ audio: match ? { deviceId: { exact: match.deviceId } } : true }));
      load();
      trace("new AudioContext…");
      const ctx = new AudioContext();
      trace("AudioContext created", ctx.state);
      const analyser = ctx.createAnalyser();
      ctx.createMediaStreamSource(stream).connect(analyser);
      const buf = new Float32Array(analyser.fftSize);
      let peak = 0;
      const timer = window.setInterval(() => {
        analyser.getFloatTimeDomainData(buf);
        const rms = Math.sqrt(buf.reduce((sum, v) => sum + v * v, 0) / buf.length);
        peak = Math.max(peak, rms);
        setLevel(rms);
      }, 50);
      await new Promise((r) => setTimeout(r, 2000));
      clearInterval(timer);
      stream.getTracks().forEach((t) => t.stop());
      void ctx.close();
      const name = stream.getAudioTracks()[0]?.label || "the microphone";
      trace("mic test peak", peak);
      setResult(peak < 0.005 ? { ok: false, text: `Very quiet on “${name}”. Speak up or pick another microphone.` } : { ok: true, text: `Heard you on “${name}”.` });
    } catch (err) {
      setResult({ ok: false, text: String(err) });
    } finally {
      setLevel(null);
    }
  };

  // A saved device that isn't connected right now still shows, so the choice isn't lost.
  const names = [...devices, ...(selected && !devices.includes(selected) ? [selected] : [])];
  return (
    <Row
      label="Microphone"
      hint={
        level !== null ? (
          <span className="mt-1.5 block h-1.5 w-40 overflow-hidden rounded-full bg-hover">
            <span className="block h-full bg-success transition-[width] duration-75" style={{ width: `${Math.min(100, level * 1200)}%` }} />
          </span>
        ) : (
          result && <span className={result.ok ? "text-success" : "text-danger"}>{result.text}</span>
        )
      }
    >
      <Select
        value={selected}
        onFocus={load}
        onChange={(e) => {
          setResult(null);
          useWorkspaces.getState().setVoiceConfig({ inputDevice: e.target.value || undefined });
        }}
        className="w-56"
      >
        <option value="">System default</option>
        {names.map((name) => (
          <option key={name} value={name}>
            {name}
            {!devices.includes(name) ? " — not connected" : ""}
          </option>
        ))}
      </Select>
      <button disabled={level !== null} onClick={() => void test()} className={cn(buttonClass, "w-24 justify-center")}>
        {level !== null && <Spinner size={12} />}
        {level !== null ? "Listening…" : "Test"}
      </button>
    </Row>
  );
}

function VoiceSettings() {
  const readback = useWorkspaces((s) => s.voiceConfig.readback ?? false);
  return (
    <>
      <LiveVoiceSettings />
      <Card title="Input">
        <Microphone />
      </Card>
      <Card title="Read aloud">
        <Row label="Speak agent updates aloud" hint="“Codex needs your input”, “Claude Code finished”.">
          <Toggle label="Speak agent updates aloud" checked={readback} onChange={(readback) => useWorkspaces.getState().setVoiceConfig({ readback })} />
        </Row>
      </Card>
    </>
  );
}

/** The live voice agent's system prompt; empty or unchanged uses the default. */
function LivePrompt() {
  const custom = useWorkspaces((s) => s.voiceConfig.livePrompt);
  const saved = custom?.trim() || LIVE_ROLE;
  const [draft, setDraft] = useState(saved);
  useEffect(() => setDraft(saved), [saved]);
  const save = (text: string | undefined) => useWorkspaces.getState().setVoiceConfig({ livePrompt: text && text.trim() !== LIVE_ROLE ? text.trim() : undefined });
  return (
    <Row
      stacked
      label="System prompt"
      hint={
        <>
          How the voice agent behaves and talks. winfer adds the current panes, workspaces and browser tabs after it; changes apply right away, also mid-conversation.{" "}
          {custom && (
            <button onClick={() => save(undefined)} className="text-accent hover:underline">
              Reset to default
            </button>
          )}
        </>
      }
    >
      <textarea
        value={draft}
        onChange={(e) => setDraft(e.target.value)}
        onBlur={() => save(draft)}
        rows={8}
        aria-label="Live voice system prompt"
        className="w-full resize-y rounded-md border border-border-strong bg-panel-raised px-2.5 py-2 text-xs leading-relaxed text-fg outline-none focus:border-accent"
      />
    </Row>
  );
}

/** Live voice: a spoken conversation with OpenAI's realtime model that runs winfer's tools. */
const titleCase = (v: string) => v[0].toUpperCase() + v.slice(1);

function LiveVoiceSettings() {
  const signedIn = useChatGpt((s) => s.signedIn);
  const model = useWorkspaces((s) => s.voiceConfig.liveModel ?? "");
  const voice = useWorkspaces((s) => s.voiceConfig.liveVoice ?? DEFAULT_LIVE_VOICE);
  const chatgptVoice = useWorkspaces((s) => s.voiceConfig.liveChatgptVoice ?? DEFAULT_CHATGPT_VOICE);
  const auth = useWorkspaces((s) => s.voiceConfig.liveAuth ?? "auto");
  const [hasKey, setHasKey] = useState<boolean | null>(null);
  const [codexReady, setCodexReady] = useState<boolean | null>(null);
  const [key, setKey] = useState("");
  const [result, setResult] = useState<{ ok: boolean; text: string } | null>(null);
  const set = useWorkspaces.getState().setVoiceConfig;

  useEffect(() => {
    void invoke<boolean>("realtime_has_api_key").then(setHasKey, () => setHasKey(false));
    void invoke<boolean>("chatgpt_voice_ready").then(setCodexReady, () => setCodexReady(false));
  }, []);

  const saveKey = async (value: string | null) => {
    setResult(null);
    try {
      await invoke("realtime_set_api_key", { key: value });
      setHasKey(Boolean(value));
      setKey("");
      setResult({ ok: true, text: value ? "API key saved in the keychain." : "API key removed." });
    } catch (err) {
      setResult({ ok: false, text: String(err) });
    }
  };

  // What a conversation would use with the current choice.
  const uses = auth === "apikey" ? "api" : auth === "chatgpt" ? "chatgpt" : hasKey ? "api" : codexReady ? "chatgpt" : null;
  const status =
    uses === "api"
      ? hasKey
        ? "OpenAI API key"
        : "OpenAI API key (none saved)"
      : uses === "chatgpt"
        ? codexReady
          ? "ChatGPT plan (Codex login)"
          : "ChatGPT plan (Codex isn't logged in)"
        : "Not set up";
  const ok = (uses === "api" && hasKey) || (uses === "chatgpt" && codexReady);

  return (
    <>
      <Card
        title="Live voice"
        description={`Talk with winfer in a continuous conversation (${keysFor({ type: "voice.live" })} or the wave button in the title bar). An OpenAI voice model hears you, answers aloud and operates winfer: panes, agents, the browser, files and plugins. Audio goes to OpenAI.`}
      >
        <Row
          label="Connection"
          hint={
            <>
              <span className={ok ? undefined : "text-warning"}>Uses: {status}.</span> An API key runs OpenAI's realtime model, which operates winfer directly (fastest; billed to the API
              account). The ChatGPT plan runs its voice model on the Codex CLI's login (`codex login`){signedIn ? " and your ChatGPT sign-in" : ""}; it hands each request to a model that runs winfer's
              tools, so actions take a few seconds longer.
            </>
          }
        >
          <Select value={auth} onChange={(e) => set({ liveAuth: e.target.value as "auto" | "apikey" | "chatgpt" })} className="w-56">
            <option value="auto">Automatic (API key, else ChatGPT)</option>
            <option value="apikey">OpenAI API key</option>
            <option value="chatgpt">ChatGPT plan</option>
          </Select>
        </Row>
        <Row label="OpenAI API key" hint={hasKey ? "Stored in the keychain." : "Create one at platform.openai.com → API keys."}>
          <div className="flex items-center gap-2">
            <input
              type="password"
              className={cn(inputClass, "w-48")}
              value={key}
              placeholder={hasKey ? "••••••••" : "sk-…"}
              onChange={(e) => setKey(e.target.value)}
              spellCheck={false}
              aria-label="OpenAI API key"
            />
            <button disabled={!key.trim()} onClick={() => void saveKey(key)} className={buttonClass}>
              Save
            </button>
            {hasKey && (
              <button onClick={() => void saveKey(null)} className={buttonClass}>
                Remove
              </button>
            )}
          </div>
        </Row>
        <Row label="Model (API key)" hint={`Empty uses ${DEFAULT_LIVE_MODEL}. The ChatGPT plan always uses its own voice model.`}>
          <input className={cn(inputClass, "w-48")} value={model} placeholder={DEFAULT_LIVE_MODEL} onChange={(e) => set({ liveModel: e.target.value.trim() })} spellCheck={false} />
        </Row>
        <LivePrompt />
        <Row label="Voice" hint="API key · ChatGPT plan. Applies to the next conversation.">
          <div className="flex items-center gap-2">
            <Select value={voice} onChange={(e) => set({ liveVoice: e.target.value })} className="w-32" aria-label="Voice with an API key">
              {LIVE_VOICES.map((v) => (
                <option key={v} value={v}>
                  {titleCase(v)}
                </option>
              ))}
            </Select>
            <Select value={chatgptVoice} onChange={(e) => set({ liveChatgptVoice: e.target.value })} className="w-32" aria-label="Voice on the ChatGPT plan">
              {CHATGPT_VOICES.map((v) => (
                <option key={v} value={v}>
                  {titleCase(v)}
                </option>
              ))}
            </Select>
          </div>
        </Row>
      </Card>
      <Result result={result} />
    </>
  );
}

/* AI */

function AiSettings() {
  const chatgpt = useChatGpt();
  // What actually runs: the choice, or what it falls back to. Recomputed when the sign-in changes.
  const writer = useWorkspaces(() => llmProvider() ?? "off");
  const available = useWorkspaces(() => availableLlms().join(","));
  const writerModel = useWorkspaces((s) => s.voiceConfig.writerModel ?? "");
  const [busy, setBusy] = useState<string | null>(null);
  const [result, setResult] = useState<{ ok: boolean; text: string } | null>(null);

  const act = async (name: string, fn: () => Promise<string | void>) => {
    setBusy(name);
    setResult(null);
    try {
      const text = await fn();
      if (text) setResult({ ok: true, text });
    } catch (err) {
      setResult({ ok: false, text: String(err) });
    } finally {
      setBusy(null);
    }
  };

  const testConnection = async () => {
    const started = performance.now();
    await invoke("llm_compose", { req: { provider: "chatgpt", model: "", system: "Reply with OK.", user: "ping" } });
    return `Connected in ${Math.round(performance.now() - started)} ms`;
  };

  return (
    <>
      <Card
        title="ChatGPT account"
        description={
          <>
            Runs the LLM below on your ChatGPT plan (Plus or Pro); no API key needed. Until you sign in, the Codex CLI's own login is used
            instead if Codex is installed. Set a weekly limit or disconnect winfer in ChatGPT's settings.
          </>
        }
      >
        {chatgpt.signedIn ? (
          <Row
            label={
              <span className="flex items-center gap-2">
                <span className="grid size-5 place-items-center rounded-full bg-success/15">
                  <Check size={11} className="text-success" />
                </span>
                Signed in
              </span>
            }
            hint={chatgpt.email}
          >
            <button disabled={busy !== null} onClick={() => void act("test", testConnection)} className={buttonClass}>
              {busy === "test" && <Spinner size={12} />}
              {busy === "test" ? "Testing…" : "Test connection"}
            </button>
            <button
              disabled={busy !== null}
              onClick={() => void act("signout", async () => (await signOutOfChatGpt(), "Signed out"))}
              className="h-7 rounded-md px-2.5 text-xs text-fg-subtle hover:bg-hover hover:text-danger disabled:opacity-50"
            >
              Sign out
            </button>
          </Row>
        ) : (
          <Row label="Not signed in" hint={busy === "signin" ? "Finish signing in in your browser…" : undefined}>
            {busy === "signin" && (
              <button onClick={() => void cancelChatGptSignIn()} className="h-7 rounded-md px-2.5 text-xs text-fg-subtle hover:bg-hover hover:text-fg">
                Cancel
              </button>
            )}
            <button
              disabled={busy !== null}
              onClick={() => void act("signin", async () => (await signInWithChatGpt(), "Signed in"))}
              className="flex h-7 items-center gap-1.5 rounded-md bg-accent px-3 text-xs font-medium text-on-accent hover:opacity-90 disabled:opacity-60"
            >
              {busy === "signin" && <Spinner size={12} />}
              Continue with ChatGPT
            </button>
          </Row>
        )}
      </Card>
      <Result result={result} />

      <Card
        title="LLM"
        description="Writes progress estimates, review summaries and other short texts in winfer."
      >
        <Row
          label="Provider"
          hint={
            writer === "claude"
              ? "Runs Claude Code headless with your Claude login, without tools, MCP servers or your settings."
              : writer === "codex"
                ? "Runs Codex headless with your Codex login, read-only and without your Codex config."
                : undefined
          }
        >
          <Select
            value={writer}
            onChange={(e) => useWorkspaces.getState().setVoiceConfig({ writer: e.target.value as "off" | LlmProvider, writerModel: "" })}
            className="w-72"
          >
            {LLM_PROVIDERS.map((p) => {
              const ready = available.split(",").includes(p.id);
              return (
                <option key={p.id} value={p.id} disabled={!ready}>
                  {p.label}
                  {ready ? "" : p.id === "chatgpt" ? " (sign in above)" : " (not installed)"}
                </option>
              );
            })}
            <option value="off">Off (no progress estimates or review summaries)</option>
          </Select>
        </Row>
        {writer === "chatgpt" && chatgpt.models.length > 0 && (
          <Row label="Model">
            <Select value={writerModel || "gpt-6-luna"} onChange={(e) => useWorkspaces.getState().setVoiceConfig({ writer, writerModel: e.target.value })} className="w-56">
              {[...new Set(["gpt-6-luna", ...chatgpt.models])].map((m) => (
                <option key={m} value={m}>
                  {m}
                </option>
              ))}
            </Select>
          </Row>
        )}
        {(writer === "claude" || writer === "codex") && (
          <Row label="Model">
            <input
              key={writer}
              defaultValue={writerModel}
              onBlur={(e) => useWorkspaces.getState().setVoiceConfig({ writer, writerModel: e.target.value.trim() })}
              placeholder={writer === "claude" ? "Default, e.g. sonnet" : "Default, e.g. gpt-6.1-sol"}
              className={cn(inputClass, "w-56")}
            />
          </Row>
        )}
      </Card>
    </>
  );
}

/* winfer code */

interface HarnessSettings {
  model: string;
  effort: string | null;
  mode: "ask" | "auto-edit" | "plan" | "yolo-plan" | "yolo";
  trimAt: number;
  compactAt: number;
  allowedCommands: string[];
  mcpServers: Record<string, unknown>;
  leanBridges: boolean;
  leanAllSkills: boolean;
  leanSkills: string[];
  planSidebar: boolean;
  leanCodex: boolean;
  leanCodexPrompt: boolean;
  /** The model (backend's own id) and effort each backend's subagents run on; absent = the main model. */
  subagents: Record<string, { model: string; effort: string | null }>;
}

/** Backends whose subagents can run on a model of their own. */
const SUBAGENT_BACKENDS = ["chatgpt", "claude", "codex"];

/** A skill Claude Code can load from the user's setup (`~/.claude/skills` or an enabled plugin). */
interface ClaudeSkill {
  id: string;
  name: string;
  description: string;
  source: string;
}

interface HarnessModel {
  spec: string;
  label: string;
  detail: string;
}

const HARNESS_BACKENDS: { id: string; label: string }[] = [
  { id: "chatgpt", label: "ChatGPT account" },
  { id: "claude", label: "Claude Code" },
  { id: "codex", label: "Codex" },
];
/** winfer code's provider id (providers.json); its `autoApproveArgs` is `--yolo`. */
const WINFER_PROVIDER = "winfer";
const PERCENTS = [30, 35, 40, 45, 50, 55, 60, 65, 70, 75, 80, 85, 90, 95];
// Models load once per app run (asking Codex and OpenAI takes a few seconds).
let harnessModelsCache: HarnessModel[] | null = null;

function HarnessSettings() {
  const [settings, setSettings] = useState<HarnessSettings | null>(null);
  const [models, setModels] = useState<HarnessModel[] | null>(harnessModelsCache);
  const [mcpText, setMcpText] = useState("");
  const [claudeSkills, setClaudeSkills] = useState<ClaudeSkill[] | null>(null);
  const [result, setResult] = useState<{ ok: boolean; text: string } | null>(null);
  const autoApproveOn = useWorkspaces((s) => s.autoApprove.includes(WINFER_PROVIDER));
  const setAutoApprove = useWorkspaces((s) => s.setAutoApprove);
  const leanCodex = useWorkspaces((s) => s.leanCodex);
  const setLeanCodex = useWorkspaces((s) => s.setLeanCodex);
  const codeWeb = useCodeWeb();

  useEffect(() => {
    void invoke<HarnessSettings>("harness_settings").then((s) => {
      setSettings(s);
      setMcpText(Object.keys(s.mcpServers).length ? JSON.stringify(s.mcpServers, null, 2) : "");
    });
    void invoke<ClaudeSkill[]>("harness_claude_skills").then(setClaudeSkills, () => setClaudeSkills([]));
    if (!harnessModelsCache) {
      void invoke<HarnessModel[]>("harness_models").then((m) => {
        harnessModelsCache = m;
        setModels(m);
      });
    }
  }, []);

  if (!settings) {
    return (
      <Card title="winfer code">
        <Row label="Loading…">
          <Spinner size={13} className="text-fg-subtle" />
        </Row>
      </Card>
    );
  }

  const save = async (patch: Partial<HarnessSettings>) => {
    const next = { ...settings, ...patch };
    setSettings(next);
    try {
      await invoke("harness_set_settings", { settings: next });
      if (patch.planSidebar !== undefined) usePlanSidebar.setState({ on: patch.planSidebar });
      setResult(null);
    } catch (err) {
      setResult({ ok: false, text: String(err) });
    }
  };

  const saveMcp = (text: string) => {
    if (!text.trim()) return void save({ mcpServers: {} });
    try {
      const parsed = JSON.parse(text) as unknown;
      if (!parsed || typeof parsed !== "object" || Array.isArray(parsed)) throw new Error("expected an object of servers");
      void save({ mcpServers: parsed as Record<string, unknown> });
    } catch (err) {
      setResult({ ok: false, text: `MCP servers: ${err instanceof Error ? err.message : String(err)}` });
    }
  };

  const percent = (value: number, onChange: (v: number) => void, label: string) => (
    <Select aria-label={label} value={value} onChange={(e) => onChange(Number(e.target.value))} className="w-28">
      <option value={0}>Off</option>
      {PERCENTS.map((p) => (
        <option key={p} value={p}>
          {p}%
        </option>
      ))}
    </Select>
  );
  // A saved model that isn't in the list (or the list hasn't loaded) still shows.
  const known = models?.some((m) => m.spec === settings.model);
  const overlap = settings.trimAt > 0 && settings.compactAt > 0 && settings.trimAt >= settings.compactAt;

  return (
    <>
      <Card
        title="Defaults"
        description={
          <>
            For <span className="font-mono">winfer code</span>, winfer's own coding agent (pick “winfer code” as an agent, or run{" "}
            <span className="font-mono">winfer code</span> in a terminal). New sessions start with these; /model, /effort, /mode and /compaction change
            them from inside a session too.
          </>
        }
      >
        <Row label="Use TUI style" hint="New winfer code panes show the terminal UI. Turn off for the web interface (timeline of tool calls, inline diffs and approvals). Same sessions, settings and hooks; open panes keep what they started with.">
          <Toggle label="Use TUI style" checked={!codeWeb.web} onChange={(tui) => codeWeb.set({ web: !tui })} />
        </Row>
        {codeWeb.web && (
          <>
            <Row label="Context meter" hint="How full the context window is, in the status line. Click it for the breakdown (also /cost).">
              <Toggle label="Context meter" checked={codeWeb.showContext} onChange={(showContext) => codeWeb.set({ showContext })} />
            </Row>
            <Row label="Cost" hint="The session's cost in the status line, when the backend reports one (API pricing; your plan may cover it).">
              <Toggle label="Cost" checked={codeWeb.showCost} onChange={(showCost) => codeWeb.set({ showCost })} />
            </Row>
            <Row label="Tokens per second" hint="Streaming speed next to the working indicator.">
              <Toggle label="Tokens per second" checked={codeWeb.showRate} onChange={(showRate) => codeWeb.set({ showRate })} />
            </Row>
            <Row label="Plan above the composer" hint="The agent's todo list and running subagents.">
              <Toggle label="Plan above the composer" checked={codeWeb.showPlan} onChange={(showPlan) => codeWeb.set({ showPlan })} />
            </Row>
            <Row label="Show thinking" hint="Open the model's reasoning instead of collapsing it.">
              <Toggle label="Show thinking" checked={codeWeb.expandThinking} onChange={(expandThinking) => codeWeb.set({ expandThinking })} />
            </Row>
            <Row label="Live shell output" hint="Open a command's output while it runs.">
              <Toggle label="Live shell output" checked={codeWeb.expandShell} onChange={(expandShell) => codeWeb.set({ expandShell })} />
            </Row>
            <Row label="Compact layout" hint="Tighter spacing in the transcript.">
              <Toggle label="Compact layout" checked={codeWeb.compact} onChange={(compact) => codeWeb.set({ compact })} />
            </Row>
            <Row
              label="Text size"
              hint={
                <>
                  Also {keysFor({ type: "terminal.zoom", delta: 1 })} / {keysFor({ type: "terminal.zoom", delta: -1 })} /{" "}
                  {keysFor({ type: "terminal.zoom", delta: 0 })}, or {IS_MAC ? "⌘" : "Ctrl"} + scroll (pinch on a trackpad) over the pane.
                </>
              }
              stacked
            >
              <span className="text-[10px] text-fg-subtle">A</span>
              <input
                type="range"
                aria-label="Text size"
                min={MIN_TEXT_SCALE}
                max={MAX_TEXT_SCALE}
                step={TEXT_SCALE_STEP}
                value={codeWeb.textScale}
                onChange={(e) => setTextScale(Number(e.target.value))}
                className="flex-1 accent-[var(--color-accent)]"
              />
              <span className="text-sm text-fg-subtle">A</span>
              <span className="w-12 text-right font-mono text-xs text-fg-muted">{Math.round(codeWeb.textScale * 100)}%</span>
              <button
                disabled={codeWeb.textScale === 1}
                onClick={() => setTextScale(1)}
                className="rounded-md px-2 py-1 text-xs text-fg-muted hover:bg-hover hover:text-fg disabled:opacity-40 disabled:hover:bg-transparent"
              >
                Reset
              </button>
            </Row>
          </>
        )}
        <Row label="Model" hint="Automatic uses your ChatGPT account when signed in (Settings → AI), else Claude Code or Codex.">
          {models === null && <Spinner size={13} className="text-fg-subtle" />}
          <Select value={settings.model} onChange={(e) => void save({ model: e.target.value })} className="w-64">
            <option value="">Automatic</option>
            {settings.model && !known && <option value={settings.model}>{settings.model}</option>}
            {HARNESS_BACKENDS.map((b) => {
              const list = (models ?? []).filter((m) => m.spec.startsWith(`${b.id}/`));
              return (
                list.length > 0 && (
                  <optgroup key={b.id} label={b.label}>
                    {list.map((m) => (
                      <option key={m.spec} value={m.spec}>
                        {m.label}
                      </option>
                    ))}
                  </optgroup>
                )
              );
            })}
          </Select>
        </Row>
        <Row label="Reasoning effort">
          <Select value={settings.effort ?? ""} onChange={(e) => void save({ effort: e.target.value || null })} className="w-40">
            <option value="">Model default</option>
            {["minimal", "low", "medium", "high", "xhigh", "max"].map((e) => (
              <option key={e} value={e}>
                {e}
              </option>
            ))}
          </Select>
        </Row>
        {HARNESS_BACKENDS.filter((b) => SUBAGENT_BACKENDS.includes(b.id)).map((b) => {
          const cur = settings.subagents?.[b.id];
          const list = (models ?? []).filter((m) => m.spec.startsWith(`${b.id}/`) && !m.spec.endsWith("-fast"));
          const setSub = (model: string, effort: string | null) => {
            const next = { ...(settings.subagents ?? {}) };
            if (model) next[b.id] = { model, effort };
            else delete next[b.id];
            void save({ subagents: next });
          };
          return (
            <Row
              key={`sub-${b.id}`}
              label={`Subagent model · ${b.label}`}
              hint="What its subagents (exploring, web lookups, small fixes) run on; the main model orchestrates and checks their work. /subagent-model changes it in a session."
            >
              <Select value={cur?.model ?? ""} onChange={(e) => setSub(e.target.value, cur?.effort ?? null)} className="w-48">
                <option value="">Same as main model</option>
                {cur && !list.some((m) => m.spec === `${b.id}/${cur.model}`) && <option value={cur.model}>{cur.model}</option>}
                {list.map((m) => (
                  <option key={m.spec} value={m.spec.slice(b.id.length + 1)}>
                    {m.label}
                  </option>
                ))}
              </Select>
              {cur && b.id !== "claude" && (
                <Select value={cur.effort ?? ""} onChange={(e) => setSub(cur.model, e.target.value || null)} className="w-32">
                  <option value="">Default effort</option>
                  {["minimal", "low", "medium", "high", "xhigh"].map((e) => (
                    <option key={e} value={e}>
                      {e}
                    </option>
                  ))}
                </Select>
              )}
            </Row>
          );
        })}
        <Row label="Permission mode" hint={settings.mode === "yolo" ? "Nothing asks before editing files or running commands." : "Shift+Tab cycles it inside a session."}>
          <Select value={settings.mode} onChange={(e) => void save({ mode: e.target.value as HarnessSettings["mode"] })} className="w-56">
            <option value="ask">Ask before edits and commands</option>
            <option value="auto-edit">Auto-edit (commands still ask)</option>
            <option value="plan">Plan (read-only)</option>
            <option value="yolo-plan">Yolo-plan (read-only, never ask)</option>
            <option value="yolo">Yolo (never ask)</option>
          </Select>
        </Row>
        <Row
          label="Auto-approve in panes"
          hint={
            autoApproveOn
              ? "winfer code panes start in yolo mode (--yolo), overriding the permission mode above. Applies to panes started or restarted after the change."
              : "Start winfer code panes in yolo mode: nothing asks before editing files or running commands."
          }
        >
          <Toggle label="Auto-approve in panes" checked={autoApproveOn} onChange={(on) => setAutoApprove(WINFER_PROVIDER, on)} />
        </Row>
        <Row
          label="Plan sidebar"
          hint="Shows the agent's plan in a sidebar on the right when the pane is wide enough, in the TUI and the web UI; otherwise it stays above the prompt. /sidebar toggles it inside a session."
        >
          <Toggle label="Plan sidebar" checked={settings.planSidebar} onChange={(planSidebar) => void save({ planSidebar })} />
        </Row>
      </Card>

      <Card title="Lean setup" description="Starts agents with fewer tools and skills, so every request is smaller. Applies to sessions started (or restarted) after the change.">
        <Row
          label="Claude Code"
          hint="When winfer code runs a Claude model: a short system prompt and only the tools coding needs (Bash, Read, Edit, Write, Grep, Glob, the plan tools, Agent, plus Skill when it has skills; no user MCP servers). In the bench: 71% fewer input tokens, 79% fewer at full price, same pass rate. Project CLAUDE.md and project skills still load."
        >
          <Toggle label="Lean Claude Code" checked={settings.leanBridges} onChange={(leanBridges) => void save({ leanBridges })} />
        </Row>
        <Row
          label="Codex"
          hint="When winfer code runs a Codex model: no ChatGPT apps, plugins and their skills, bundled skills, image generation, computer use, Codex's own browser or goals, no permission, apps and collaboration-mode instructions, only the skills picked below, and each tool result capped at 4k tokens. winfer's tools stay. In the bench: 37% fewer input tokens, same pass rate."
        >
          <Toggle label="Lean Codex" checked={settings.leanCodex} onChange={(leanCodex) => void save({ leanCodex })} />
        </Row>
        {settings.leanCodex && (
          <Row
            label="Short Codex prompt"
            hint="Also swaps Codex's built-in system prompt (about 5k tokens) for a short one. In the bench: 52% fewer input tokens instead of 37%, same pass rate on 12 tasks, but Codex loses its own guidance on its tools."
          >
            <Toggle label="Short Codex prompt" checked={settings.leanCodexPrompt} onChange={(leanCodexPrompt) => void save({ leanCodexPrompt })} />
          </Row>
        )}
        <Row
          label="Codex CLI panes too"
          hint="Gives Codex CLI panes (not just winfer code) the same lean setup and output cap, with Codex's built-in prompt. Off by default: Codex runs as you configured it. A setup other sessions don't share may miss Codex's prompt cache, so it can cost more."
        >
          <Toggle label="Lean Codex CLI panes" checked={leanCodex} onChange={setLeanCodex} />
        </Row>
        {(settings.leanBridges || settings.leanCodex || leanCodex) && (
          <>
            <Row
              label="Skills"
              hint={
                settings.leanAllSkills
                  ? "Loads all your skills (Claude Code also gets your user settings, hooks and plugins). Each skill's description goes into every request."
                  : "Loads only the skills you pick below (Claude Code without your user settings, hooks or plugins). Picking none turns skills off. Claude Code's built-in skills and project skills still come along."
              }
            >
              <Select
                aria-label="Skills in lean setup"
                value={settings.leanAllSkills ? "all" : "selected"}
                onChange={(e) => void save({ leanAllSkills: e.target.value === "all" })}
              >
                <option value="all">All skills</option>
                <option value="selected">Selected only</option>
              </Select>
            </Row>
            {!settings.leanAllSkills && (
              <Row
                stacked
                label={`Selected skills (${settings.leanSkills.filter((id) => claudeSkills?.some((s) => s.id === id)).length})`}
                hint="Comes from ~/.claude/skills, your enabled Claude Code plugins, ~/.codex/skills and ~/.agents/skills. A skill counts for both agents by name."
              >
                {claudeSkills === null ? (
                  <Spinner size={13} className="text-fg-subtle" />
                ) : claudeSkills.length === 0 ? (
                  <span className="text-xs text-fg-subtle">No skills found.</span>
                ) : (
                  <div className="flex flex-wrap gap-1.5">
                    {claudeSkills.map((skill) => {
                      const on = settings.leanSkills.includes(skill.id);
                      return (
                        <Chip
                          key={skill.id}
                          active={on}
                          title={skill.description || skill.id}
                          onClick={() => void save({ leanSkills: on ? settings.leanSkills.filter((id) => id !== skill.id) : [...settings.leanSkills, skill.id] })}
                        >
                          {skill.source === "user" ? skill.name : `${skill.source}:${skill.name}`}
                        </Chip>
                      );
                    })}
                  </div>
                )}
              </Row>
            )}
          </>
        )}
      </Card>

      <Card title="Context compaction" description="When a conversation fills the model's context window, these keep it going and save tokens.">
        <Row label="Micro-compact at" hint="Trims old tool output and file contents from earlier turns. Claude Code and Codex are briefly restarted between turns to apply it.">
          {percent(settings.trimAt, (trimAt) => void save({ trimAt }), "Micro-compact at")}
        </Row>
        <Row
          label="Full compact at"
          hint={overlap ? <span className="text-warning">At or below micro-compact's level, so micro-compact never runs.</span> : "Summarizes the conversation; Claude Code and Codex are asked to compact."}
        >
          {percent(settings.compactAt, (compactAt) => void save({ compactAt }), "Full compact at")}
        </Row>
      </Card>

      <Card title="Tools" description="With the ChatGPT account, winfer code uses its own tools; Claude Code and Codex use theirs.">
        <Row label="Always-allowed commands" hint="Commands starting with these run without asking, e.g. git, cargo, pnpm (ChatGPT account).">
          <input
            key={settings.allowedCommands.join(",")}
            defaultValue={settings.allowedCommands.join(", ")}
            onBlur={(e) => void save({ allowedCommands: e.target.value.split(/[,\s]+/).filter(Boolean) })}
            placeholder="git, cargo, pnpm"
            className={cn(inputClass, "w-56")}
          />
        </Row>
        <Row
          stacked
          label="MCP servers"
          hint={
            <>
              Extra servers for every backend, as JSON: <span className="font-mono">{`{ "name": { "command": "…", "args": [], "env": {} } }`}</span>. winfer's own server is added
              automatically in panes.
            </>
          }
        >
          <textarea
            value={mcpText}
            onChange={(e) => setMcpText(e.target.value)}
            onBlur={(e) => saveMcp(e.target.value)}
            spellCheck={false}
            rows={4}
            placeholder="{}"
            className="w-full resize-y rounded-md border border-border-strong bg-panel-raised px-2.5 py-2 font-mono text-xs text-fg outline-none focus:border-accent"
          />
        </Row>
      </Card>
      <Result result={result} />
    </>
  );
}

/* Mobile */

const ago = (at: number) => {
  const s = Math.max(0, Math.round((Date.now() - at) / 1000));
  return s < 60 ? "just now" : s < 3600 ? `${Math.round(s / 60)} min ago` : s < 86400 ? `${Math.round(s / 3600)} h ago` : `${Math.round(s / 86400)} d ago`;
};

/** Pair phones with the mobile companion (docs/mobile/protocol.md) and manage them. */
function MobileSettings() {
  const [status, setStatus] = useState<MobileStatus | null>(null);
  const [pairing, setPairing] = useState<Pairing | null>(null);
  const [relay, setRelay] = useState("");
  const [result, setResult] = useState<{ ok: boolean; text: string } | null>(null);

  useEffect(() => {
    let alive = true;
    const refresh = () => void mobileStatus().then((s) => alive && setStatus(s)).catch(() => {});
    refresh();
    const unlisten = onMobileChange(refresh);
    // Relative times and link expiry move on even when nothing changes.
    const timer = window.setInterval(refresh, 15_000);
    return () => {
      alive = false;
      window.clearInterval(timer);
      void unlisten.then((f) => f());
    };
  }, []);
  useEffect(() => {
    if (status) setRelay(status.relayUrl);
  }, [status?.relayUrl]);

  // The QR code goes away once the phone has used it, or once it expires.
  const pending = pairing && status?.devices.find((d) => d.deviceId === pairing.deviceId);
  useEffect(() => {
    if (!pairing) return;
    if (pending?.paired) {
      setPairing(null);
      setResult({ ok: true, text: `Paired ${pending.name}.` });
    } else if (Date.now() > pairing.expiresAt) {
      setPairing(null);
      setResult({ ok: false, text: "The pairing code expired. Make a new one." });
    }
  }, [pairing, status, pending]);

  const pair = async () => {
    setResult(null);
    try {
      setPairing(await startPairing());
    } catch (err) {
      setResult({ ok: false, text: String(err) });
    }
  };
  const saveRelay = async () => {
    try {
      await setRelayUrl(relay);
      setResult({ ok: true, text: "Relay saved. Phones reconnect to it." });
    } catch (err) {
      setResult({ ok: false, text: String(err) });
    }
  };
  const revoke = async (id: string, name: string) => {
    if (!(await ask(`${name} will stop getting notifications and can't reach your agents until you pair it again.`, { title: `Unpair ${name}`, kind: "warning", okLabel: "Unpair" }))) return;
    await revokeDevice(id).catch((err) => setResult({ ok: false, text: String(err) }));
  };

  const paired = status?.devices.filter((d) => d.paired) ?? [];
  return (
    <>
      <Card
        title="Phone"
        description="Follow your agents from your phone: get a notification when one finishes or asks something, read its thread and answer it. Messages are end-to-end encrypted; the relay only passes them on."
      >
        <Row label="Pair a phone" hint="Scan the code with your phone's camera, open the page and add it to your Home Screen. On iPhone, notifications need iOS 16.4 or later and the Home Screen app.">
          <button className={buttonClass} onClick={() => void pair()}>
            <Smartphone size={13} />
            {pairing ? "New code" : "Pair phone"}
          </button>
        </Row>
        {pairing && (
          <div className="anim-fade flex items-center gap-4 px-3.5 py-3">
            <div className="size-[180px] shrink-0 overflow-hidden rounded-md bg-white p-1 [&>svg]:size-full" dangerouslySetInnerHTML={{ __html: pairing.qrSvg }} />
            <div className="min-w-0 space-y-2 text-xs text-fg-muted">
              <p>{pending?.connected ? "Waiting for your phone…" : pending?.error ? `Can't reach the relay: ${pending.error}` : "Connecting to the relay…"}</p>
              <p>The code works once, for 10 minutes. It holds the key to your agents: don't share it.</p>
              <button
                className={buttonClass}
                onClick={() => void navigator.clipboard.writeText(pairing.link).then(() => setResult({ ok: true, text: "Link copied." }))}
              >
                <Copy size={13} />
                Copy link
              </button>
            </div>
          </div>
        )}
        {paired.map((d) => (
          <Row
            key={d.deviceId}
            label={d.name}
            hint={[
              d.phones > 0 ? "Open now" : d.lastSeen ? `Last seen ${ago(d.lastSeen)}` : "Paired",
              d.push ? "notifications on" : "notifications off",
              !d.connected && d.error ? `relay: ${d.error}` : null,
            ]
              .filter(Boolean)
              .join(" · ")}
          >
            <span className={cn("size-2 rounded-full", d.phones > 0 ? "bg-success" : d.connected ? "bg-fg-subtle" : "bg-danger")} title={d.connected ? "Connected to the relay" : "Not connected to the relay"} />
            <button className={buttonClass} onClick={() => void revoke(d.deviceId, d.name)}>
              <Trash2 size={13} />
              Unpair
            </button>
          </Row>
        ))}
      </Card>
      <Card title="Relay" description="Where winfer and your phones meet. Change it only if you run your own relay (relay/ in the winfer repo).">
        <Row label="Relay URL" hint={status && status.relayUrl !== status.defaultRelayUrl ? `Default: ${status.defaultRelayUrl}` : undefined}>
          <input className={cn(inputClass, "w-64")} value={relay} onChange={(e) => setRelay(e.target.value)} spellCheck={false} />
          <button className={buttonClass} disabled={!status || relay.trim().replace(/\/+$/, "") === status.relayUrl} onClick={() => void saveRelay()}>
            Save
          </button>
        </Row>
      </Card>
      <Result result={result} />
    </>
  );
}

/* Data */

const EXPORT_FILTERS = [{ name: "winfer export", extensions: ["winfer"] }];

/** Moving winfer to another machine: export everything portable into one file, import it there. */
function DataSettings() {
  const [withSecrets, setWithSecrets] = useState(false);
  const [passphrase, setPassphrase] = useState("");
  const [confirm, setConfirm] = useState("");
  const [busy, setBusy] = useState<"export" | "import" | null>(null);
  const [exportResult, setExportResult] = useState<{ ok: boolean; text: string } | null>(null);
  const [importing, setImporting] = useState<{ path: string; manifest: ExportManifest } | null>(null);
  const [importPassphrase, setImportPassphrase] = useState("");
  const [importResult, setImportResult] = useState<{ ok: boolean; text: string } | null>(null);

  const mismatch = withSecrets && passphrase !== confirm;
  const canExport = !busy && (!withSecrets || (passphrase.length >= 8 && !mismatch));

  const runExport = async () => {
    const date = new Date().toISOString().slice(0, 10);
    const path = await saveDialog({ defaultPath: `winfer-${date}.winfer`, filters: EXPORT_FILTERS });
    if (!path) return;
    setBusy("export");
    setExportResult(null);
    try {
      await exportData(path, withSecrets ? passphrase : null);
      setExportResult({ ok: true, text: `Exported to ${path}` });
      setPassphrase("");
      setConfirm("");
    } catch (err) {
      setExportResult({ ok: false, text: `Export failed: ${err}` });
    } finally {
      setBusy(null);
    }
  };

  const pickImport = async () => {
    const path = await openDialog({ multiple: false, filters: EXPORT_FILTERS });
    if (typeof path !== "string") return;
    setImportResult(null);
    try {
      setImporting({ path, manifest: await inspectExport(path) });
      setImportPassphrase("");
    } catch (err) {
      setImporting(null);
      setImportResult({ ok: false, text: String(err) });
    }
  };

  const runImport = async () => {
    if (!importing) return;
    const confirmed = await ask(
      "Importing replaces this machine's settings, workspaces, plugins and usage history, then restarts winfer. The current data is kept in a pre-import folder in winfer's data folder.",
      { title: "Import winfer data?", kind: "warning", okLabel: "Import and restart" },
    );
    if (!confirmed) return;
    setBusy("import");
    setImportResult(null);
    try {
      await stageImport(importing.path, importing.manifest.hasSecrets && importPassphrase ? importPassphrase : null);
      suspendSaving();
      await relaunch();
    } catch (err) {
      setImportResult({ ok: false, text: `Import failed: ${err}` });
      setBusy(null);
    }
  };

  return (
    <>
      <Card
        title="Export"
        description="Saves settings, workspaces, plugins with their data, and usage history into one file to import on another machine. Agent sign-ins, paired phones, browser logins and winfer code chats stay here."
      >
        <Row label="Include secrets" hint="ChatGPT sign-in, the voice OpenAI key and plugin vault values, encrypted with a passphrase you'll enter when importing.">
          <Toggle checked={withSecrets} onChange={setWithSecrets} label="Include secrets" />
        </Row>
        {withSecrets && (
          <Row label="Passphrase" hint={mismatch && confirm ? "The passphrases don't match." : "At least 8 characters. It can't be recovered."} stacked>
            <input type="password" value={passphrase} onChange={(e) => setPassphrase(e.target.value)} placeholder="Passphrase" className={cn(inputClass, "flex-1")} />
            <input type="password" value={confirm} onChange={(e) => setConfirm(e.target.value)} placeholder="Repeat" className={cn(inputClass, "flex-1")} />
          </Row>
        )}
        <Row label="Export file">
          <button onClick={() => void runExport()} disabled={!canExport} className={buttonClass}>
            {busy === "export" ? <Spinner size={12} /> : <Download size={12} />}
            Export…
          </button>
        </Row>
      </Card>
      <Result result={exportResult} />

      <Card title="Import" description="Replaces this machine's data with an export, then restarts. Paths under the other machine's home folder are moved to this one's.">
        <Row label={importing ? importing.path.split(/[\\/]/).pop() : "Export file"} hint={importing && `From winfer ${importing.manifest.appVersion} on ${importing.manifest.platform}, ${new Date(importing.manifest.createdAt).toLocaleString()}`}>
          <button onClick={() => void pickImport()} disabled={!!busy} className={buttonClass}>
            <FolderOpen size={12} />
            {importing ? "Change…" : "Choose…"}
          </button>
        </Row>
        {importing?.manifest.hasSecrets && (
          <Row label="Passphrase" hint="Leave empty to import without the secrets.">
            <input type="password" value={importPassphrase} onChange={(e) => setImportPassphrase(e.target.value)} placeholder="Passphrase" className={cn(inputClass, "w-48")} />
          </Row>
        )}
        {importing && (
          <Row label="Import and restart">
            <button onClick={() => void runImport()} disabled={!!busy} className={buttonClass}>
              {busy === "import" ? <Spinner size={12} /> : <ArchiveRestore size={12} />}
              Import
            </button>
          </Row>
        )}
      </Card>
      <Result result={importResult} />
    </>
  );
}

const PAGES: { id: string; label: string; icon: LucideIcon; render: () => React.ReactNode }[] = [
  {
    id: "general",
    label: "General",
    icon: SlidersHorizontal,
    render: () => (
      <>
        <TerminalSettings />
        <EditorSettings />
        <ShortcutSettings />
      </>
    ),
  },
  { id: "theme", label: "Theme", icon: Palette, render: () => (
      <>
        <AppearanceSettings />
        <PaneDesignSettings />
      </>
    ),
  },
  {
    id: "agents",
    label: "Agents",
    icon: Bot,
    render: () => (
      <>
        <AgentSettings />
        <ReviewSettings />
        <ProgressSettings />
      </>
    ),
  },
  { id: "accounts", label: "Accounts", icon: Users, render: () => <AccountsSettings /> },
  { id: "voice", label: "Voice", icon: Mic, render: () => <VoiceSettings /> },
  { id: "ai", label: "AI", icon: Sparkles, render: () => <AiSettings /> },
  { id: "mobile", label: "Mobile", icon: Smartphone, render: () => <MobileSettings /> },
  { id: "winfer-code", label: "winfer code", icon: SquareTerminal, render: () => <HarnessSettings /> },
  {
    id: "usage",
    label: "Usage",
    icon: ChartColumn,
    render: () => (
      <UsageSettings>
        <PaneCostSettings />
      </UsageSettings>
    ),
  },
  { id: "plugins", label: "Plugins", icon: Puzzle, render: () => <PluginSettings /> },
  { id: "skills", label: "Skills", icon: Wand2, render: () => <SkillSettings /> },
  { id: "data", label: "Data", icon: ArchiveRestore, render: () => <DataSettings /> },
];

/** What a plugin adds, e.g. "2 tools · 1 agent · code". */
function contributions(plugin: PluginInfo): string {
  const m = plugin.manifest;
  if (!m) return "";
  const count = (n: number, one: string) => (n ? [`${n} ${one}${n === 1 ? "" : "s"}`] : []);
  return [...count(m.tools.length, "tool"), ...count(m.providers.length, "agent"), ...count(m.recipes.length, "recipe"), ...(m.main ? ["code"] : [])].join(" · ");
}

const trustWarning = (name: string) =>
  `“${name}” runs its own code with full access to your files, terminals and network. Only turn on plugins you trust.`;

async function togglePlugin(plugin: PluginInfo, enabled: boolean) {
  // Plugin code and agent commands run with winfer's access to files, terminals and the network.
  if (enabled && !plugin.fromStore && (plugin.manifest?.main || plugin.manifest?.providers.length)) {
    const ok = await ask(trustWarning(plugin.manifest.name), { title: "Turn on plugin", kind: "warning", okLabel: "Turn on" });
    if (!ok) return;
  }
  await setPluginEnabled(plugin.id, enabled);
}

// The store's catalog, kept while the app runs so reopening the page is instant.
let catalogCache: StoreEntry[] | null = null;

type StoreTab = "discover" | "installed";

/** The sticky bar on the Plugins and Skills pages: Discover / Installed tabs and a search box. */
function StoreToolbar({
  tab,
  onTab,
  counts,
  updates,
  query,
  onQuery,
  placeholder,
}: {
  tab: StoreTab;
  onTab: (tab: StoreTab) => void;
  counts: Record<StoreTab, number | null>;
  updates: boolean;
  query: string;
  onQuery: (query: string) => void;
  placeholder: string;
}) {
  const tabs: { id: StoreTab; label: string; icon: LucideIcon }[] = [
    { id: "discover", label: "Discover", icon: Store },
    { id: "installed", label: "Installed", icon: PackageCheck },
  ];
  return (
    <div className="sticky -top-5 z-10 -mx-6 -mt-5 flex items-center gap-3 border-b border-border bg-panel-raised/95 px-6 py-3 backdrop-blur">
      <div role="tablist" className="flex shrink-0 rounded-lg border border-border bg-panel p-0.5">
        {tabs.map((t) => (
          <button
            key={t.id}
            role="tab"
            aria-selected={tab === t.id}
            onClick={() => onTab(t.id)}
            className={cn("relative flex h-7 items-center gap-1.5 rounded-md px-3 text-xs transition-colors", tab === t.id ? "bg-active text-fg shadow-sm" : "text-fg-muted hover:text-fg")}
          >
            <t.icon size={12} />
            {t.label}
            {counts[t.id] !== null && <span className="font-mono text-[10px] text-fg-subtle">{counts[t.id]}</span>}
            {t.id === "installed" && updates && <span className="absolute top-1 right-1 size-1.5 rounded-full bg-accent" title="Updates available" />}
          </button>
        ))}
      </div>
      <div className="relative min-w-0 flex-1">
        <Search size={13} className="pointer-events-none absolute top-1/2 left-2.5 -translate-y-1/2 text-fg-subtle" />
        <input
          type="search"
          value={query}
          onChange={(e) => onQuery(e.target.value)}
          placeholder={placeholder}
          autoFocus
          className="h-8 w-full rounded-lg border border-border-strong bg-panel pr-2.5 pl-8 text-xs text-fg outline-none focus:border-accent"
        />
      </div>
    </div>
  );
}

/** "N updates are available" with an Update all button, atop an Installed tab. */
const UpdateBanner = ({ text, label, busy, disabled, onClick }: { text: string; label: string; busy: boolean; disabled: boolean; onClick: () => void }) => (
  <div className="flex items-center gap-3 rounded-lg border border-accent/30 bg-accent-soft px-3.5 py-2.5">
    <ArrowUpCircle size={15} className="shrink-0 text-accent" />
    <p className="flex-1 text-xs text-fg">{text}</p>
    <button disabled={disabled} onClick={onClick} className={primaryButtonClass}>
      {busy ? <Spinner size={12} /> : <Download size={12} />}
      {label}
    </button>
  </div>
);

/** Grey placeholder cards while a listing loads. */
const SkeletonGrid = () => (
  <div className="grid grid-cols-2 gap-3">
    {[0, 1, 2, 3].map((i) => (
      <div key={i} className="h-40 animate-pulse rounded-lg border border-border bg-panel" />
    ))}
  </div>
);

const primaryButtonClass =
  "flex h-7 shrink-0 items-center gap-1.5 rounded-md bg-accent px-2.5 text-xs font-medium text-on-accent hover:brightness-110 disabled:opacity-50";
const iconButtonClass = "flex size-7 shrink-0 items-center justify-center rounded-md text-fg-subtle hover:bg-hover hover:text-fg disabled:opacity-50";

/** A plugin's monogram ("Cross-Project Knowledge" → "CK") on a tile tinted by its id. */
function PluginTile({ id, name, size = "md" }: { id: string; name: string; size?: "md" | "lg" }) {
  let hash = 0;
  for (const ch of id) hash = (hash * 31 + ch.charCodeAt(0)) | 0;
  const hue = Math.abs(hash) % 360;
  const words = name.replace(/[^\p{L}\p{N}\s-]/gu, " ").split(/[\s-]+/).filter(Boolean);
  const letters = (words.length > 1 ? words[0][0] + words[words.length - 1][0] : (words[0] ?? id).slice(0, 2)).toUpperCase();
  return (
    <div
      aria-hidden
      className={cn("flex shrink-0 items-center justify-center rounded-lg font-semibold tracking-tight", size === "lg" ? "size-10 text-sm" : "size-9 text-xs")}
      style={{ background: `hsl(${hue} 65% 55% / 0.14)`, color: `color-mix(in oklab, hsl(${hue} 65% 50%) 70%, var(--color-fg))`, boxShadow: `inset 0 0 0 1px hsl(${hue} 65% 55% / 0.22)` }}
    >
      {letters}
    </div>
  );
}

const Pill = ({ children, tone = "neutral", title }: { children: React.ReactNode; tone?: "neutral" | "accent" | "warning" | "danger"; title?: string }) => (
  <span
    title={title}
    className={cn(
      "inline-flex h-[18px] shrink-0 items-center gap-1 rounded px-1.5 text-[10px] font-medium",
      tone === "neutral" && "bg-active text-fg-muted",
      tone === "accent" && "bg-accent-soft text-accent",
      tone === "warning" && "bg-warning/15 text-warning",
      tone === "danger" && "bg-danger/15 text-danger",
    )}
  >
    {children}
  </span>
);

/** Empty or loading state in the middle of a tab. */
const EmptyState = ({ icon: Icon, title, hint, children }: { icon: LucideIcon; title: string; hint?: React.ReactNode; children?: React.ReactNode }) => (
  <div className="flex flex-col items-center rounded-lg border border-dashed border-border-strong px-6 py-10 text-center">
    <Icon size={20} className="text-fg-subtle" />
    <p className="mt-3 text-[13px] text-fg">{title}</p>
    {hint && <p className="mt-1 max-w-sm text-xs leading-relaxed text-fg-subtle">{hint}</p>}
    {children && <div className="mt-4 flex gap-2">{children}</div>}
  </div>
);

// The tab the page last showed, kept while the app runs.
let lastStoreTab: StoreTab | null = null;

/** The plugin store: browse and install plugins, and turn on, update or uninstall installed ones. */
function PluginSettings() {
  const { plugins, runtimeErrors } = usePlugins();
  const iconsOnly = useWorkspaces((s) => s.pageTabIconsOnly);
  const [catalog, setCatalog] = useState<StoreEntry[] | null>(catalogCache);
  const [catalogError, setCatalogError] = useState<string | null>(null);
  const [tab, setTabState] = useState<StoreTab>(lastStoreTab ?? (plugins.some((p) => p.manifest) ? "installed" : "discover"));
  const [query, setQuery] = useState("");
  const [tag, setTag] = useState<string | null>(null);
  const [expanded, setExpanded] = useState<string | null>(null);
  const [busy, setBusy] = useState<string | null>(null);

  const setTab = (next: StoreTab) => {
    lastStoreTab = next;
    setTabState(next);
  };

  const loadCatalog = () => {
    setCatalogError(null);
    fetchCatalog().then(
      (list) => {
        catalogCache = list;
        setCatalog(list);
      },
      (err) => setCatalogError(String(err)),
    );
  };
  useEffect(loadCatalog, []);

  const run = async (id: string, work: () => Promise<void>) => {
    setBusy(id);
    try {
      await work();
    } catch (err) {
      toast(String(err), "error");
    } finally {
      setBusy(null);
    }
  };
  const install = (entry: StoreEntry, update: boolean) =>
    run(entry.id, async () => {
      if (!update && !(await ask(trustWarning(entry.name), { title: `Install ${entry.name}`, kind: "warning", okLabel: "Install" }))) return;
      await installPlugin(entry.id);
      toast(update ? `Updated ${entry.name} to ${entry.version}` : `Installed ${entry.name}`, "success");
    });
  const uninstall = (plugin: PluginInfo) =>
    run(plugin.id, async () => {
      const name = plugin.manifest?.name ?? plugin.id;
      if (!(await ask(`Remove “${name}” and its files? Data it stored (like saved secrets) is kept for a reinstall.`, { title: "Uninstall plugin", kind: "warning", okLabel: "Uninstall" }))) return;
      await uninstallPlugin(plugin.id);
    });

  const latestFor = (p: PluginInfo) => (p.fromStore ? catalog?.find((e) => e.id === p.id && e.version !== (p.storeVersion ?? p.manifest?.version)) : undefined);
  const updates = plugins.map((p) => latestFor(p)).filter((e) => e !== undefined);
  const updateAll = () =>
    run("*", async () => {
      for (const entry of updates) await installPlugin(entry.id);
      toast(`Updated ${updates.length} plugin${updates.length === 1 ? "" : "s"}`, "success");
    });

  const notInstalled = (catalog ?? []).filter((e) => !plugins.some((p) => p.id === e.id));
  // The most used tags among plugins left to install, as filters.
  const tagCounts = new Map<string, number>();
  for (const e of notInstalled) for (const t of e.tags) tagCounts.set(t, (tagCounts.get(t) ?? 0) + 1);
  const topTags = [...tagCounts].filter(([, n]) => n > 1).sort((a, b) => b[1] - a[1] || a[0].localeCompare(b[0])).slice(0, 8).map(([t]) => t);
  if (tag && !topTags.includes(tag)) topTags.push(tag);

  const installed = plugins.filter((p) => matchesSearch(query, [p.id, p.manifest?.name, p.manifest?.description]));
  const available = notInstalled.filter((e) => (!tag || e.tags.includes(tag)) && matchesSearch(query, [e.id, e.name, e.description, e.author, ...e.tags]));
  const enabledCount = plugins.filter((p) => p.enabled && !p.error).length;

  return (
    <div className="space-y-5">
      <StoreToolbar
        tab={tab}
        onTab={setTab}
        counts={{ discover: catalog ? notInstalled.length : null, installed: plugins.length }}
        updates={updates.length > 0}
        query={query}
        onQuery={setQuery}
        placeholder={tab === "discover" ? "Search the store" : "Search installed plugins"}
      />

      {tab === "discover" ? (
        <>
          {catalogError ? (
            <EmptyState icon={CloudOff} title="Couldn't load the plugin store" hint={catalogError}>
              <button onClick={loadCatalog} className={buttonClass}>
                <RefreshCw size={12} />
                Retry
              </button>
            </EmptyState>
          ) : catalog === null ? (
            <SkeletonGrid />
          ) : (
            <>
              {topTags.length > 0 && (
                <div className="flex flex-wrap items-center gap-1.5">
                  <Chip active={tag === null} onClick={() => setTag(null)}>
                    All
                  </Chip>
                  {topTags.map((t) => (
                    <Chip key={t} active={tag === t} onClick={() => setTag(tag === t ? null : t)}>
                      {t}
                    </Chip>
                  ))}
                </div>
              )}
              {available.length === 0 ? (
                notInstalled.length === 0 ? (
                  <EmptyState icon={PackageCheck} title="You have every plugin from the store" hint="New plugins show up here when they're published." />
                ) : (
                  <EmptyState icon={Search} title={`Nothing in the store matches${query ? ` “${query}”` : ""}${tag ? ` in ${tag}` : ""}`}>
                    <button onClick={() => (setQuery(""), setTag(null))} className={buttonClass}>
                      Clear filters
                    </button>
                  </EmptyState>
                )
              ) : (
                <div className="grid grid-cols-2 gap-3">
                  {available.map((e) => {
                    const open = expanded === e.id;
                    return (
                      <article key={e.id} className="group flex flex-col rounded-lg border border-border bg-panel p-3.5 transition-colors hover:border-border-strong">
                        <div className="flex items-start gap-3">
                          <PluginTile id={e.id} name={e.name} />
                          <div className="min-w-0 flex-1">
                            <h4 className="truncate text-[13px] font-medium text-fg" title={e.name}>
                              {e.name}
                            </h4>
                            <p className="truncate text-[11px] text-fg-subtle">
                              {e.author ? `by ${e.author}` : "Unknown author"}
                              {e.version && <span className="font-mono"> · {e.version}</span>}
                            </p>
                          </div>
                        </div>
                        <button
                          onClick={() => setExpanded(open ? null : e.id)}
                          title={open ? "Show less" : "Show more"}
                          className={cn("mt-2.5 text-left text-xs leading-relaxed text-fg-muted hover:text-fg", !open && "line-clamp-3")}
                        >
                          {e.description || "No description."}
                        </button>
                        <div className="mt-auto flex items-end gap-2 pt-3">
                          <div className={cn("flex min-w-0 flex-1 gap-1", open ? "flex-wrap" : "overflow-hidden")}>
                            {e.tags.slice(0, open ? undefined : 2).map((t) => (
                              <button
                                key={t}
                                onClick={() => setTag(t)}
                                className={cn("shrink-0 rounded px-1.5 py-0.5 text-[10px] hover:text-fg", t === tag ? "bg-accent-soft text-accent" : "bg-active text-fg-subtle")}
                              >
                                {t}
                              </button>
                            ))}
                            {!open && e.tags.length > 2 && (
                              <button onClick={() => setExpanded(e.id)} className="shrink-0 px-1 py-0.5 text-[10px] text-fg-subtle hover:text-fg">
                                +{e.tags.length - 2}
                              </button>
                            )}
                          </div>
                          <button
                            disabled={!!busy}
                            onClick={() => void install(e, false)}
                            className={primaryButtonClass}
                          >
                            {busy === e.id ? <Spinner size={12} /> : <Download size={12} />}
                            Install
                          </button>
                        </div>
                      </article>
                    );
                  })}
                </div>
              )}
            </>
          )}
        </>
      ) : (
        <>
          {updates.length > 0 && !query && (
            <UpdateBanner
              text={updates.length === 1 ? `An update for ${updates[0].name} is available.` : `${updates.length} plugin updates are available.`}
              label={updates.length === 1 ? "Update" : "Update all"}
              busy={busy === "*"}
              disabled={!!busy}
              onClick={() => void (updates.length === 1 ? install(updates[0], true) : updateAll())}
            />
          )}

          {installed.length === 0 ? (
            plugins.length === 0 ? (
              <EmptyState icon={Puzzle} title="No plugins installed" hint="Plugins add pages, agents, recipes, commands, voice actions and tools for your agents.">
                <button onClick={() => setTab("discover")} className="flex h-7 items-center gap-1.5 rounded-md bg-accent px-2.5 text-xs font-medium text-on-accent hover:brightness-110">
                  <Store size={12} />
                  Browse the store
                </button>
              </EmptyState>
            ) : (
              <EmptyState icon={Search} title={`No installed plugin matches “${query}”`} />
            )
          ) : (
            <section className="space-y-2">
              <p className="px-1 text-[11px] text-fg-subtle">
                {enabledCount} of {plugins.length} on
              </p>
              <div className="divide-y divide-border overflow-hidden rounded-lg border border-border bg-panel">
                {installed.map((p) => {
                  const error = p.error ?? (p.enabled ? runtimeErrors[p.id] : undefined);
                  const name = p.manifest?.name ?? p.id;
                  const latest = latestFor(p);
                  const version = p.storeVersion ?? p.manifest?.version;
                  const adds = contributions(p);
                  return (
                    <div key={p.id} className={cn("group flex gap-3 px-3.5 py-3 transition-opacity", !p.enabled && !error && "[&>.wf-dim]:opacity-60")}>
                      <div className="wf-dim">
                        <PluginTile id={p.id} name={name} />
                      </div>
                      <div className="wf-dim min-w-0 flex-1">
                        <div className="flex min-w-0 items-center gap-1.5">
                          <h4 className="truncate text-[13px] font-medium text-fg">{name}</h4>
                          {version && <span className="shrink-0 font-mono text-[11px] text-fg-subtle">{version}</span>}
                          {!p.fromStore && <Pill title="From your plugins folder">local</Pill>}
                          {latest && <Pill tone="accent">{latest.version} available</Pill>}
                        </div>
                        {p.manifest?.description && <p className="mt-0.5 line-clamp-2 text-xs leading-relaxed text-fg-muted" title={p.manifest.description}>{p.manifest.description}</p>}
                        {adds && <p className="mt-1 text-[11px] text-fg-subtle">Adds {adds}</p>}
                        {error && (
                          <p className="mt-2 flex items-start gap-1.5 rounded-md bg-danger/10 px-2 py-1.5 text-[11px] leading-relaxed text-danger">
                            <TriangleAlert size={12} className="mt-px shrink-0" />
                            <span className="min-w-0 break-words">{error}</span>
                          </p>
                        )}
                      </div>
                      <div className="flex shrink-0 items-start gap-1.5">
                        {busy === p.id && <Spinner size={13} className="mt-1.5 text-fg-subtle" />}
                        {latest && (
                          <button disabled={!!busy} onClick={() => void install(latest, true)} className={buttonClass}>
                            <Download size={12} />
                            Update
                          </button>
                        )}
                        <button
                          disabled={!!busy}
                          onClick={() => void uninstall(p)}
                          aria-label={`Uninstall ${name}`}
                          title="Uninstall"
                          className="flex size-7 items-center justify-center rounded-md text-fg-subtle opacity-0 group-hover:opacity-100 hover:bg-hover hover:text-danger focus-visible:opacity-100 disabled:opacity-50"
                        >
                          <Trash2 size={13} />
                        </button>
                        {!p.error && (
                          <div className="flex h-7 items-center">
                            <Toggle label={`Turn on ${name}`} checked={p.enabled} onChange={(enabled) => void togglePlugin(p, enabled)} />
                          </div>
                        )}
                      </div>
                    </div>
                  );
                })}
              </div>
            </section>
          )}

          <Card title="Options">
            <Row label="Hide names on plugin tabs" hint="Plugin page tabs in the title bar show just their icon. Hover a tab to see its name.">
              <Toggle label="Hide names on plugin tabs" checked={iconsOnly} onChange={(on) => useWorkspaces.getState().setPageTabIconsOnly(on)} />
            </Row>
            <Row label="Your own plugins" hint="A folder with a plugin.json in the plugins folder shows up here, off until you turn it on. Reload after changing one.">
              <button onClick={() => void pluginsDir().then((dir) => revealItemInDir(dir)).catch(() => {})} className={buttonClass}>
                <FolderOpen size={12} />
                Open folder
              </button>
              <button onClick={() => void loadPlugins()} aria-label="Reload plugins" title="Reload plugins" className={cn(buttonClass, "w-7 justify-center px-0")}>
                <RefreshCw size={15} className="shrink-0" />
              </button>
            </Row>
          </Card>
        </>
      )}
    </div>
  );
}

/* Skills */

// Repo listings, kept while the app runs so reopening the page is instant.
const skillListings = new Map<string, RepoListing>();

const Chip = ({ active, onClick, children, title }: { active: boolean; onClick?: () => void; children: React.ReactNode; title?: string }) => (
  <button
    onClick={onClick}
    title={title}
    aria-pressed={active}
    className={cn(
      "flex h-6 items-center gap-1 rounded-md border px-2 text-[11px] transition-colors",
      active ? "border-accent/60 bg-accent/15 text-fg" : "border-border-strong text-fg-muted hover:bg-hover hover:text-fg",
    )}
  >
    {active && <Check size={11} className="text-accent" />}
    {children}
  </button>
);

// The tab the page last showed, kept while the app runs.
let lastSkillTab: StoreTab | null = null;

/** Installs agent skills (SKILL.md folders) from GitHub repos into each agent's skills folder. */
function SkillSettings() {
  const [targets, setTargets] = useState<SkillTarget[]>([]);
  const [selected, setSelected] = useState<string[]>(savedTargets);
  const [installed, setInstalled] = useState<InstalledSkill[] | null>(null);
  const [sources, setSources] = useState<string[]>(customSources);
  const [source, setSource] = useState(FEATURED_SOURCES[0].repo);
  const [listings, setListings] = useState<Map<string, RepoListing>>(() => new Map(skillListings));
  const [listingError, setListingError] = useState<string | null>(null);
  const [newRepo, setNewRepo] = useState("");
  const [query, setQuery] = useState("");
  const [busy, setBusy] = useState<string | null>(null);
  const [tab, setTabState] = useState<StoreTab>(lastSkillTab ?? "installed");
  const [expanded, setExpanded] = useState<string | null>(null);
  const setTab = (next: StoreTab) => {
    lastSkillTab = next;
    setTabState(next);
  };

  // Project-level skills and commands (e.g. specs.md's) live in the workspaces' folders.
  const workspaces = useWorkspaces((s) => s.workspaces);
  const projectKey = [...new Set(workspaces.map((w) => w.rootDir).filter(Boolean))].join("\n");
  const projects = useMemo(() => (projectKey ? projectKey.split("\n") : []), [projectKey]);
  const reload = () => fetchInstalledSkills(projects).then(setInstalled, (err) => toast(String(err), "error"));
  useEffect(() => {
    fetchTargets().then(setTargets, (err) => toast(String(err), "error"));
  }, []);
  useEffect(() => {
    void reload();
  }, [projects]);

  const loadListing = (repo: string, fresh = false) => {
    setListingError(null);
    if (fresh) skillListings.delete(repo);
    setListings(new Map(skillListings));
    return fetchRepoSkills(repo, fresh).then(
      (listing) => {
        skillListings.set(repo, listing);
        setListings(new Map(skillListings));
      },
      (err) => {
        if (repo === source) setListingError(String(err));
      },
    );
  };
  useEffect(() => {
    if (!skillListings.has(source)) void loadListing(source);
  }, [source]);
  // The repos installed skills came from, for their updates.
  useEffect(() => {
    const repos = new Set((installed ?? []).map(skillOrigin).filter((o) => o !== null).map((o) => o.repo));
    for (const repo of repos) if (!skillListings.has(repo)) void fetchRepoSkills(repo).then((l) => (skillListings.set(repo, l), setListings(new Map(skillListings))), () => {});
  }, [installed]);

  const toggleTarget = (id: string) => {
    const next = selected.includes(id) ? selected.filter((t) => t !== id) : [...selected, id];
    setSelected(next);
    saveTargets(next);
  };
  const label = (id: string) => targets.find((t) => t.id === id)?.label ?? id;
  const names = (ids: string[]) => ids.map(label).join(", ");

  const run = async (key: string, work: () => Promise<void>) => {
    setBusy(key);
    try {
      await work();
    } catch (err) {
      toast(String(err), "error");
    } finally {
      setBusy(null);
      void reload();
    }
  };
  const install = (repo: string, skill: RepoSkill, update: boolean, into: string[]) =>
    run(skill.name, async () => {
      if (into.length === 0) throw new Error("Pick at least one agent under “Install for”.");
      await installSkill(repo, skill.path, into);
      toast(update ? `Updated ${skill.name}` : `Installed ${skill.name} for ${names(into)}`, "success");
    });
  const update = (skill: InstalledSkill) => {
    const origin = skillOrigin(skill)!;
    const latest = listings.get(origin.repo)?.skills.find((s) => s.path === origin.path);
    // Every copy that came from the repo is replaced.
    if (latest) void install(origin.repo, latest, true, homeSkillCopies(skill).filter((c) => c.origin).map((c) => c.target));
  };
  const remove = (skill: InstalledSkill) =>
    run(skill.name, async () => {
      if (!(await ask(`Remove “${skill.name}” from ${skill.copies.map(copyLabel).join(", ")}?`, { title: "Remove skill", kind: "warning", okLabel: "Remove" }))) return;
      await uninstallSkill(skill.name, skill.copies.map((c) => c.dir), projects);
    });
  const copyToSelected = (skill: InstalledSkill, missing: string[]) =>
    run(skill.name, async () => {
      const from = homeSkillCopies(skill)[0] ?? skill.copies.find((c) => c.kind === "skill");
      if (!from) throw new Error(`${skill.name} is a command, not a skill folder`);
      await copySkill(skill.name, from.dir, missing, projects);
      toast(`Added ${skill.name} to ${names(missing)}`, "success");
    });
  const addSource = () => {
    const repo = normalizeRepo(newRepo);
    if (!repo) return toast("Enter a GitHub repo as owner/repo or its URL", "error");
    if (!FEATURED_SOURCES.some((f) => f.repo.toLowerCase() === repo.toLowerCase()) && !sources.includes(repo)) {
      const next = [...sources, repo];
      setSources(next);
      saveCustomSources(next);
    }
    setNewRepo("");
    setSource(repo);
  };
  const removeSource = (repo: string) => {
    const next = sources.filter((r) => r !== repo);
    setSources(next);
    saveCustomSources(next);
    if (source === repo) setSource(FEATURED_SOURCES[0].repo);
  };

  const byName = new Map((installed ?? []).map((s) => [s.name, s]));
  const shownInstalled = (installed ?? []).filter((s) => matchesSearch(query, [s.name, s.description, skillOrigin(s)?.repo]));
  const listing = listings.get(source);
  const shownRepo = (listing?.skills ?? []).filter((s) => matchesSearch(query, [s.name, s.description, s.path]));
  const featured = FEATURED_SOURCES.find((f) => f.repo === source);
  const outdatedSkills = (installed ?? []).filter((s) => {
    const origin = skillOrigin(s);
    return origin && isOutdated(s, listings.get(origin.repo));
  });
  const updateAll = () =>
    run("*", async () => {
      for (const skill of outdatedSkills) {
        const origin = skillOrigin(skill)!;
        const latest = listings.get(origin.repo)?.skills.find((s) => s.path === origin.path);
        if (latest) await installSkill(origin.repo, latest.path, homeSkillCopies(skill).filter((c) => c.origin).map((c) => c.target));
      }
      toast(`Updated ${outdatedSkills.length} skill${outdatedSkills.length === 1 ? "" : "s"}`, "success");
    });

  // Where new installs go; shown on both tabs since "Add" on an installed skill uses it too.
  const installFor = (
    <div className="flex items-start gap-3 rounded-lg border border-border bg-panel px-3.5 py-2.5">
      <span className="mt-1 shrink-0 text-xs text-fg-muted">Install for</span>
      <div className="min-w-0 flex-1">
        <div className="flex flex-wrap gap-1.5">
          {targets.map((t) => (
            <Chip key={t.id} active={selected.includes(t.id)} onClick={() => toggleTarget(t.id)} title={t.dir + (t.detected ? "" : " (agent not found on this machine)")}>
              {t.label}
              {!t.detected && <span className="text-fg-subtle">·&nbsp;not found</span>}
            </Chip>
          ))}
        </div>
        <p className={cn("mt-1.5 text-[11px] leading-relaxed", selected.length === 0 ? "text-danger" : "text-fg-subtle")}>
          {selected.length === 0 ? "Pick at least one agent." : "Codex, OpenCode, Cursor and other agents also read the shared .agents folder."}
        </p>
      </div>
    </div>
  );

  return (
    <div className="space-y-5">
      <StoreToolbar
        tab={tab}
        onTab={setTab}
        counts={{ discover: listing ? listing.skills.length : null, installed: installed ? installed.length : null }}
        updates={outdatedSkills.length > 0}
        query={query}
        onQuery={setQuery}
        placeholder={tab === "discover" ? `Search ${featured?.label ?? source}` : "Search installed skills"}
      />

      {tab === "discover" ? (
        <>
          <section className="space-y-2.5">
            <div className="flex flex-wrap items-center gap-1.5">
              {FEATURED_SOURCES.map((f) => (
                <Chip key={f.repo} active={source === f.repo} onClick={() => setSource(f.repo)} title={f.repo}>
                  {f.label}
                </Chip>
              ))}
              {sources.map((repo) => (
                <span key={repo} className="group relative">
                  <Chip active={source === repo} onClick={() => setSource(repo)}>
                    {repo}
                  </Chip>
                  <button onClick={() => removeSource(repo)} aria-label={`Remove ${repo}`} className="absolute -top-1.5 -right-1.5 hidden rounded-full bg-panel-raised p-0.5 text-fg-subtle ring-1 ring-border-strong group-hover:block hover:text-danger">
                    <X size={9} />
                  </button>
                </span>
              ))}
              <form
                className="flex items-center gap-1"
                onSubmit={(e) => {
                  e.preventDefault();
                  addSource();
                }}
              >
                <input value={newRepo} onChange={(e) => setNewRepo(e.target.value)} placeholder="Add owner/repo" aria-label="GitHub repo" className={cn(inputClass, "h-6 w-36")} />
                <button type="submit" disabled={!newRepo.trim()} aria-label="Add repo" className="rounded p-1 text-fg-subtle hover:bg-hover hover:text-fg disabled:opacity-40">
                  <Plus size={13} />
                </button>
              </form>
            </div>
            <div className="flex items-center gap-2 px-1">
              <p className="min-w-0 flex-1 truncate text-xs text-fg-muted" title={featured?.description}>
                <span className="font-mono text-fg-subtle">{source}</span>
                {featured && <> · {featured.description}</>}
              </p>
              <button onClick={() => void loadListing(source, true)} aria-label="Refresh" title="Check the repo again" className={cn(iconButtonClass, "size-6")}>
                <RefreshCw size={12} />
              </button>
            </div>
          </section>

          {installFor}

          {listingError ? (
            <EmptyState icon={CloudOff} title={`Couldn't load ${source}`} hint={listingError}>
              <button onClick={() => void loadListing(source, true)} className={buttonClass}>
                <RefreshCw size={12} />
                Retry
              </button>
            </EmptyState>
          ) : !listing ? (
            <SkeletonGrid />
          ) : shownRepo.length === 0 ? (
            listing.skills.length === 0 ? (
              <EmptyState icon={Wand2} title={`${source} has no skills`} hint="No folder in it has a SKILL.md." />
            ) : (
              <EmptyState icon={Search} title={`No skill in ${source} matches “${query}”`}>
                <button onClick={() => setQuery("")} className={buttonClass}>
                  Clear search
                </button>
              </EmptyState>
            )
          ) : (
            <div className="grid grid-cols-2 gap-3">
              {shownRepo.map((skill) => {
                const have = byName.get(skill.name);
                const origin = have && skillOrigin(have);
                const sameSkill = origin && origin.repo.toLowerCase() === listing.repo.toLowerCase() && origin.path === skill.path;
                const outdated = have && sameSkill && isOutdated(have, listing);
                const everywhere = have && selected.every((t) => homeSkillCopies(have).some((c) => c.target === t));
                const open = expanded === skill.path;
                return (
                  <article key={skill.path} className="flex flex-col rounded-lg border border-border bg-panel p-3.5 transition-colors hover:border-border-strong">
                    <div className="flex items-start gap-3">
                      <PluginTile id={skill.name} name={skill.name} />
                      <div className="min-w-0 flex-1">
                        <h4 className="truncate font-mono text-[12px] font-medium text-fg" title={skill.name}>
                          {skill.name}
                        </h4>
                        <p className="truncate text-[11px] text-fg-subtle" title={skill.path}>
                          {skill.path} · {skill.files} file{skill.files === 1 ? "" : "s"}
                        </p>
                      </div>
                    </div>
                    <button
                      onClick={() => setExpanded(open ? null : skill.path)}
                      title={open ? "Show less" : "Show more"}
                      className={cn("mt-2.5 text-left text-xs leading-relaxed text-fg-muted hover:text-fg", !open && "line-clamp-3")}
                    >
                      {skill.description || "No description."}
                    </button>
                    {have && !sameSkill && (
                      <p className="mt-2 flex items-start gap-1.5 rounded-md bg-warning/10 px-2 py-1.5 text-[11px] leading-relaxed text-warning">
                        <TriangleAlert size={12} className="mt-px shrink-0" />
                        <span>A different “{skill.name}” is installed; installing replaces it.</span>
                      </p>
                    )}
                    <div className="mt-auto flex items-center justify-end gap-2 pt-3">
                      {have && sameSkill && !outdated && (
                        <span className="mr-auto flex min-w-0 items-center gap-1 truncate text-[11px] text-success">
                          <Check size={12} className="shrink-0" />
                          {everywhere ? "Installed" : `For ${names(homeSkillCopies(have).map((c) => c.target))}`}
                        </span>
                      )}
                      {!(have && everywhere && !outdated) && (
                        <button
                          disabled={!!busy || selected.length === 0}
                          onClick={() => void install(listing.repo, skill, !!outdated, outdated ? [...new Set([...homeSkillCopies(have!).map((c) => c.target), ...selected])] : selected)}
                          title={selected.length ? `Install for ${names(selected)}` : "Pick an agent under “Install for”"}
                          className={primaryButtonClass}
                        >
                          {busy === skill.name ? <Spinner size={12} /> : <Download size={12} />}
                          {outdated ? "Update" : have && sameSkill ? "Install for more" : "Install"}
                        </button>
                      )}
                    </div>
                  </article>
                );
              })}
            </div>
          )}
          <p className="px-1 text-[11px] leading-relaxed text-fg-subtle">
            Every folder with a SKILL.md in a GitHub repo is a skill. Skills are instructions and scripts your agents follow, so install them from repos you trust.
          </p>
        </>
      ) : (
        <>
          {outdatedSkills.length > 0 && !query && (
            <UpdateBanner
              text={outdatedSkills.length === 1 ? `An update for ${outdatedSkills[0].name} is available.` : `${outdatedSkills.length} skill updates are available.`}
              label={outdatedSkills.length === 1 ? "Update" : "Update all"}
              busy={busy === "*"}
              disabled={!!busy}
              onClick={() => (outdatedSkills.length === 1 ? update(outdatedSkills[0]) : void updateAll())}
            />
          )}

          {installed === null ? (
            <div className="space-y-2">
              {[0, 1, 2].map((i) => (
                <div key={i} className="h-16 animate-pulse rounded-lg border border-border bg-panel" />
              ))}
            </div>
          ) : shownInstalled.length === 0 ? (
            installed.length === 0 ? (
              <EmptyState icon={Wand2} title="No skills installed" hint="Skills are folders with a SKILL.md that agents load from their skills folder, in your home folder or a project's.">
                <button onClick={() => setTab("discover")} className={primaryButtonClass}>
                  <Store size={12} />
                  Browse skills
                </button>
              </EmptyState>
            ) : (
              <EmptyState icon={Search} title={`No installed skill matches “${query}”`} />
            )
          ) : (
            <div className="divide-y divide-border overflow-hidden rounded-lg border border-border bg-panel">
              {shownInstalled.map((skill) => {
                const origin = skillOrigin(skill);
                const outdated = isOutdated(skill, origin ? listings.get(origin.repo) : undefined);
                const canCopy = skill.copies.some((c) => c.kind === "skill");
                const missing = canCopy ? selected.filter((t) => !homeSkillCopies(skill).some((c) => c.target === t)) : [];
                return (
                  <div key={skill.name} className="group flex gap-3 px-3.5 py-3">
                    <PluginTile id={skill.name} name={skill.name} />
                    <div className="min-w-0 flex-1">
                      <div className="flex min-w-0 flex-wrap items-center gap-1">
                        <h4 className="mr-0.5 truncate font-mono text-[12px] font-medium text-fg">{skill.name}</h4>
                        {outdated && <Pill tone="accent">update available</Pill>}
                      </div>
                      {skill.description && (
                        <p className="mt-0.5 line-clamp-2 text-xs leading-relaxed text-fg-muted" title={skill.description}>
                          {skill.description}
                        </p>
                      )}
                      <div className="mt-1.5 flex flex-wrap items-center gap-1">
                        {skill.copies.map((c) => (
                          <Pill key={c.dir} title={c.dir + (c.link ? " (link)" : "")}>
                            {copyLabel(c)}
                            {c.link && " ↗"}
                          </Pill>
                        ))}
                        <span className="ml-1 truncate text-[11px] text-fg-subtle">{origin ? `from ${origin.repo}` : skill.copies.every((c) => c.project) ? "in the project" : "added by hand"}</span>
                      </div>
                      {(outdated || missing.length > 0) && (
                        <div className="mt-2.5 flex flex-wrap gap-1.5">
                          {outdated && (
                            <button disabled={!!busy} onClick={() => update(skill)} className={primaryButtonClass}>
                              <Download size={12} />
                              Update
                            </button>
                          )}
                          {missing.length > 0 && (
                            <button disabled={!!busy} onClick={() => void copyToSelected(skill, missing)} title={`Also install for ${names(missing)}`} className={buttonClass}>
                              <Copy size={12} />
                              Add for {names(missing)}
                            </button>
                          )}
                        </div>
                      )}
                    </div>
                    <div className="flex shrink-0 items-start gap-1">
                      {busy === skill.name && <Spinner size={13} className="mt-1.5 text-fg-subtle" />}
                      <button onClick={() => void revealItemInDir(copyFile(skill.copies[0])).catch(() => {})} aria-label={`Show ${skill.name} in Finder`} title="Show the folder" className={iconButtonClass}>
                        <FolderOpen size={13} />
                      </button>
                      <button
                        disabled={!!busy}
                        onClick={() => void remove(skill)}
                        aria-label={`Remove ${skill.name}`}
                        title="Remove"
                        className={cn(iconButtonClass, "opacity-0 group-hover:opacity-100 hover:text-danger focus-visible:opacity-100")}
                      >
                        <Trash2 size={13} />
                      </button>
                    </div>
                  </div>
                );
              })}
            </div>
          )}

          {installFor}
        </>
      )}
    </div>
  );
}

export function SettingsModal() {
  const { settingsOpen, settingsPage, setSettings } = useWorkspacesPick("settingsOpen", "settingsPage", "setSettings");
  const page = PAGES.find((p) => p.id === settingsPage) ?? PAGES[0];

  if (!settingsOpen) return null;
  const open = (id: string) => setSettings(true, id);
  return (
    <div className="anim-backdrop fixed inset-0 z-50 flex items-center justify-center bg-black/40" onMouseDown={() => setSettings(false)}>
      <div
        className="anim-dialog flex h-[min(620px,85vh)] w-[min(820px,92vw)] overflow-hidden rounded-xl border border-border-strong bg-panel-raised shadow-2xl"
        onMouseDown={(e) => e.stopPropagation()}
      >
        <nav className="flex w-48 shrink-0 flex-col gap-0.5 border-r border-border bg-panel p-2">
          <h1 className="px-2.5 pt-2 pb-3 text-sm font-semibold text-fg">Settings</h1>
          {PAGES.map((p) => (
            <button
              key={p.id}
              role="tab"
              aria-selected={p.id === page.id}
              onClick={() => open(p.id)}
              className={cn(
                "flex h-8 items-center gap-2.5 rounded-md px-2.5 text-[13px] transition-colors",
                p.id === page.id ? "bg-active text-fg" : "text-fg-muted hover:bg-hover hover:text-fg",
              )}
            >
              <p.icon size={14} className={p.id === page.id ? "text-accent" : undefined} />
              {p.label}
            </button>
          ))}
          <UpdateFooter />
        </nav>
        <div className="flex min-w-0 flex-1 flex-col">
          <header className="flex h-12 shrink-0 items-center border-b border-border px-6">
            <h2 className="flex-1 text-sm font-medium text-fg">{page.label}</h2>
            <button onClick={() => setSettings(false)} aria-label="Close settings" className="rounded p-1 text-fg-subtle hover:bg-hover hover:text-fg">
              <X size={14} />
            </button>
          </header>
          <div key={page.id} className="anim-fade flex-1 space-y-6 overflow-y-auto px-6 py-5">
            {page.render()}
          </div>
        </div>
      </div>
    </div>
  );
}
