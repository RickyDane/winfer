import { invoke } from "@tauri-apps/api/core";
import { Check, Download, Loader2, X } from "lucide-react";
import { useEffect, useState } from "react";
import { defaultProviderId, useWorkspaces } from "@/stores/workspaces";
import { keysFor, voiceKeys } from "@/commands/keymap";
import { configureVoice, refreshVoiceStatus, useVoice } from "@/features/voice/voice";
import { WORKERS_MODELS } from "@/features/voice/writer";

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
      {result && <p className={`text-xs ${result.ok ? "text-success" : "text-danger"}`}>{result.text}</p>}
    </div>
  );
}

function VoiceSettings() {
  const status = useVoice((s) => s.status);
  const readback = useWorkspaces((s) => s.voiceConfig.readback ?? false);
  const confirm = useWorkspaces((s) => s.voiceConfig.confirm ?? false);
  const writer = useWorkspaces((s) => s.voiceConfig.writer ?? "workers");
  const writerModel = useWorkspaces((s) => s.voiceConfig.writerModel ?? "");
  const shortcuts = useWorkspaces((s) => s.shortcuts);
  const [accountId, setAccountId] = useState("");
  const [token, setToken] = useState("");
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

  if (!status) return <div className="text-xs text-fg-subtle">Loading…</div>;

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

      <Section title="Prompt writer (LLM)">
        <p className="text-xs text-fg-muted">
          Turns what you say into a clear prompt for the agent, using its recent output as context (“tell Claude to make the page nicer” →
          a concrete task). Clef still decides which action runs. “Type … into Claude” stays word for word.
        </p>
        <div className="flex items-center gap-2">
          <select
            value={writer}
            onChange={(e) => useWorkspaces.getState().setVoiceConfig({ writer: e.target.value as "off" | "workers" | "codex", writerModel: "" })}
            className="h-7 rounded-md border border-border bg-panel px-2 text-xs text-fg"
          >
            <option value="workers">Cloudflare Workers AI (fast, uses the token below)</option>
            <option value="codex">OpenAI via your ChatGPT plan (Codex CLI, slower)</option>
            <option value="off">Off (send my words as spoken)</option>
          </select>
          {writer === "workers" && (
            <select
              value={writerModel || WORKERS_MODELS[0].id}
              onChange={(e) => useWorkspaces.getState().setVoiceConfig({ writerModel: e.target.value })}
              className="h-7 rounded-md border border-border bg-panel px-2 text-xs text-fg"
            >
              {WORKERS_MODELS.map((m) => (
                <option key={m.id} value={m.id}>
                  {m.label}
                </option>
              ))}
            </select>
          )}
          {writer === "codex" && (
            <input
              defaultValue={writerModel}
              onBlur={(e) => useWorkspaces.getState().setVoiceConfig({ writerModel: e.target.value.trim() })}
              placeholder="Model (Codex default)"
              className={inputClass + " w-48"}
            />
          )}
        </div>
        {writer === "codex" && <p className="text-[11px] text-fg-subtle">Uses your Codex login (“codex login” → Sign in with ChatGPT); runs read-only.</p>}
      </Section>

      <Section title="Voice commands (Cloudflare Clef)">
        <p className="text-xs text-fg-muted">
          The transcript and a short summary of your panes are sent to Cloudflare Workers AI to pick the action. Terminal output, file paths and
          environment variables are never sent. {status.credentials ? "Credentials are saved in the Keychain." : "Add credentials to enable."}
        </p>
        <div className="grid grid-cols-2 gap-2">
          <input className={inputClass} placeholder="Account ID" value={accountId} onChange={(e) => setAccountId(e.target.value)} />
          <input className={inputClass} placeholder="API token (Workers AI)" type="password" value={token} onChange={(e) => setToken(e.target.value)} />
        </div>
        <div className="flex items-center gap-2">
          <button
            disabled={!accountId || !token || busy !== null}
            onClick={() =>
              void act("save", async () => {
                await invoke("voice_set_credentials", { accountId, token });
                setAccountId("");
                setToken("");
                return "Saved to the Keychain";
              })
            }
            className="rounded-md bg-accent px-2.5 py-1 text-xs font-medium text-white hover:opacity-90 disabled:opacity-40"
          >
            Save
          </button>
          <button
            disabled={!status.credentials || busy !== null}
            onClick={() => void act("test", async () => `Connected in ${await invoke<number>("voice_test_connection")} ms`)}
            className="rounded-md border border-border-strong px-2.5 py-1 text-xs text-fg hover:bg-hover disabled:opacity-40"
          >
            {busy === "test" ? "Testing…" : "Test connection"}
          </button>
          {status.credentials && (
            <button
              onClick={() => void act("clear", async () => (await invoke("voice_clear_credentials"), "Removed"))}
              className="rounded-md px-2.5 py-1 text-xs text-fg-subtle hover:bg-hover hover:text-danger"
            >
              Remove
            </button>
          )}
          <select
            value={status.clefModel}
            onChange={(e) => void configureVoice({ clefModel: e.target.value })}
            className="ml-auto h-7 rounded-md border border-border bg-panel px-2 text-xs text-fg"
          >
            <option value="clef-flash">clef-flash (fast)</option>
            <option value="clef">clef (more precise)</option>
          </select>
        </div>
        {result && <p className={result.ok ? "text-xs text-success" : "text-xs text-danger"}>{result.text}</p>}
      </Section>
    </>
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
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/40" onMouseDown={() => setSettings(false)}>
      <div className="max-h-[85vh] w-[600px] space-y-6 overflow-y-auto rounded-xl border border-border-strong bg-panel-raised p-6 shadow-2xl" onMouseDown={(e) => e.stopPropagation()}>
        <header className="flex items-center">
          <h1 className="flex-1 text-sm font-semibold text-fg">Settings</h1>
          <button onClick={() => setSettings(false)} className="rounded p-1 text-fg-subtle hover:bg-hover hover:text-fg">
            <X size={14} />
          </button>
        </header>
        <DefaultAgentSetting />
        <ReviewSettings />
        <ModelSettings />
        <VoiceSettings />
      </div>
    </div>
  );
}
