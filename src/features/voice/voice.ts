/**
 * Voice flow in the UI: push-to-talk events from src-tauri/src/voice, then the LLM planner
 * (planner.ts) decides which tools to run and writes their prompts, and risky commands can ask
 * for confirmation first.
 */
import { applyVoiceShortcuts, isCustomized, VOICE_BINDINGS } from "@/commands/keymap";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { create } from "zustand";
import { execute, type Command } from "@/commands/bus";
import { sendText } from "@/features/terminal/xtermRegistry";
import { toolContext } from "@/features/palette/entries";
import { providerName, useWorkspaces } from "@/stores/workspaces";
import { leaves } from "@/features/layout/tree";
import { logActivity } from "@/stores/activity";
import { orderByMention, type Decision } from "./decision";
import { planWithLlm } from "./planner";
import { llmEnabled } from "@/features/llm/llm";
import { refreshChatGpt } from "@/features/llm/chatgpt";

export type Phase = "idle" | "listening" | "transcribing" | "deciding" | "confirm" | "done" | "error";
export type Mode = "command" | "dictation";

export interface ModelStatus {
  id: string;
  file: string;
  size: number;
  label: string;
  installed: boolean;
}

export interface VoiceStatus {
  models: ModelStatus[];
  sttModel: string;
  language: "en" | "de" | "auto";
  /** Whether winfer is signed in with ChatGPT. */
  credentials: boolean;
  /** Chosen microphone; null follows the system default. */
  inputDevice: string | null;
  /** Why the global push-to-talk keys couldn't be registered, if they couldn't. */
  shortcutError: string | null;
}

interface VoiceState {
  phase: Phase;
  mode: Mode;
  level: number;
  transcript: string;
  message: string;
  pending: { decisions: Decision[]; transcript: string } | null;
  timings: { sttMs?: number; decideMs?: number };
  recent: string[];
  lastCommands: Command[];
  status: VoiceStatus | null;
}

export const useVoice = create<VoiceState>(() => ({
  phase: "idle",
  mode: "command",
  level: 0,
  transcript: "",
  message: "",
  pending: null,
  timings: {},
  recent: [],
  lastCommands: [],
  status: null,
}));

let hideTimer: number | undefined;
function show(phase: Phase, patch: Partial<VoiceState> = {}, hideAfter?: number) {
  clearTimeout(hideTimer);
  useVoice.setState({ phase, ...patch });
  if (hideAfter) hideTimer = window.setTimeout(() => useVoice.setState({ phase: "idle", pending: null }), hideAfter);
}

export async function refreshVoiceStatus() {
  try {
    useVoice.setState({ status: await invoke<VoiceStatus>("voice_status") });
  } catch (err) {
    console.error("voice_status failed", err);
  }
}

/** Human-readable summary of what a decision does, for the HUD. */
function describe(d: Decision): string {
  const ctx = toolContext();
  if (d.tool.id === "spawn_panes") {
    const counts = (d.args.counts ?? {}) as Record<string, number>;
    const parts = Object.entries(counts)
      .filter(([, n]) => n > 0)
      .map(([id, n]) => `${n > 1 ? `${n}× ` : ""}${providerName(ctx.providers, id)}`);
    return `Open ${parts.join(", ") || "nothing"}`;
  }
  const details = Object.entries(d.args)
    .filter(([key, v]) => v !== undefined && typeof v !== "object" && !["submit", "text", "prompt", "newTab", "recent"].includes(key))
    .map(([key, v]) => {
      if (key === "target" && typeof v === "string" && v !== "focused") {
        const order = ctx.active.layout ? leaves(ctx.active.layout) : [];
        return `pane ${order.indexOf(v) + 1}`;
      }
      return String(v);
    });
  return details.length ? `${d.tool.title}: ${details.join(", ")}` : d.tool.title;
}

function commandsFor(decisions: Decision[], transcript: string): Command[] {
  const ctx = toolContext(useVoice.getState().lastCommands);
  const commands = decisions.flatMap((d) =>
    d.tool.toCommands(d.args, ctx).map((c) => (c.type === "pane.spawn" && d.args.worktree === true ? { ...c, worktree: true } : c)),
  );
  return orderByMention(commands, transcript, ctx);
}

function run(decisions: Decision[], transcript: string) {
  const commands = commandsFor(decisions, transcript);
  commands.forEach(execute);
  const written = decisions.map((d) => d.args.prompt ?? (d.tool.id === "send_to_pane" || d.tool.id === "broadcast" ? d.args.text : undefined)).find(Boolean);
  const summary = decisions.map(describe).join(" · ") + (written ? ` — “${String(written).slice(0, 140)}”` : "");
  useVoice.setState((s) => ({
    recent: [...s.recent, `${transcript} → ${summary}`].slice(-3),
    lastCommands: decisions.some((d) => d.tool.id === "repeat_last") ? s.lastCommands : commands,
  }));
  show("done", { message: summary, pending: null }, 2500);
  logActivity(useWorkspaces.getState().activeWorkspaceId, `Voice: “${transcript}” → ${summary}`);
}

