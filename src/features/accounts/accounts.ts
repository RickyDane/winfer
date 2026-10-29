/**
 * Several Claude Code / Codex sign-ins (see src-tauri/src/accounts.rs), used in order: when one
 * account's plan runs out, agents continue on the next. The first of each agent is its default
 * sign-in; the others run with their own config dir.
 */
import { invoke } from "@tauri-apps/api/core";
import { create } from "zustand";

export const ACCOUNT_PROVIDERS = ["claude", "codex"] as const;

export interface Account {
  id: string;
  provider: string;
  label: string;
  enabled: boolean;
  dir: string;
  signedIn: boolean;
  /** Out of usage until (unix ms; 0 = unknown). */
  exhaustedUntil: number | null;
}

export const useAccounts = create<{ list: Account[] }>(() => ({ list: [] }));

export async function loadAccounts(): Promise<Account[]> {
  const list = await invoke<Account[] | null>("accounts_list").catch(() => useAccounts.getState().list);
  useAccounts.setState({ list: list ?? [] });
  return list ?? [];
}

export const defaultAccountId = (provider: string) => `${provider}:default`;
export const isDefaultAccount = (id: string) => id.endsWith(":default");
export const providerOfAccount = (id: string) => id.split(":")[0];

/** An agent's accounts in failover order. */
export const accountsOf = (provider: string, list = useAccounts.getState().list) => list.filter((a) => a.provider === provider);

/** Whether an agent has more than its default account to fall back on. */
export const hasAlternatives = (provider: string, list = useAccounts.getState().list) => accountsOf(provider, list).filter((a) => a.enabled).length > 1;

const usable = (a: Account, now = Date.now()) => a.enabled && a.signedIn && (a.exhaustedUntil === null || (a.exhaustedUntil > 0 && a.exhaustedUntil <= now));

/** The account new agents start on: the first with usage left (null = the default sign-in). */
export function startAccount(provider: string, list = useAccounts.getState().list): string | null {
  if (!(ACCOUNT_PROVIDERS as readonly string[]).includes(provider)) return null;
  const first = accountsOf(provider, list).find((a) => usable(a));
  return first && !isDefaultAccount(first.id) ? first.id : null;
}

/** Whether an account can take work now; undefined while the list isn't loaded or it's unknown. */
export function accountUsable(id: string, list = useAccounts.getState().list): boolean | undefined {
  const account = list.find((a) => a.id === id);
  return account ? usable(account) : undefined;
}

/** The account a pane runs on, by id (its default sign-in when none is set). */
export const paneAccount = (pane: { provider: string; accountId: string | null }) => pane.accountId ?? defaultAccountId(pane.provider);

export function accountLabel(id: string, list = useAccounts.getState().list): string {
  return list.find((a) => a.id === id)?.label ?? (isDefaultAccount(id) ? "Default" : id);
}

/**
 * Where an account's usage is kept in the usage store: the default account under the agent's id
 * (as before accounts existed), the others under their own id.
 */
export const usageKey = (id: string) => (isDefaultAccount(id) ? providerOfAccount(id) : id);

/** Records that an account ran out (until `resetAt`, if known) and returns the next one to use. */
export async function failoverAccount(provider: string, current: string, resetAt: number | null): Promise<string | null> {
  await invoke("accounts_mark_exhausted", { id: current, resetAt });
  const next = await invoke<string | null>("accounts_pick", { provider, exclude: [current] }).catch(() => null);
  void loadAccounts();
  return next;
}

export const addAccount = (provider: string, label: string) => invoke<Account>("accounts_add", { provider, label }).finally(loadAccounts);
export const removeAccount = (id: string) => invoke<void>("accounts_remove", { id }).finally(loadAccounts);
export const updateAccount = (id: string, patch: { label?: string; enabled?: boolean }) =>
  invoke<void>("accounts_update", { id, label: patch.label ?? null, enabled: patch.enabled ?? null }).finally(loadAccounts);
export const reorderAccounts = (provider: string, ids: string[]) => invoke<void>("accounts_reorder", { provider, ids }).finally(loadAccounts);
export const clearExhausted = (id: string) => invoke<void>("accounts_clear_exhausted", { id }).finally(loadAccounts);
export const accountLogin = (id: string) => invoke<{ command: string; env: Record<string, string> }>("accounts_login", { id });
