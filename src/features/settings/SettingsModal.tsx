import { invoke } from "@tauri-apps/api/core";
import { Check, Download, Loader2, X } from "lucide-react";
import { useEffect, useState } from "react";
import { DEFAULT_TERMINAL_FONT_SIZE, defaultProviderId, MAX_TERMINAL_FONT_SIZE, MIN_TERMINAL_FONT_SIZE, useWorkspaces } from "@/stores/workspaces";
import { IS_MAC, keysFor, voiceKeys } from "@/commands/keymap";
import { configureVoice, refreshVoiceStatus, useVoice } from "@/features/voice/voice";
import { availableLlms, LLM_PROVIDERS, llmProvider } from "@/features/llm/llm";
import { checkForUpdates, installUpdate, useUpdates } from "@/features/updates/updater";
import { cancelChatGptSignIn, markDecisionsUnavailable, signInWithChatGpt, signOutOfChatGpt, useChatGpt } from "@/features/llm/chatgpt";

const Section = ({ title, children }: { title: string; children: React.ReactNode }) => (
  <section className="space-y-3">
    <h2 className="text-xs font-medium tracking-wide text-fg-subtle uppercase">{title}</h2>
    {children}
  </section>
);

const inputClass = "h-8 w-full rounded-md border border-border bg-panel px-2.5 font-mono text-xs text-fg outline-none focus:border-accent";

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
    <div className="space-y-1.5">
      <label className="flex items-center gap-2 text-[13px] text-fg">
        Microphone
        <select
          value={selected ?? ""}
          onFocus={load}
          onChange={(e) => {
            setResult(null);
            void configureVoice({ inputDevice: e.target.value });
          }}
          className="h-7 max-w-72 rounded-md border border-border bg-panel px-2 text-xs text-fg"
        >
          <option value="">System default{devices?.default ? ` (${devices.default})` : ""}</option>
          {names.map((name) => (
            <option key={name} value={name}>
              {name}
              {devices && !devices.devices.includes(name) ? " — not connected" : ""}
            </option>
          ))}
        </select>
        <button
          disabled={testing}
          onClick={() => void test()}
          className="flex items-center gap-1.5 rounded-md border border-border-strong px-2 py-1 text-xs hover:bg-hover disabled:opacity-50"
        >
          {testing && <Loader2 size={12} className="animate-spin" />}
          {testing ? "Say something…" : "Test"}
        </button>
        {testing && (
          <span className="h-1.5 w-24 overflow-hidden rounded-full bg-hover">
            <span className="block h-full bg-success transition-[width] duration-75" style={{ width: `${Math.min(100, level * 1200)}%` }} />
          </span>
        )}
      </label>
      {result && <p className={`anim-fade text-xs ${result.ok ? "text-success" : "text-danger"}`}>{result.text}</p>}
    </div>
  );
}