export function confirmPending() {
  const pending = useVoice.getState().pending;
  if (pending) run(pending.decisions, pending.transcript);
}

export function cancelPending() {
  show("idle", { pending: null });
}

async function decide(transcript: string) {
  if (!llmEnabled()) {
    return show("error", { message: "Voice commands need an LLM: sign in with ChatGPT or install Codex or Claude Code (Settings → AI)." }, 6000);
  }
  show("deciding", { transcript, message: "Planning…" });
  const ctx = toolContext(useVoice.getState().lastCommands);
  const started = performance.now();
  const decisions = await planWithLlm(transcript, ctx, useVoice.getState().recent);
  if (!decisions) return show("error", { message: "Couldn't plan that command; check the LLM in Settings → AI." }, 6000);
  useVoice.setState((s) => ({ timings: { ...s.timings, decideMs: Math.round(performance.now() - started) } }));
  if (decisions.length === 0) show("error", { message: `No winfer command in “${transcript}”` }, 4000);
  else if (useWorkspaces.getState().voiceConfig.confirm && decisions.some((d) => d.tool.risk === "confirm")) {
    show("confirm", { pending: { decisions, transcript }, message: decisions.map(describe).join(" · ") });
  } else run(decisions, transcript);
}

function dictate(text: string) {
  const { focusedPaneId } = useWorkspaces.getState();
  if (!focusedPaneId) {
    show("error", { message: "Focus a pane to dictate into it" }, 3000);
    return;
  }
  // Typed, not submitted: review it, then press Enter.
  sendText(focusedPaneId, text);
  show("done", { transcript: text, message: "Typed into the focused pane" }, 2000);
}

/** Keeps whisper biased toward names it would otherwise mishear. */
function syncVocabulary() {
  const s = useWorkspaces.getState();
  const words = new Set<string>();
  for (const p of s.providers.filter((p) => p.installed)) {
    words.add(p.name);
    if (p.id !== "shell") words.add(p.id);
  }
  s.workspaces.forEach((w) => words.add(w.name));
  void invoke("voice_configure", { config: { vocabulary: [...words] } }).catch(() => {});
}

let started = false;
export function startVoice() {
  if (started) return;
  started = true;
  void refreshChatGpt();
  // Restore saved model choices once the workspace state has loaded.
  const applySaved = () => {
    const saved = useWorkspaces.getState().voiceConfig;
    void invoke("voice_configure", { config: saved }).then(refreshVoiceStatus, () => {});
    if (VOICE_BINDINGS.some((v) => isCustomized(v.id))) void applyVoiceShortcuts().catch(() => {}).finally(refreshVoiceStatus);
    useWorkspaces.subscribe((s, prev) => {
      if (VOICE_BINDINGS.some((v) => s.shortcuts[v.id] !== prev.shortcuts[v.id])) void applyVoiceShortcuts(s.shortcuts).catch(() => {}).finally(refreshVoiceStatus);
    });
  };
  if (useWorkspaces.getState().hydrated) applySaved();
  else {
    const unsubscribe = useWorkspaces.subscribe((s) => {
      if (s.hydrated) {
        unsubscribe();
        applySaved();
      }
    });
  }

  void listen<{ state: string; mode: Mode | null }>("voice://state", ({ payload }) => {
    const phase = useVoice.getState().phase;
    if (payload.state === "listening") show("listening", { mode: payload.mode ?? "command", transcript: "", message: "", level: 0 });
    else if (payload.state === "transcribing") show("transcribing");
    else if (payload.state === "idle" && (phase === "listening" || phase === "transcribing")) show("idle");
  });
  void listen<number>("voice://level", ({ payload }) => useVoice.setState({ level: payload }));
  void listen<{ message: string }>("voice://error", ({ payload }) => show("error", { message: payload.message }, 5000));
  void listen<{ mode: Mode; text: string; ms: number }>("voice://transcript", ({ payload }) => {
    useVoice.setState({ timings: { sttMs: payload.ms } });
    if (payload.mode === "dictation") dictate(payload.text);
    else void decide(payload.text);
  });

  let vocabTimer: number | undefined;
  useWorkspaces.subscribe((s, prev) => {
    if (s.providers === prev.providers && s.workspaces.length === prev.workspaces.length) return;
    clearTimeout(vocabTimer);
    vocabTimer = window.setTimeout(syncVocabulary, 500);
  });
  syncVocabulary();
}

/** Click-and-hold on the mic button. */
export const startListening = (mode: Mode = "command") => void invoke("voice_start", { mode });
export const stopListening = () => void invoke("voice_stop");

export async function configureVoice(config: { sttModel?: string; language?: "en" | "de" | "auto"; inputDevice?: string }) {
  useWorkspaces.getState().setVoiceConfig(config);
  await invoke("voice_configure", { config });
  await refreshVoiceStatus();
}
