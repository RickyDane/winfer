import { ArrowDown, ArrowUp, LogIn, Plus, RefreshCw, Trash2 } from "lucide-react";
import { useEffect, useState } from "react";
import { ask } from "@tauri-apps/plugin-dialog";
import { cn } from "@/lib/cn";
import { ProviderIcon } from "@/features/providers/ProviderIcon";
import { leftNow, levelColor, refreshOtherAccounts, refreshUsage, tightest, useUsage, when } from "@/features/usage/usage";
import { toast } from "@/stores/toasts";
import { providerName, useWorkspaces } from "@/stores/workspaces";
import {
  ACCOUNT_PROVIDERS,
  accountLogin,
  accountsOf,
  addAccount,
  clearExhausted,
  isDefaultAccount,
  loadAccounts,
  removeAccount,
  reorderAccounts,
  updateAccount,
  useAccounts,
  usageKey,
  type Account,
} from "./accounts";

const iconButton = "grid size-7 place-items-center rounded-md text-fg-subtle hover:bg-hover hover:text-fg disabled:opacity-30 disabled:hover:bg-transparent";

/** Opens a terminal that signs the account in (`claude /login`, `codex login`) in its own config dir. */
async function signIn(account: Account) {
  const { command } = await accountLogin(account.id);
  const s = useWorkspaces.getState();
  const id = s.addPane("shell", { prompt: command, customTitle: `Sign in · ${account.label}` });
  if (!isDefaultAccount(account.id)) s.updatePane(id, { accountId: account.id });
  s.setSettings(false);
  toast(`Sign in to "${account.label}" in the new terminal`, "info", "Close it when you're done; winfer picks the account up on its own.");
}

