import { invoke } from "@tauri-apps/api/core";
import { Bot, Check, ChevronDown, Download, FolderOpen, Keyboard, Loader2, Mic, Moon, Puzzle, RefreshCw, SlidersHorizontal, Sparkles, Sun, X, type LucideIcon } from "lucide-react";
import { useEffect, useState } from "react";
import { DEFAULT_TERMINAL_FONT_SIZE, defaultProviderId, MAX_TERMINAL_FONT_SIZE, MIN_TERMINAL_FONT_SIZE, useWorkspaces } from "@/stores/workspaces";
import { IS_MAC, keysFor, voiceKeys } from "@/commands/keymap";
import { configureVoice, refreshVoiceStatus, useVoice } from "@/features/voice/voice";
import { availableLlms, LLM_PROVIDERS, llmProvider } from "@/features/llm/llm";
import { ask } from "@tauri-apps/plugin-dialog";
import { revealItemInDir } from "@tauri-apps/plugin-opener";
import { loadPlugins, pluginsDir, setPluginEnabled, usePlugins, type PluginInfo } from "@/features/plugins/plugins";
import { cancelChatGptSignIn, markDecisionsUnavailable, signInWithChatGpt, signOutOfChatGpt, useChatGpt } from "@/features/llm/chatgpt";
import { cn } from "@/lib/cn";

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

function AppearanceSettings() {
  const { theme, toggleTheme } = useWorkspaces();
  return (
    <Card title="Appearance">
      <Row label="Theme">
        <div className="flex rounded-md border border-border-strong bg-panel-raised p-0.5">
          {(["dark", "light"] as const).map((t) => {
            const Icon = t === "dark" ? Moon : Sun;
            return (
              <button
                key={t}
                onClick={() => theme !== t && toggleTheme()}
                className={cn(
                  "flex h-6 items-center gap-1.5 rounded px-2.5 text-xs capitalize transition-colors",
                  theme === t ? "bg-active text-fg shadow-sm" : "text-fg-subtle hover:text-fg",
                )}
              >
                <Icon size={12} />
                {t}
              </button>
            );
          })}
        </div>
      </Row>
    </Card>
  );
}