function VoiceSettings() {
  const status = useVoice((s) => s.status);
  const readback = useWorkspaces((s) => s.voiceConfig.readback ?? false);
  const confirm = useWorkspaces((s) => s.voiceConfig.confirm ?? false);
  const chatgpt = useChatGpt();
  // What actually runs: the choice, or what it falls back to. Recomputed when the sign-in changes.
  const writer = useWorkspaces(() => llmProvider() ?? "off");
  const available = useWorkspaces(() => availableLlms().join(","));
  const writerModel = useWorkspaces((s) => s.voiceConfig.writerModel ?? "");
  const planner = useWorkspaces((s) => s.voiceConfig.planner ?? "auto");
  const shortcuts = useWorkspaces((s) => s.shortcuts);
  const [busy, setBusy] = useState<string | null>(null);
  const [result, setResult] = useState<{ ok: boolean; text: string } | null>(null);

  useEffect(() => {
    void refreshVoiceStatus();
  }, []);

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

  if (!status)
    return (
      <div className="flex items-center gap-1.5 text-xs text-fg-subtle">
        <Loader2 size={12} className="animate-spin" /> Loading…
      </div>
    );

  return (
    <>
      <Section title="Speech recognition (on this Mac)">
        <p className="text-xs text-fg-muted">
          Hold {voiceKeys(shortcuts).command} to give a command, {voiceKeys(shortcuts).dictation} to dictate into the focused pane (change them under{" "}
          <button onClick={() => useWorkspaces.getState().setCheatSheet(true)} className="text-accent hover:underline">
            Keyboard shortcuts
          </button>
          ). Audio never leaves your machine.
        </p>
        {status.shortcutError && <p className="text-xs text-warning">{status.shortcutError} Use the mic button in the title bar instead.</p>}
        <label className="flex items-center gap-2 text-[13px] text-fg">
          Language
          <select
            value={status.language}
            onChange={(e) => void configureVoice({ language: e.target.value as "en" | "de" | "auto" })}
            className="h-7 rounded-md border border-border bg-panel px-2 text-xs text-fg"
          >
            <option value="en">English</option>
            <option value="de">Deutsch</option>
            <option value="auto">Auto-detect</option>
          </select>
          {status.language !== "en" && status.sttModel.endsWith(".en") && (
            <span className="text-xs text-warning">Pick a multilingual model below for {status.language === "de" ? "German" : "other languages"}.</span>
          )}
        </label>
        <Microphone selected={status.inputDevice} />
        {status.models.map((m) => (
          <label key={m.id} className="flex items-center gap-3 text-[13px] text-fg">
            <input
              type="radio"
              name="stt"
              checked={status.sttModel === m.id}
              disabled={!m.installed}
              onChange={() => void configureVoice({ sttModel: m.id })}
            />
            <span className="flex-1">{m.label}</span>
            {m.installed ? (
              <span className="flex items-center gap-1 text-xs text-success">
                <Check size={12} /> Installed
              </span>
            ) : (
              <button
                disabled={busy !== null}
                onClick={() => void act(`dl-${m.id}`, () => invoke("voice_download_model", { id: m.id }))}
                className="flex items-center gap-1.5 rounded-md border border-border-strong px-2 py-1 text-xs hover:bg-hover disabled:opacity-50"
              >
                {busy === `dl-${m.id}` ? <Loader2 size={12} className="animate-spin" /> : <Download size={12} />}
                Download
              </button>
            )}
          </label>
        ))}
      </Section>

      <label className="flex items-center gap-2 text-[13px] text-fg">
        <input type="checkbox" checked={readback} onChange={(e) => useWorkspaces.getState().setVoiceConfig({ readback: e.target.checked })} />
        Speak agent updates aloud (“Codex needs your input”, “Claude Code finished”)
      </label>

      <label className="flex items-center gap-2 text-[13px] text-fg">
        <input type="checkbox" checked={confirm} onChange={(e) => useWorkspaces.getState().setVoiceConfig({ confirm: e.target.checked })} />
        Ask before running uncertain or risky voice commands
      </label>

      <Section title="ChatGPT account">
        <p className="text-xs text-fg-muted">
          Sign in with ChatGPT to run voice commands and the prompt writer on your ChatGPT plan (Plus or Pro); no API key needed. Voice
          commands send the transcript and a short summary of your panes; terminal output, file paths and environment variables aren't sent
          to pick an action. You can set a weekly limit for winfer or disconnect it in ChatGPT's settings.
        </p>
        <div className="flex items-center gap-2">
          {chatgpt.signedIn ? (
            <>
              <span className="flex items-center gap-1.5 text-[13px] text-fg">
                <Check size={13} className="text-success" />
                Signed in{chatgpt.email ? ` as ${chatgpt.email}` : ""}
              </span>
              <button
                disabled={busy !== null}
                onClick={() =>
                  void act("test", async () => {
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
                  })
                }
                className="flex items-center gap-1.5 rounded-md border border-border-strong px-2.5 py-1 text-xs text-fg hover:bg-hover disabled:opacity-40"
              >
                {busy === "test" && <Loader2 size={12} className="animate-spin" />}
                {busy === "test" ? "Testing…" : "Test connection"}
              </button>
              <button
                disabled={busy !== null}
                onClick={() => void act("signout", async () => (await signOutOfChatGpt(), "Signed out"))}
                className="rounded-md px-2.5 py-1 text-xs text-fg-subtle hover:bg-hover hover:text-danger"
              >
                Sign out
              </button>
            </>
          ) : (
            <button
              disabled={busy !== null}
              onClick={() => void act("signin", async () => (await signInWithChatGpt(), "Signed in"))}
              className="flex items-center gap-1.5 rounded-md bg-accent px-3 py-1.5 text-xs font-medium text-white hover:opacity-90 disabled:opacity-40"
            >
              {busy === "signin" && <Loader2 size={12} className="animate-spin" />}
              {busy === "signin" ? "Finish signing in in your browser…" : "Continue with ChatGPT"}
            </button>
          )}
          {busy === "signin" && (
            <button onClick={() => void cancelChatGptSignIn()} className="rounded-md px-2.5 py-1 text-xs text-fg-subtle hover:bg-hover hover:text-fg">
              Cancel
            </button>
          )}
        </div>
        {chatgpt.decisionsUnavailable && (
          <p className="text-[11px] text-fg-subtle">Your plan can't use the Decisions API; every voice command is planned by the LLM below.</p>
        )}
        {result && <p className={result.ok ? "anim-fade text-xs text-success" : "anim-fade text-xs text-danger"}>{result.text}</p>}
      </Section>

      <Section title="LLM (planner and prompt writer)">
        <p className="text-xs text-fg-muted">
          Plans long or complex commands (“we need four features: …”) with its full reasoning, and turns what you say into a clear prompt
          for the agent, using its recent output as context. Short commands go to the Decisions API (GPT-6 Luna). “Type … into Claude”
          stays word for word. Also used for progress estimates and review summaries.
        </p>
        <div className="flex items-center gap-2">
          <select
            value={writer}
            onChange={(e) => useWorkspaces.getState().setVoiceConfig({ writer: e.target.value as "off" | "chatgpt" | "claude", writerModel: "" })}
            className="h-7 rounded-md border border-border bg-panel px-2 text-xs text-fg"
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
          </select>
          {writer === "chatgpt" && chatgpt.models.length > 0 && (
            <select
              value={writerModel || "gpt-6-luna"}
              onChange={(e) => useWorkspaces.getState().setVoiceConfig({ writer, writerModel: e.target.value })}
              className="h-7 max-w-56 rounded-md border border-border bg-panel px-2 text-xs text-fg"
            >
              {[...new Set(["gpt-6-luna", ...chatgpt.models])].map((m) => (
                <option key={m} value={m}>
                  {m}
                </option>
              ))}
            </select>
          )}
          {writer === "claude" && (
            <input
              key={writer}
              defaultValue={writerModel}
              onBlur={(e) => useWorkspaces.getState().setVoiceConfig({ writer, writerModel: e.target.value.trim() })}
              placeholder="Model (default, e.g. sonnet)"
              className={inputClass + " w-48"}
            />
          )}
        </div>
        {writer !== "off" && (
          <label className="flex items-center gap-2 text-[13px] text-fg">
            Who decides what runs
            <select
              value={planner}
              onChange={(e) => useWorkspaces.getState().setVoiceConfig({ planner: e.target.value as "auto" | "always" | "off" })}
              className="h-7 rounded-md border border-border bg-panel px-2 text-xs text-fg"
            >
              <option value="auto">LLM for long or complex commands, Decisions API for short ones</option>
              <option value="always">LLM for every command (slower)</option>
              <option value="off">Decisions API for every command</option>
            </select>
          </label>
        )}
        {writer === "claude" && (
          <p className="text-[11px] text-fg-subtle">Runs Claude Code headless with your Claude login, without tools, MCP servers or your settings.</p>
        )}
      </Section>
    </>
  );
}