function AccountRow({ account, index, count, ids }: { account: Account; index: number; count: number; ids: string[] }) {
  const snapshot = useUsage((s) => s.snapshots[usageKey(account.id)]);
  const w = tightest(snapshot);
  const [label, setLabel] = useState(account.label);
  useEffect(() => setLabel(account.label), [account.label]);
  const move = (by: number) => {
    const next = [...ids];
    next.splice(index, 1);
    next.splice(index + by, 0, account.id);
    void reorderAccounts(account.provider, next);
  };
  const out = account.exhaustedUntil !== null;
  return (
    <div className="flex items-center gap-3 px-3.5 py-2.5">
      <span className="w-4 text-center font-mono text-[11px] text-fg-subtle">{index + 1}</span>
      <div className="min-w-0 flex-1 space-y-0.5">
        <input
          value={label}
          onChange={(e) => setLabel(e.target.value)}
          onBlur={() => label.trim() && label !== account.label && void updateAccount(account.id, { label })}
          onKeyDown={(e) => e.key === "Enter" && (e.target as HTMLInputElement).blur()}
          aria-label="Account name"
          className="w-full rounded bg-transparent text-[13px] text-fg outline-none focus:bg-panel-raised focus:px-1"
        />
        <div className="flex flex-wrap items-center gap-x-2 text-[11px] text-fg-subtle">
          {isDefaultAccount(account.id) ? <span>Default sign-in</span> : <span className="truncate font-mono" title={account.dir}>{account.dir.replace(/^.*\/accounts\//, "accounts/")}</span>}
          {!account.signedIn && <span className="text-warning">Not signed in</span>}
          {out && (
            <button type="button" onClick={() => void clearExhausted(account.id)} className="text-danger hover:underline" title="Try this account again now">
              Out of usage{account.exhaustedUntil ? ` · resets ${when(account.exhaustedUntil)}` : ""}
            </button>
          )}
          {w && !out && (
            <span style={{ color: levelColor(leftNow(w)) }}>
              {Math.round(leftNow(w))}% of {w.label.toLowerCase()} left
            </span>
          )}
        </div>
      </div>
      <div className="flex items-center gap-0.5">
        <button type="button" className={iconButton} onClick={() => move(-1)} disabled={index === 0} aria-label="Use earlier" title="Use earlier">
          <ArrowUp size={13} />
        </button>
        <button type="button" className={iconButton} onClick={() => move(1)} disabled={index === count - 1} aria-label="Use later" title="Use later">
          <ArrowDown size={13} />
        </button>
        {!isDefaultAccount(account.id) && (
          <>
            <button type="button" className={iconButton} onClick={() => void signIn(account)} aria-label="Sign in" title={account.signedIn ? "Sign in again" : "Sign in"}>
              <LogIn size={13} />
            </button>
            <button
              type="button"
              className={cn(iconButton, "hover:text-danger")}
              aria-label="Remove account"
              title="Remove account"
              onClick={async () => {
                if (await ask(`Remove "${account.label}"? Its sign-in is deleted; sessions and settings stay.`, { title: "Remove account", kind: "warning" })) {
                  await removeAccount(account.id).catch((e: unknown) => toast(String(e), "error"));
                }
              }}
            >
              <Trash2 size={13} />
            </button>
          </>
        )}
        <button
          type="button"
          role="switch"
          aria-checked={account.enabled}
          aria-label={account.enabled ? "Disable account" : "Enable account"}
          onClick={() => void updateAccount(account.id, { enabled: !account.enabled })}
          className={cn("relative ml-1 h-5 w-9 shrink-0 rounded-full transition-colors", account.enabled ? "bg-accent" : "bg-active ring-1 ring-border-strong ring-inset")}
        >
          <span className={cn("absolute top-0.5 left-0.5 size-4 rounded-full bg-white shadow transition-transform duration-150", account.enabled && "translate-x-4")} />
        </button>
      </div>
    </div>
  );
}

function ProviderAccounts({ provider }: { provider: string }) {
  const all = useAccounts((s) => s.list);
  const list = accountsOf(provider, all);
  const providers = useWorkspaces((s) => s.providers);
  const [adding, setAdding] = useState(false);
  const name = providerName(providers, provider);
  const ids = list.map((a) => a.id);
  return (
    <section className="space-y-2">
      <div className="flex items-center gap-2 px-1">
        <ProviderIcon provider={provider} />
        <h3 className="flex-1 text-[13px] font-medium text-fg">{name}</h3>
        <button
          type="button"
          disabled={adding}
          onClick={async () => {
            setAdding(true);
            try {
              const account = await addAccount(provider, "");
              await signIn({ ...account, dir: "", signedIn: false, exhaustedUntil: null });
            } catch (e) {
              toast(String(e), "error");
            } finally {
              setAdding(false);
            }
          }}
          className="flex h-7 items-center gap-1.5 rounded-md px-2.5 text-xs text-fg-muted hover:bg-hover hover:text-fg disabled:opacity-50"
        >
          <Plus size={13} /> Add account
        </button>
      </div>
      <div className="divide-y divide-border overflow-hidden rounded-lg border border-border bg-panel">
        {list.map((a, i) => <AccountRow key={a.id} account={a} index={i} count={list.length} ids={ids} />)}
      </div>
    </section>
  );
}

/** Settings → Accounts: several Claude Code / Codex sign-ins, used in order as each runs out. */
export function AccountsSettings() {
  const [busy, setBusy] = useState(false);
  useEffect(() => {
    void loadAccounts().then(() => refreshOtherAccounts(true));
    // Back from a sign-in terminal: pick the new sign-in up.
    const onFocus = () => void loadAccounts();
    window.addEventListener("focus", onFocus);
    const timer = window.setInterval(() => void loadAccounts(), 5000);
    return () => {
      window.removeEventListener("focus", onFocus);
      window.clearInterval(timer);
    };
  }, []);
  return (
    <div className="space-y-6">
      <div className="flex items-start gap-3 px-1">
        <p className="flex-1 text-xs leading-relaxed text-fg-muted">
          When an account runs out of usage, agents move to the next one in this order and continue their session there: winfer code chats right away,
          terminal panes once the agent stops (they restart on the next account and resume). Every account shares sessions, settings and skills with
          the default sign-in; only the sign-in is its own.
        </p>
        <button
          type="button"
          disabled={busy}
          onClick={async () => {
            setBusy(true);
            await loadAccounts();
            await refreshUsage().finally(() => setBusy(false));
          }}
          className="flex h-7 shrink-0 items-center gap-1.5 rounded-md px-2.5 text-xs text-fg-muted hover:bg-hover hover:text-fg disabled:opacity-50"
        >
          <RefreshCw size={12} className={busy ? "animate-spin" : undefined} /> Refresh
        </button>
      </div>
      {ACCOUNT_PROVIDERS.map((p) => <ProviderAccounts key={p} provider={p} />)}
    </div>
  );
}
