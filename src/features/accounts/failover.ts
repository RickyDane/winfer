/**
 * Moves Claude Code / Codex terminal panes to the next account when theirs runs out of usage:
 * once the agent has stopped, the pane restarts on the next account, resumes its session (shared
 * between accounts, see src-tauri/src/accounts.rs) and is told to continue.
 *
 * A pane counts as out when the CLI prints its usage-limit message (checked against the account's
 * live limits when they can be read), or when the usage store sees the account's limit reached
 * while the pane was working. winfer code chats do the same in their bridges.
 */
import { outputAge, outputTaps, restartTerminal, sendText } from "@/features/terminal/xtermRegistry";
import { logActivity } from "@/stores/activity";
import { toast } from "@/stores/toasts";
import { providerName, useWorkspaces, type Pane } from "@/stores/workspaces";
import { exhaustedListeners, leftNow, refreshAccount, tightest, useUsage, when } from "@/features/usage/usage";
import { ACCOUNT_PROVIDERS, accountLabel, failoverAccount, hasAlternatives, isDefaultAccount, loadAccounts, paneAccount, usageKey } from "./accounts";

export const CONTINUE_PROMPT = "The previous request stopped because the account ran out of usage. winfer switched to another account: continue where you left off.";

/** What Claude Code and Codex print when the plan's limit is reached. */
const LIMIT_TEXT = /(usage limit reached|you've hit your (usage )?limit|you have hit your (usage )?limit|5-hour limit reached|weekly limit reached|out of extra usage)/i;

const TAIL = 2048;
const tails = new Map<string, string>();
const decoders = new Map<string, TextDecoder>();
/** Panes waiting to move: pane id → when the account resets (if known). */
const waiting = new Map<string, number | null>();
const moving = new Set<string>();
/** When a pane's limit message was last checked: output that keeps matching is checked once a minute. */
const checkedAt = new Map<string, number>();

const paneById = (id: string) => useWorkspaces.getState().workspaces.flatMap((w) => w.panes).find((p) => p.id === id);
const isAccountAgent = (provider: string) => (ACCOUNT_PROVIDERS as readonly string[]).includes(provider);

const ANSI = /\x1b\[[0-9;?]*[ -/]*[@-~]|\x1b\][^\x07\x1b]*(\x07|\x1b\\)|\x1b[@-_]/g;

function onOutput(paneId: string, provider: string, data: ArrayBuffer) {
  if (!isAccountAgent(provider) || moving.has(paneId)) return;
  let decoder = decoders.get(paneId);
  if (!decoder) decoders.set(paneId, (decoder = new TextDecoder()));
  const text = decoder.decode(new Uint8Array(data), { stream: true }).replace(ANSI, "");
  const tail = ((tails.get(paneId) ?? "") + text).slice(-TAIL);
  tails.set(paneId, tail);
  // Only the newest output counts: an old message scrolled past, or one the agent quotes in its
  // work, is checked against the account's limits before anything moves.
  if (!waiting.has(paneId) && LIMIT_TEXT.test(text) && Date.now() - (checkedAt.get(paneId) ?? 0) > 60_000) {
    checkedAt.set(paneId, Date.now());
    void confirmLimit(paneId);
  }
}

/** The limit message showed: move the pane if its account's limits agree (or can't be read). */
async function confirmLimit(paneId: string) {
  const pane = paneById(paneId);
  if (!pane || !hasAlternatives(pane.provider)) return;
  const account = paneAccount(pane);
  const key = usageKey(account);
  await refreshAccount(account);
  const { snapshots, errors } = useUsage.getState();
  const w = tightest(snapshots[key]);
  if (!errors[key] && w && leftNow(w) > 1) return;
  waiting.set(paneId, w?.resetsAt ?? null);
  tryMove(paneId);
}

/** The usage store saw an account run out: panes working on it move once they stop. */
function onExhausted(accountId: string, resetAt: number | null) {
  for (const pane of useWorkspaces.getState().workspaces.flatMap((w) => w.panes)) {
    if (!isAccountAgent(pane.provider) || paneAccount(pane) !== accountId || pane.status !== "working") continue;
    if (!hasAlternatives(pane.provider)) continue;
    waiting.set(pane.id, resetAt);
    tryMove(pane.id);
  }
}

/** Moves a waiting pane once its agent has stopped (not working, output settled). */
function tryMove(paneId: string) {
  const pane = paneById(paneId);
  if (!pane) return void waiting.delete(paneId);
  if (pane.status === "working" || outputAge(paneId) < 1500 || moving.has(paneId)) return;
  const resetAt = waiting.get(paneId) ?? null;
  waiting.delete(paneId);
  void move(pane, resetAt);
}

async function move(pane: Pane, resetAt: number | null) {
  moving.add(pane.id);
  try {
    const from = paneAccount(pane);
    const name = providerName(useWorkspaces.getState().providers, pane.provider);
    const next = await failoverAccount(pane.provider, from, resetAt);
    if (!next) {
      toast(`${name}: every account is out of usage`, "error", resetAt ? `"${accountLabel(from)}" resets ${when(resetAt)}` : undefined);
      return;
    }
    const s = useWorkspaces.getState();
    s.updatePane(pane.id, { accountId: isDefaultAccount(next) ? null : next, resume: true, prompt: null });
    tails.delete(pane.id);
    decoders.delete(pane.id);
    await restartTerminal(paneById(pane.id)!);
    const text = `${name}: "${accountLabel(from)}" is out of usage — continuing on "${accountLabel(next)}"`;
    toast(text, "info", resetAt ? `"${accountLabel(from)}" resets ${when(resetAt)}` : undefined);
    logActivity(pane.workspaceId, text, pane.provider);
    await settled(pane.id);
    sendText(pane.id, CONTINUE_PROMPT);
    sendText(pane.id, "\r");
  } finally {
    moving.delete(pane.id);
  }
}

/** Waits until a restarted agent is up: it printed something, then went quiet (at most 30s). */
async function settled(paneId: string) {
  const started = Date.now();
  while (Date.now() - started < 30_000) {
    await new Promise((r) => setTimeout(r, 500));
    const age = outputAge(paneId);
    if (age !== Infinity && age >= 1500 && Date.now() - started > 2000) return;
  }
}

let started = false;

export function startAccountFailover() {
  if (started) return;
  started = true;
  void loadAccounts();
  outputTaps.add(onOutput);
  exhaustedListeners.add(onExhausted);
  // Waiting panes move as soon as their agent stops.
  useWorkspaces.subscribe(() => waiting.forEach((_, id) => tryMove(id)));
  window.setInterval(() => waiting.forEach((_, id) => tryMove(id)), 2000);
}