/** Text size of every terminal. */
function TerminalSettings() {
  const { terminalFontSize, setTerminalFontSize } = useWorkspaces();
  return (
    <Section title="Terminals">
      <label className="flex items-center gap-3 text-[13px] text-fg">
        Text size
        <input
          type="range"
          min={MIN_TERMINAL_FONT_SIZE}
          max={MAX_TERMINAL_FONT_SIZE}
          step={0.5}
          value={terminalFontSize}
          onChange={(e) => setTerminalFontSize(Number(e.target.value))}
          className="flex-1 accent-[var(--color-accent)]"
        />
        <span className="w-12 text-right font-mono text-xs text-fg-muted">{terminalFontSize}px</span>
        <button onClick={() => setTerminalFontSize(DEFAULT_TERMINAL_FONT_SIZE)} className="rounded-md px-2 py-1 text-xs text-fg-muted hover:bg-hover hover:text-fg">
          Reset
        </button>
      </label>
      <p className="text-xs text-fg-muted">
        Also {keysFor({ type: "terminal.zoom", delta: 1 })} / {keysFor({ type: "terminal.zoom", delta: -1 })} / {keysFor({ type: "terminal.zoom", delta: 0 })}, or{" "}
        {IS_MAC ? "⌘" : "Ctrl"} + scroll (pinch on a trackpad) over a terminal.
      </p>
    </Section>
  );
}