/** Text size of every terminal. */
function TerminalSettings() {
  const { terminalFontSize, setTerminalFontSize } = useWorkspaces();
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

function ShortcutSettings() {
  return (
    <Card title="Keyboard">
      <Row label="Keyboard shortcuts" hint="See every shortcut and change any of them, including the push-to-talk keys.">
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
  const { providers, defaultProvider, setDefaultProvider, models, agentModels, setAgentModel, loadModels } = useWorkspaces();
  const current = defaultProviderId({ providers, defaultProvider });
  const agents = providers.filter((p) => p.installed && p.modelArg);
  useEffect(() => agents.forEach((p) => void loadModels(p.id)), [agents.length]); // eslint-disable-line react-hooks/exhaustive-deps
  return (
    <>
      <Card title="New panes">
        <Row label="Default agent" hint={`${keysFor({ type: "pane.spawn", provider: "default" })}, split and “open a new window” start this agent.`}>
          <Select value={current} onChange={(e) => setDefaultProvider(e.target.value)} className="w-44">
            {providers.map((p) => (
              <option key={p.id} value={p.id} disabled={!p.installed}>
                {p.name}
                {p.installed ? "" : " (not installed)"}
              </option>
            ))}
          </Select>
        </Row>
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
    </>
  );
}

/** What the review inbox does on its own when an agent finishes. */
function ReviewSettings() {
  const { reviewConfig, setReviewConfig } = useWorkspaces();
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

/** How often the LLM re-estimates working agents' progress. */
function ProgressSettings() {
  const { progressConfig, setProgressConfig } = useWorkspaces();
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

function Microphone({ selected }: { selected: string | null }) {
  const level = useVoice((s) => s.level);
  const [devices, setDevices] = useState<{ devices: string[]; default: string | null } | null>(null);
  const [testing, setTesting] = useState(false);
  const [result, setResult] = useState<{ ok: boolean; text: string } | null>(null);

  const load = () => void invoke<{ devices: string[]; default: string | null }>("voice_input_devices").then(setDevices, () => {});
  useEffect(load, []);

  const test = async () => {
    setTesting(true);
    setResult(null);
    try {
      const r = await invoke<{ device: string; peak: number }>("voice_test_microphone");
      setResult(r.peak < 0.005 ? { ok: false, text: `Very quiet on “${r.device}”. Speak up or pick another microphone.` } : { ok: true, text: `Heard you on “${r.device}”.` });
    } catch (err) {
      setResult({ ok: false, text: String(err) });
    } finally {
      setTesting(false);
    }
  };

  // A saved device that isn't connected right now still shows, so the choice isn't lost.
  const names = devices ? [...devices.devices, ...(selected && !devices.devices.includes(selected) ? [selected] : [])] : selected ? [selected] : [];
  return (
    <Row
      label="Microphone"
      hint={
        testing ? (
          <span className="mt-1.5 block h-1.5 w-40 overflow-hidden rounded-full bg-hover">
            <span className="block h-full bg-success transition-[width] duration-75" style={{ width: `${Math.min(100, level * 1200)}%` }} />
          </span>
        ) : (
          result && <span className={result.ok ? "text-success" : "text-danger"}>{result.text}</span>
        )
      }
    >
      <Select
        value={selected ?? ""}
        onFocus={load}
        onChange={(e) => {
          setResult(null);
          void configureVoice({ inputDevice: e.target.value });
        }}
        className="w-56"
      >
        <option value="">System default{devices?.default ? ` (${devices.default})` : ""}</option>
        {names.map((name) => (
          <option key={name} value={name}>
            {name}
            {devices && !devices.devices.includes(name) ? " — not connected" : ""}
          </option>
        ))}
      </Select>
      <button disabled={testing} onClick={() => void test()} className={cn(buttonClass, "w-24 justify-center")}>
        {testing && <Loader2 size={12} className="animate-spin" />}
        {testing ? "Listening…" : "Test"}
      </button>
    </Row>
  );
}

function VoiceSettings() {
  const status = useVoice((s) => s.status);
  const readback = useWorkspaces((s) => s.voiceConfig.readback ?? false);
  const confirm = useWorkspaces((s) => s.voiceConfig.confirm ?? false);
  const shortcuts = useWorkspaces((s) => s.shortcuts);
  const [busy, setBusy] = useState<string | null>(null);
  const [result, setResult] = useState<{ ok: boolean; text: string } | null>(null);

  useEffect(() => {
    void refreshVoiceStatus();
  }, []);

  if (!status)
    return (
      <div className="flex items-center gap-1.5 text-xs text-fg-subtle">
        <Loader2 size={12} className="animate-spin" /> Loading…
      </div>
    );

  const download = async (id: string) => {
    setBusy(id);
    setResult(null);
    try {
      await invoke("voice_download_model", { id });
    } catch (err) {
      setResult({ ok: false, text: String(err) });
    } finally {
      setBusy(null);
      void refreshVoiceStatus();
    }
  };

  const keys = voiceKeys(shortcuts);
  return (
    <>
      <div className="flex gap-3 rounded-lg border border-border bg-panel px-3.5 py-3 text-xs leading-relaxed text-fg-muted">
        <Mic size={14} className="mt-0.5 shrink-0 text-accent" />
        <p>
          Hold <kbd className="rounded border border-border-strong bg-panel-raised px-1 font-mono text-[11px] text-fg">{keys.command}</kbd> to give a
          command, <kbd className="rounded border border-border-strong bg-panel-raised px-1 font-mono text-[11px] text-fg">{keys.dictation}</kbd> to
          dictate into the focused pane. Speech is recognized on this Mac; audio never leaves your machine.{" "}
          <button onClick={() => useWorkspaces.getState().setCheatSheet(true)} className="text-accent hover:underline">
            Change keys
          </button>
        </p>
      </div>
      {status.shortcutError && <p className="px-1 text-xs text-warning">{status.shortcutError} Use the mic button in the title bar instead.</p>}

      <Card title="Input">
        <Row
          label="Language"
          hint={
            status.language !== "en" &&
            status.sttModel.endsWith(".en") && (
              <span className="text-warning">Pick a multilingual model below for {status.language === "de" ? "German" : "other languages"}.</span>
            )
          }
        >
          <Select value={status.language} onChange={(e) => void configureVoice({ language: e.target.value as "en" | "de" | "auto" })} className="w-36">
            <option value="en">English</option>
            <option value="de">Deutsch</option>
            <option value="auto">Auto-detect</option>
          </Select>
        </Row>
        <Microphone selected={status.inputDevice} />
      </Card>

      <Card title="Speech model" description="Larger models are more accurate and a bit slower.">
        {status.models.map((m) => {
          const selected = status.sttModel === m.id;
          return (
            <div
              key={m.id}
              role={m.installed ? "button" : undefined}
              onClick={() => m.installed && !selected && void configureVoice({ sttModel: m.id })}
              className={cn("flex items-center gap-3 px-3.5 py-2.5 transition-colors", m.installed && !selected && "hover:bg-hover")}
            >
              <span
                className={cn(
                  "grid size-4 shrink-0 place-items-center rounded-full border",
                  selected ? "border-accent bg-accent" : "border-border-strong",
                  !m.installed && "opacity-40",
                )}
              >
                {selected && <span className="size-1.5 rounded-full bg-white" />}
              </span>
              <span className={cn("flex-1 text-[13px]", m.installed ? "text-fg" : "text-fg-muted")}>{m.label}</span>
              {m.installed ? (
                <span className="flex items-center gap-1 text-xs text-fg-subtle">
                  <Check size={12} className="text-success" /> Installed
                </span>
              ) : (
                <button
                  disabled={busy !== null}
                  onClick={(e) => {
                    e.stopPropagation();
                    void download(m.id);
                  }}
                  className={buttonClass}
                >
                  {busy === m.id ? <Loader2 size={12} className="animate-spin" /> : <Download size={12} />}
                  {busy === m.id ? "Downloading…" : "Download"}
                </button>
              )}
            </div>
          );
        })}
      </Card>
      <Result result={result} />

      <Card title="Behavior">
        <Row label="Speak agent updates aloud" hint="“Codex needs your input”, “Claude Code finished”.">
          <Toggle label="Speak agent updates aloud" checked={readback} onChange={(readback) => useWorkspaces.getState().setVoiceConfig({ readback })} />
        </Row>
        <Row label="Confirm uncertain commands" hint="Ask before running voice commands that are unclear or risky.">
          <Toggle label="Confirm uncertain commands" checked={confirm} onChange={(confirm) => useWorkspaces.getState().setVoiceConfig({ confirm })} />
        </Row>
      </Card>
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
  const planner = useWorkspaces((s) => s.voiceConfig.planner ?? "auto");
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
      void refreshVoiceStatus();
    }
  };

  const testConnection = async () => {
    try {
      return `Connected in ${await invoke<number>("voice_test_connection")} ms`;
    } catch (err) {
      if (!String(err).includes("decisions-unavailable")) throw err;
      markDecisionsUnavailable();
    }
    // The plan can't use the Decisions API; voice commands go to the LLM, so check that.
    const started = performance.now();
    await invoke("llm_compose", { req: { provider: "chatgpt", model: "", system: "Reply with OK.", user: "ping" } });
    return `Connected in ${Math.round(performance.now() - started)} ms. Your plan can't use the Decisions API, so voice commands are planned by the LLM.`;
  };

  return (
    <>
      <Card
        title="ChatGPT account"
        description={
          <>
            Runs voice commands and the prompt writer on your ChatGPT plan (Plus or Pro); no API key needed. Voice commands send the
            transcript and a short summary of your panes, never terminal output, file paths or environment variables. Set a weekly limit or
            disconnect winfer in ChatGPT's settings.
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
              {busy === "test" && <Loader2 size={12} className="animate-spin" />}
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
              className="flex h-7 items-center gap-1.5 rounded-md bg-accent px-3 text-xs font-medium text-white hover:opacity-90 disabled:opacity-60"
            >
              {busy === "signin" && <Loader2 size={12} className="animate-spin" />}
              Continue with ChatGPT
            </button>
          </Row>
        )}
      </Card>
      {chatgpt.decisionsUnavailable && <p className="px-1 text-xs text-fg-subtle">Your plan can't use the Decisions API; every voice command is planned by the LLM below.</p>}
      <Result result={result} />

      <Card
        title="LLM"
        description="Plans long or complex commands (“we need four features: …”) and turns what you say into a clear prompt for the agent, using its recent output as context. Short commands go to the Decisions API (GPT-6 Luna); “Type … into Claude” stays word for word. Also used for progress estimates and review summaries."
      >
        <Row
          label="Provider"
          hint={writer === "claude" ? "Runs Claude Code headless with your Claude login, without tools, MCP servers or your settings." : undefined}
        >
          <Select
            value={writer}
            onChange={(e) => useWorkspaces.getState().setVoiceConfig({ writer: e.target.value as "off" | "chatgpt" | "claude", writerModel: "" })}
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
            <option value="off">Off (send my words as spoken)</option>
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
        {writer === "claude" && (
          <Row label="Model">
            <input
              key={writer}
              defaultValue={writerModel}
              onBlur={(e) => useWorkspaces.getState().setVoiceConfig({ writer, writerModel: e.target.value.trim() })}
              placeholder="Default, e.g. sonnet"
              className={cn(inputClass, "w-56")}
            />
          </Row>
        )}
        {writer !== "off" && (
          <Row label="Who decides what runs" stacked>
            <Select value={planner} onChange={(e) => useWorkspaces.getState().setVoiceConfig({ planner: e.target.value as "auto" | "always" | "off" })} className="w-full">
              <option value="auto">LLM for long or complex commands, Decisions API for short ones</option>
              <option value="always">LLM for every command (slower)</option>
              <option value="off">Decisions API for every command</option>
            </Select>
          </Row>
        )}
      </Card>
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
        <AppearanceSettings />
        <TerminalSettings />
        <ShortcutSettings />
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
  { id: "voice", label: "Voice", icon: Mic, render: () => <VoiceSettings /> },
  { id: "ai", label: "AI", icon: Sparkles, render: () => <AiSettings /> },
  { id: "plugins", label: "Plugins", icon: Puzzle, render: () => <PluginSettings /> },
];

// The page last looked at, so reopening settings lands where you left off.
let lastPage = PAGES[0].id;

/** What a plugin adds, e.g. "2 tools · 1 agent · code". */
function contributions(plugin: PluginInfo): string {
  const m = plugin.manifest;
  if (!m) return "";
  const count = (n: number, one: string) => (n ? [`${n} ${one}${n === 1 ? "" : "s"}`] : []);
  return [...count(m.tools.length, "tool"), ...count(m.providers.length, "agent"), ...count(m.recipes.length, "recipe"), ...(m.main ? ["code"] : [])].join(" · ");
}

async function togglePlugin(plugin: PluginInfo, enabled: boolean) {
  // Plugin code and agent commands run with winfer's access to files, terminals and the network.
  if (enabled && (plugin.manifest?.main || plugin.manifest?.providers.length)) {
    const ok = await ask(`“${plugin.manifest.name}” runs its own code with full access to your files, terminals and network. Only turn on plugins you trust.`, {
      title: "Turn on plugin",
      kind: "warning",
      okLabel: "Turn on",
    });
    if (!ok) return;
  }
  await setPluginEnabled(plugin.id, enabled);
}

/** Installed plugins: turn each on or off, open the folder, reload after editing. */
function PluginSettings() {
  const { plugins, runtimeErrors } = usePlugins();
  return (
    <>
      <Card
        title="Installed plugins"
        description="Plugins add agents, recipes, commands and voice actions. Each one starts off; plugins with code ask before they run."
      >
        {plugins.length === 0 ? (
          <Row label="No plugins yet" hint="Put a folder with a plugin.json in the plugins folder, then reload." />
        ) : (
          plugins.map((p) => {
            const error = p.error ?? (p.enabled ? runtimeErrors[p.id] : undefined);
            const name = p.manifest?.name ?? p.id;
            return (
              <Row
                key={p.id}
                label={
                  <>
                    {name}
                    {p.manifest?.version && <span className="ml-1.5 font-mono text-[11px] text-fg-subtle">{p.manifest.version}</span>}
                  </>
                }
                hint={
                  <>
                    {p.manifest?.description && <span className="block">{p.manifest.description}</span>}
                    {contributions(p) && <span className="block">{contributions(p)}</span>}
                    {error && <span className="block text-danger">{error}</span>}
                  </>
                }
              >
                {!p.error && <Toggle label={`Turn on ${name}`} checked={p.enabled} onChange={(enabled) => void togglePlugin(p, enabled)} />}
              </Row>
            );
          })
        )}
      </Card>
      <Card title="Plugins folder">
        <Row label="Add or edit plugins" hint="One folder per plugin. Reload after changing one.">
          <button onClick={() => void pluginsDir().then((dir) => revealItemInDir(dir)).catch(() => {})} className={buttonClass}>
            <FolderOpen size={12} />
            Open folder
          </button>
          <button onClick={() => void loadPlugins()} className={buttonClass}>
            <RefreshCw size={12} />
            Reload
          </button>
        </Row>
      </Card>
    </>
  );
}

export function SettingsModal() {
  const { settingsOpen, setSettings } = useWorkspaces();
  const [pageId, setPageId] = useState(lastPage);
  const page = PAGES.find((p) => p.id === pageId) ?? PAGES[0];

  useEffect(() => {
    if (!settingsOpen) return;
    const onKey = (e: KeyboardEvent) => e.key === "Escape" && setSettings(false);
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [settingsOpen, setSettings]);

  if (!settingsOpen) return null;
  const open = (id: string) => {
    lastPage = id;
    setPageId(id);
  };
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
