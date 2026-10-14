/**
 * Voice flow in the UI: push-to-talk events from src-tauri/src/voice, then Clef decides which
 * tools to run (decision.ts) and the policy gate runs, confirms or rejects.
 */
import { applyVoiceShortcuts, isCustomized, VOICE_BINDINGS } from "@/commands/keymap";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { create } from "zustand";
import { execute, type Command } from "@/commands/bus";
import { sendText } from "@/features/terminal/xtermRegistry";
import { toolContext } from "@/features/palette/entries";
import { defaultProviderId, providerName, useWorkspaces } from "@/stores/workspaces";
import { leaves } from "@/features/layout/tree";
import { logActivity } from "@/stores/activity";
import { needsWriting, planPerRecipient, recipientsOf, splitTasks, writeInstructions } from "./writer";
import { buildRequest, orderByMention, parseDecision, verdict, type Decision, type DecisionResult } from "./decision";
import { isComplex, planWithLlm } from "./planner";
import { llmEnabled } from "@/features/llm/llm";

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
  clefModel: string;
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
  /** `planned`: from the LLM planner, whose prompts are already written. */
  pending: { decisions: Decision[]; transcript: string; planned?: boolean } | null;
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

/** Spawn order for a decision: one entry per new agent, in the order they were named. */
function spawnOrder(d: Decision, transcript: string): string[] {
  const ctx = toolContext();
  return orderByMention(d.tool.toCommands(d.args, ctx), transcript, ctx).flatMap((c) =>
    c.type === "pane.spawn" ? Array(c.count ?? 1).fill(c.provider === "default" ? defaultProviderId() : c.provider) : [],
  );
}

async function run(decisions: Decision[], transcript: string, opts: { planned?: boolean } = {}) {
  // Commands that reach several agents get one planned message per agent.
  const planned = new Map<Decision, Command[]>();
  // Prompts that are already written (by the planner or task splitter) and shouldn't be rewritten.
  const ready = new Set<Decision>(opts.planned ? decisions : []);
  decisions = [...decisions];
  for (const [i, d] of decisions.entries()) {
    if (ready.has(d)) continue;
    // One new agent asked to do several separate things: one agent per task, each in its own
    // worktree, so every feature gets its own branch and pull request.
    const order = (d.tool.id === "spawn_panes" || d.tool.id === "start_features") && d.args.prompt ? spawnOrder(d, transcript) : [];
    if (order.length !== 1 || order[0] === "shell") continue;
    show("deciding", { transcript, message: "Writing the prompt…" });
    const tasks = await splitTasks(transcript, String(d.args.prompt), providerName(useWorkspaces.getState().providers, order[0]));
    if (!tasks) continue;
    if (tasks.length === 1) {
      decisions[i] = { ...d, args: { ...d.args, prompt: tasks[0] } };
      ready.add(decisions[i]);
      continue;
    }
    const model = d.args.model && d.args.model !== "default" ? String(d.args.model) : undefined;
    planned.set(d, tasks.map((prompt) => ({ type: "pane.spawn", provider: order[0], count: 1, prompt, model, worktree: true })));
  }
  for (const d of decisions) {
    if (planned.has(d) || ready.has(d)) continue;
    const recipients = recipientsOf(d, d.tool.id === "spawn_panes" ? spawnOrder(d, transcript) : []);
    const instruction = String(d.args.prompt ?? d.args.text ?? "");
    if (!recipients || !instruction) continue;
    show("deciding", { transcript, message: `Writing ${recipients.length} prompts…` });
    const commands = await planPerRecipient(transcript, instruction, recipients);
    if (commands) planned.set(d, commands);
  }
  const single = decisions.filter((d) => !planned.has(d) && !ready.has(d));
  if (needsWriting(single)) {
    show("deciding", { transcript, message: "Writing the prompt…" });
    const written = await writeInstructions(single, transcript);
    decisions = decisions.map((d) => (single.includes(d) ? written[single.indexOf(d)] : d));
  }
  const commands = decisions.flatMap((d) => planned.get(d) ?? commandsFor([d], transcript));
  commands.forEach(execute);
  const perAgent = [...planned.values()].flat().length;
  const split = [...planned.entries()].filter(([d]) => d.tool.id === "spawn_panes" || d.tool.id === "start_features").map(([, c]) => c.length);
  const written = perAgent ? `${perAgent} individual prompts` : decisions.map((d) => d.args.prompt ?? (d.tool.id === "send_to_pane" || d.tool.id === "broadcast" ? d.args.text : undefined)).find(Boolean);
  const summary =
    decisions.map(describe).join(" · ") +
    (split.length ? ` — split into ${split.join(" + ")} tasks` : "") +
    (written ? ` — “${String(written).slice(0, 140)}”` : "");
  useVoice.setState((s) => ({
    recent: [...s.recent, `${transcript} → ${summary}`].slice(-3),
    lastCommands: decisions.some((d) => d.tool.id === "repeat_last") ? s.lastCommands : commands,
  }));
  show("done", { message: summary, pending: null }, 2500);
  logActivity(useWorkspaces.getState().activeWorkspaceId, `Voice: “${transcript}” → ${summary}`);
}

export function confirmPending(alternative?: Decision) {
  const pending = useVoice.getState().pending;
  if (!pending) return;
  void run(alternative ? [alternative] : pending.decisions, pending.transcript, { planned: pending.planned && !alternative });
}

export function cancelPending() {
  show("idle", { pending: null });
}

async function decide(transcript: string) {
  show("deciding", { transcript, message: "" });
  const ctx = toolContext(useVoice.getState().lastCommands);
  const config = useWorkspaces.getState().voiceConfig;
  const planner = config.planner ?? "auto";
  // Long or complex commands go to the full LLM; Clef only picks from short questions.
  if (planner !== "off" && llmEnabled() && (planner === "always" || isComplex(transcript))) {
    show("deciding", { transcript, message: "Planning…" });
    const started = performance.now();
    const decisions = await planWithLlm(transcript, ctx, useVoice.getState().recent);
    if (decisions) {
      useVoice.setState((s) => ({ timings: { ...s.timings, decideMs: Math.round(performance.now() - started) } }));
      if (decisions.length === 0) return show("error", { message: `No winfer command in “${transcript}”` }, 4000);
      if (config.confirm && decisions.some((d) => d.tool.risk === "confirm")) {
        return show("confirm", { pending: { decisions, transcript, planned: true }, message: decisions.map(describe).join(" · ") });
      }
      return run(decisions, transcript, { planned: true });
    }
  }
  const request = buildRequest(transcript, ctx, useVoice.getState().recent);
  const started = performance.now();
  let result: DecisionResult;
  try {
    result = await invoke<DecisionResult>("voice_decide", { body: request });
  } catch (err) {
    show("error", { message: String(err) }, 6000);
    return;
  }
  useVoice.setState((s) => ({ timings: { ...s.timings, decideMs: Math.round(performance.now() - started) } }));

  const decisions = parseDecision(result, ctx);
  switch (verdict(decisions, { confirm: useWorkspaces.getState().voiceConfig.confirm ?? false })) {
    case "run":
      return run(decisions, transcript);
    case "confirm":
      return show("confirm", { pending: { decisions, transcript }, message: decisions.map(describe).join(" · ") });
    case "reject":
      return show("error", { message: `No winfer command in “${transcript}”` }, 4000);
  }
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

export async function configureVoice(config: { sttModel?: string; clefModel?: string; language?: "en" | "de" | "auto"; inputDevice?: string }) {
  useWorkspaces.getState().setVoiceConfig(config);
  await invoke("voice_configure", { config });
  await refreshVoiceStatus();
}
