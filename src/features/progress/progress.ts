/**
 * Session progress: an LLM (the prompt writer's provider) periodically reads each working
 * agent's task and recent output and estimates how far along it is, for the progress bars in
 * pane headers, the sidebar and the overview.
 */
import { llmCompose, llmEnabled } from "@/features/llm/llm";
import { create } from "zustand";
import { outputAge, readTerminal } from "@/features/terminal/xtermRegistry";
import { worktreeDiff } from "@/features/worktree/worktree";
import { speak } from "@/features/voice/readback";
import { providerName, useWorkspaces, type Pane } from "@/stores/workspaces";

export interface Progress {
  /** What the agent was asked to do (its start prompt or the last message it got). */
  task: string | null;
  /** Short task title from the LLM, for compact displays. */
  title: string | null;
  /** 0–100, or null before the first estimate. */
  percent: number | null;
  /** What the agent is doing right now, per the LLM. */
  step: string | null;
  updatedAt: number | null;
  estimating: boolean;
  error: string | null;
}

interface ProgressState {
  byPane: Record<string, Progress>;
}

export const useProgress = create<ProgressState>(() => ({ byPane: {} }));

const EMPTY: Progress = { task: null, title: null, percent: null, step: null, updatedAt: null, estimating: false, error: null };

const get = (paneId: string): Progress => useProgress.getState().byPane[paneId] ?? EMPTY;

function patch(paneId: string, p: Partial<Progress>) {
  useProgress.setState((s) => ({ byPane: { ...s.byPane, [paneId]: { ...(s.byPane[paneId] ?? EMPTY), ...p } } }));
}

export function forgetProgress(paneId: string) {
  useProgress.setState((s) => {
    const { [paneId]: _, ...rest } = s.byPane;
    return { byPane: rest };
  });
}

/**
 * The agent got a new task (typed, sent by voice, or broadcast): progress starts over.
 * Short inputs ("y", "continue", menu picks) don't replace the task.
 */
export function setTask(paneId: string, text: string) {
  const task = text.trim();
  if (task.length < 12) return;
  patch(paneId, { task, title: null, percent: 0, step: null, updatedAt: null, error: null });
  lastEstimate.delete(paneId);
}

/** Called when an agent finishes: done means done, no need to ask the LLM. */
export function markFinished(paneId: string) {
  const p = get(paneId);
  if (p.task || p.percent !== null) patch(paneId, { percent: 100, step: "Finished", updatedAt: Date.now() });
}

const SYSTEM = [
  "You estimate how far a coding agent is with its current task, from the task and the agent's recent terminal output.",
  "Evidence: its plan or todo list (count checked items), files it edited, tests or builds it ran and their results, what it says it is doing.",
  "100 only when the task is complete and the agent stopped; exploring and planning is early (5–20), implementing is the middle, testing and fixing is late.",
  "An agent that stopped to ask a question or wait for approval isn't done. If the output shows no work on the task yet, answer 0.",
  'Reply with JSON only: {"percent": <integer 0-100>, "step": "<what it is doing now, max 8 words>", "title": "<the task, max 6 words>"}.',
  "Write step and title in the language of the task.",
].join(" ");

/** Pulls {percent, step, title} out of a model answer, tolerating prose around the JSON. */
export function parseEstimate(answer: string): { percent: number; step: string | null; title: string | null } | null {
  const match = /\{[\s\S]*\}/.exec(answer);
  if (!match) return null;
  try {
    const json = JSON.parse(match[0]) as { percent?: unknown; step?: unknown; title?: unknown };
    const percent = Math.round(Number(json.percent));
    if (!Number.isFinite(percent)) return null;
    const text = (v: unknown) => (typeof v === "string" && v.trim() ? v.trim().slice(0, 80) : null);
    return { percent: Math.min(100, Math.max(0, percent)), step: text(json.step), title: text(json.title) };
  } catch {
    return null;
  }
}

const status = (paneId: string) =>
  useWorkspaces
    .getState()
    .workspaces.flatMap((w) => w.panes)
    .find((p) => p.id === paneId)?.status;

