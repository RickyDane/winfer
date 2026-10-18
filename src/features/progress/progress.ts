/**
 * Session progress: an LLM (the prompt writer's provider) periodically reads each working
 * agent's task and recent output and estimates how far along it is, for the progress bars in
 * pane headers, the sidebar and the overview.
 */
import { llmCompose, llmEnabled, replyLanguage, wrongScript } from "@/features/llm/llm";
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
  if ("task" in p || "title" in p) saveTask(paneId);
}

/** Keeps the pane's task and title in the workspace store, which saves them across restarts. */
function saveTask(paneId: string) {
  const { task, title } = get(paneId);
  const saved = useWorkspaces.getState().paneTasks[paneId];
  if (saved?.task === task && saved?.title === title) return;
  useWorkspaces.setState((s) => ({ paneTasks: { ...s.paneTasks, [paneId]: { task, title } } }));
}

/** Brings back the tasks and titles saved before the last restart (panes that have none yet). */
function restoreTasks() {
  const saved = useWorkspaces.getState().paneTasks;
  useProgress.setState((s) => {
    const byPane = { ...s.byPane };
    for (const [id, { task, title }] of Object.entries(saved)) if (!byPane[id]) byPane[id] = { ...EMPTY, task, title };
    return { byPane };
  });
}

export function forgetProgress(paneId: string) {
  useProgress.setState((s) => {
    const { [paneId]: _, ...rest } = s.byPane;
    return { byPane: rest };
  });
  const { [paneId]: saved, ...rest } = useWorkspaces.getState().paneTasks;
  if (saved) useWorkspaces.setState({ paneTasks: rest });
}

/**
 * The agent got a new task (typed, sent by voice, or broadcast): progress starts over.
 * Short inputs ("y", "continue", menu picks) don't replace the task.
 */
export function setTask(paneId: string, text: string) {
  const task = text.trim();
  if (task.length < 12) return;
  // The title stays until the new one arrives: a follow-up usually continues the same work.
  patch(paneId, { task, percent: 0, step: null, updatedAt: null, error: null });
  lastEstimate.delete(paneId);
  void nameSession(paneId, task);
}

const NAMING = [
  "You name a coding agent's session for a list of sessions, like a short issue title.",
  "Given the session's current title (if any) and the user's newest message to the agent, reply with a title of 2 to 6 words summarizing the goal of the work,",
  'e.g. "Fix login redirect loop", "Persist session titles", "Dark mode for settings".',
  "Describe the work, don't quote or paraphrase the message: no filler (please, I noticed, can you), no trailing punctuation, no quotes.",
  "If the message continues or refines the current work (a follow-up, a correction, an answer, a small addition), reply with the current title unchanged.",
  "Reply with the title only.",
].join(" ");

/** Newest naming request per pane, so a slow answer doesn't overwrite a newer one. */
const naming = new Map<string, number>();
let namingSeq = 0;

/** Asks the LLM for a short title for the session's work (keeps the old one for follow-ups). */
export async function nameSession(paneId: string, message: string) {
  if (!llmEnabled()) return;
  const seq = ++namingSeq;
  naming.set(paneId, seq);
  const current = get(paneId).title;
  const user = [current ? `Current title: ${current}` : "Current title: (none, this is the first message)", `Newest message:\n${message.slice(0, 2000)}`].join("\n\n");
  try {
    const answer = await llmCompose(`${NAMING} Write it in ${replyLanguage(message)}.`, user);
    const title = cleanTitle(answer);
    if (naming.get(paneId) !== seq || !title || wrongScript(title, message)) return;
    if (useProgress.getState().byPane[paneId]) patch(paneId, { title });
  } catch {
    // No title then: the task's headline stands in.
  } finally {
    if (naming.get(paneId) === seq) naming.delete(paneId);
  }
}

/** A model's title answer, without quotes, a "Title:" label or trailing punctuation. */
export function cleanTitle(answer: string): string | null {
  const line = answer.trim().split("\n")[0] ?? "";
  const title = line
    .replace(/^(title|titel)\s*:\s*/i, "")
    .replace(/^["'“”‘’`*]+|["'“”‘’`*]+$/g, "")
    .replace(/[.!?:;,]+$/, "")
    .replace(/\s+/g, " ")
    .trim();
  return title && title.split(" ").length <= 10 ? title.slice(0, 60) : null;
}

const FILLER = /^(?:(?:hey|hi|ok(?:ay)?|so|also|and|now|then|next|alright|great|thanks?|thank you|please|pls|bitte|kannst du|can you|could you|would you|will you|i want you to|i'd like you to|i need you to|i noticed(?: that)?|i see(?: that)?|it seems(?: that)?|let's|lets)\b[\s,:;!.-]*)+/i;

/** Without an LLM title: the task's first sentence, trimmed of filler and cut to a few words. */
export function taskHeadline(task: string): string {
  const first = task.trim().split(/\n|(?<=[.!?])\s/)[0].trim();
  const stripped = first.replace(FILLER, "").replace(/[.!?:;,]+$/, "").trim() || first;
  const words = stripped.split(/\s+/);
  const short = words.length > 8 ? `${words.slice(0, 8).join(" ")}…` : stripped;
  return short.charAt(0).toUpperCase() + short.slice(1);
}

/** What to call the session's work: the LLM's title, else the task's headline. */
export const progressTitle = (p: Pick<Progress, "title" | "task"> | undefined): string | null =>
  p?.title ?? (p?.task ? taskHeadline(p.task) : null);

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
    const answer = await llmCompose(`${SYSTEM} Write step and title in ${replyLanguage(task)}.`, user);
    const result = parseEstimate(answer);
    if (!result) throw new Error("no estimate in the answer");
    if (result.step && wrongScript(result.step, task)) result.step = null;
    if (result.title && wrongScript(result.title, task)) result.title = null;
    const current = get(pane.id);
    // Estimates are noisy: within one task the bar only moves forward, and a working agent
    // isn't finished. A stopped one may be (agents without completion hooks only stop).
    const cap = status(pane.id) === "working" ? 99 : 100;
    const percent = Math.min(cap, Math.max(current.percent ?? 0, result.percent));
    // The naming call owns the title; the estimate's only fills in when there's none yet.
    patch(pane.id, { percent, step: result.step ?? current.step, title: current.title ?? result.title, updatedAt: Date.now(), estimating: false, error: null });
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
  if (useWorkspaces.getState().hydrated) restoreTasks();
  else {
    const stop = useWorkspaces.subscribe((s) => {
      if (!s.hydrated) return;
      stop();
      restoreTasks();
    });
  }
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