/** Which agent ⌘T, splits and "open a new window" start. */
function DefaultAgentSetting() {
  const { providers, defaultProvider, setDefaultProvider } = useWorkspaces();
  const current = defaultProviderId({ providers, defaultProvider });
  return (
    <Section title="New panes">
      <label className="flex items-center gap-2 text-[13px] text-fg">
        Default agent
        <select
          value={current}
          onChange={(e) => setDefaultProvider(e.target.value)}
          className="h-7 rounded-md border border-border bg-panel px-2 text-xs text-fg"
        >
          {providers.map((p) => (
            <option key={p.id} value={p.id} disabled={!p.installed}>
              {p.name}
              {p.installed ? "" : " (not installed)"}
            </option>
          ))}
        </select>
        <span className="text-xs text-fg-subtle">
          {keysFor({ type: "pane.spawn", provider: "default" })}, split and “open a new window” start this agent.
        </span>
      </label>
    </Section>
  );
}

/** How often the LLM re-estimates working agents' progress. */
function ProgressSettings() {
  const { progressConfig, setProgressConfig } = useWorkspaces();
  return (
    <Section title="Session progress">
      <label className="flex items-center gap-2 text-[13px] text-fg">
        <input type="checkbox" checked={progressConfig.enabled ?? true} onChange={(e) => setProgressConfig({ enabled: e.target.checked })} />
        Estimate how far working agents are, every
        <select
          value={progressConfig.intervalSec ?? 60}
          onChange={(e) => setProgressConfig({ intervalSec: Number(e.target.value) })}
          className="h-7 rounded-md border border-border bg-panel px-2 text-xs text-fg"
        >
          <option value={30}>30 seconds</option>
          <option value={60}>minute</option>
          <option value={120}>2 minutes</option>
          <option value={300}>5 minutes</option>
        </select>
      </label>
      <p className="text-xs text-fg-muted">Uses the prompt writer LLM below. Agents whose output hasn't changed aren't re-checked.</p>
    </Section>
  );
}

/** What the review inbox does on its own when an agent finishes. */
function ReviewSettings() {
  const { reviewConfig, setReviewConfig } = useWorkspaces();
  return (
    <Section title="Review inbox">
      <label className="flex items-center gap-2 text-[13px] text-fg">
        <input type="checkbox" checked={reviewConfig.autoTests ?? false} onChange={(e) => setReviewConfig({ autoTests: e.target.checked })} />
        Run the project's tests when an agent finishes (in its worktree)
      </label>
      <label className="flex items-center gap-2 text-[13px] text-fg">
        <input type="checkbox" checked={reviewConfig.screenshots ?? true} onChange={(e) => setReviewConfig({ screenshots: e.target.checked })} />
        Screenshot the agent's dev server for its card
      </label>
      <p className="text-xs text-fg-muted">Summaries use the prompt writer below; with it off, cards show the agent's last output instead.</p>
    </Section>
  );
}