/** Estimates right away (an agent without completion hooks just went quiet). */
export function estimateNow(paneId: string) {
  const pane = useWorkspaces
    .getState()
    .workspaces.flatMap((w) => w.panes)
    .find((p) => p.id === paneId);
  if (pane && useWorkspaces.getState().progressConfig.enabled !== false) void estimate(pane);
}

/** Output the last estimate was based on, so unchanged panes aren't re-asked. */
const lastEstimate = new Map<string, string>();

export async function estimate(pane: Pane) {
  if (!llmEnabled() || get(pane.id).estimating) return;
  const progress = get(pane.id);
  // Without a task there's nothing to measure: a freshly started agent's welcome screen would
  // only get a made-up estimate.
  const task = progress.task ?? pane.prompt;
  if (!task) return;
  const output = (readTerminal(pane.id, 120) ?? "").slice(-3500).trim();
  if (!output || lastEstimate.get(pane.id) === output) return;
  lastEstimate.set(pane.id, output);

  let changes = "";
  if (pane.worktree) {
    const diff = await worktreeDiff(pane.worktree).catch(() => null);
    const files = diff?.diff.match(/^diff --git a\/.+ b\/(.+)$/gm)?.map((l) => l.replace(/^.* b\//, "")) ?? [];
    if (diff) changes = `Files changed so far: ${[...files, ...diff.untracked].join(", ") || "none"}`;
  }
  const user = [
    `Task: ${task}`,
    `Agent: ${providerName(useWorkspaces.getState().providers, pane.provider)}`,
    `Agent state: ${status(pane.id) === "working" ? "working" : "stopped, waiting for input"}`,
    changes,
    `Recent output:\n${output}`,
  ]
    .filter(Boolean)
    .join("\n\n");

  patch(pane.id, { estimating: true });
  try {
    const answer = await llmCompose(SYSTEM, user);
    const result = parseEstimate(answer);
    if (!result) throw new Error("no estimate in the answer");
    const current = get(pane.id);
    // Estimates are noisy: within one task the bar only moves forward, and a working agent
    // isn't finished. A stopped one may be (agents without completion hooks only stop).
    const cap = status(pane.id) === "working" ? 99 : 100;
    const percent = Math.min(cap, Math.max(current.percent ?? 0, result.percent));
    patch(pane.id, { percent, step: result.step, title: result.title ?? current.title, updatedAt: Date.now(), estimating: false, error: null });
  } catch (err) {
    patch(pane.id, { estimating: false, error: String(err) });
  }
}

const allPanes = () => useWorkspaces.getState().workspaces.flatMap((w) => w.panes);

let started = false;
/** Re-estimates working agents every `progressInterval` seconds while their output changes. */
export function startProgressTracking() {
  if (started) return;
  started = true;
  window.setInterval(() => {
    const s = useWorkspaces.getState();
    if (s.progressConfig.enabled === false) return;
    const interval = (s.progressConfig.intervalSec ?? 60) * 1000;
    const open = new Set(allPanes().map((p) => p.id));
    for (const id of Object.keys(useProgress.getState().byPane)) if (!open.has(id)) forgetProgress(id);
    for (const pane of allPanes()) {
      if (pane.provider === "shell" || pane.status !== "working") continue;
      const p = get(pane.id);
      const due = !p.updatedAt || Date.now() - p.updatedAt >= interval;
      // First estimate soon after the agent starts; then on the interval, if it's still busy.
      if (due && outputAge(pane.id) < interval) void estimate(pane);
    }
  }, 5000);
}

/** Speaks every working session's progress ("how far are the agents?"). */
export function readProgressAloud() {
  const s = useWorkspaces.getState();
  const sessions = allPanes().filter((p) => p.provider !== "shell" && (p.status === "working" || get(p.id).percent !== null));
  if (sessions.length === 0) return speak("No agent is working right now.", { force: true });
  const lines = sessions.map((p) => {
    const progress = get(p.id);
    const name = providerName(s.providers, p.provider);
    const what = progress.title ? ` on ${progress.title}` : "";
    return progress.percent === null ? `${name}${what}: just started.` : `${name}${what}: ${progress.percent} percent${progress.step ? `, ${progress.step}` : ""}.`;
  });
  speak(lines.join(" "), { force: true });
}