/** Default model per agent for new panes. */
function ModelSettings() {
  const { providers, models, agentModels, setAgentModel, loadModels } = useWorkspaces();
  const agents = providers.filter((p) => p.installed && p.modelArg);
  useEffect(() => agents.forEach((p) => void loadModels(p.id)), [agents.length]); // eslint-disable-line react-hooks/exhaustive-deps
  if (agents.length === 0) return null;
  return (
    <Section title="Agent models (defaults for new panes)">
      <div className="grid grid-cols-[120px_1fr] items-center gap-2">
        {agents.map((p) => {
          const list = models[p.id] ?? [];
          return (
            <div key={p.id} className="contents">
              <span className="text-[13px] text-fg">{p.name}</span>
              {list.length ? (
                <select
                  value={agentModels[p.id] ?? ""}
                  onChange={(e) => setAgentModel(p.id, e.target.value || null)}
                  className="h-7 rounded-md border border-border bg-panel px-2 text-xs text-fg"
                >
                  <option value="">CLI default</option>
                  {list.map((m) => (
                    <option key={m.id} value={m.id}>
                      {m.label}
                    </option>
                  ))}
                </select>
              ) : (
                <input
                  defaultValue={agentModels[p.id] ?? ""}
                  onBlur={(e) => setAgentModel(p.id, e.target.value.trim() || null)}
                  placeholder="CLI default (type a model name)"
                  className={inputClass}
                />
              )}
            </div>
          );
        })}
      </div>
    </Section>
  );
}

function UpdateSettings() {
  const { version, checking, installing, available } = useUpdates();
  return (
    <Section title="Updates">
      <div className="flex items-center gap-3">
        <p className="flex-1 text-xs text-fg-muted">
          {available ? `winfer ${available.version} is available (you have ${version ?? "?"}).` : `winfer ${version ?? ""}`}
        </p>
        {available ? (
          <button
            onClick={() => void installUpdate()}
            disabled={installing}
            className="flex shrink-0 items-center gap-1.5 rounded-md bg-accent px-2.5 py-1 text-xs text-white hover:opacity-90 disabled:opacity-50"
          >
            {installing ? <Loader2 size={12} className="animate-spin" /> : <Download size={12} />}
            Install and restart
          </button>
        ) : (
          <button
            onClick={() => void checkForUpdates()}
            disabled={checking}
            className="flex shrink-0 items-center gap-1.5 rounded-md border border-border-strong px-2.5 py-1 text-xs text-fg hover:bg-hover disabled:opacity-50"
          >
            {checking && <Loader2 size={12} className="animate-spin" />}
            Check for updates
          </button>
        )}
      </div>
    </Section>
  );
}

export function SettingsModal() {
  const { settingsOpen, setSettings } = useWorkspaces();

  useEffect(() => {
    if (!settingsOpen) return;
    const onKey = (e: KeyboardEvent) => e.key === "Escape" && setSettings(false);
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [settingsOpen, setSettings]);

  if (!settingsOpen) return null;
  return (
    <div className="anim-backdrop fixed inset-0 z-50 flex items-center justify-center bg-black/40" onMouseDown={() => setSettings(false)}>
      <div className="anim-dialog max-h-[85vh] w-[600px] space-y-6 overflow-y-auto rounded-xl border border-border-strong bg-panel-raised p-6 shadow-2xl" onMouseDown={(e) => e.stopPropagation()}>
        <header className="flex items-center">
          <h1 className="flex-1 text-sm font-semibold text-fg">Settings</h1>
          <button onClick={() => setSettings(false)} className="rounded p-1 text-fg-subtle hover:bg-hover hover:text-fg">
            <X size={14} />
          </button>
        </header>
        <Section title="Keyboard shortcuts">
          <div className="flex items-center gap-3">
            <p className="flex-1 text-xs text-fg-muted">See every shortcut and change any of them, including the push-to-talk keys.</p>
            <button
              onClick={() => useWorkspaces.getState().setCheatSheet(true)}
              className="shrink-0 rounded-md border border-border-strong px-2.5 py-1 text-xs text-fg hover:bg-hover"
            >
              Edit shortcuts ({keysFor({ type: "ui.cheatSheet" })})
            </button>
          </div>
        </Section>
        <TerminalSettings />
        <DefaultAgentSetting />
        <ReviewSettings />
        <ProgressSettings />
        <ModelSettings />
        <VoiceSettings />
        <UpdateSettings />
      </div>
    </div>
  );
}
