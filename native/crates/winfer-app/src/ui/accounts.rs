//! Several Claude Code / Codex sign-ins (port of `features/accounts`): Settings → Accounts, and
//! failover: a terminal pane whose account runs out of usage restarts on the next account once
//! its agent stops, resumes its session and is told to continue.

use super::commands::Command;
use super::review::small_button;
use super::usage::{Account, left_now, tightest, usage_key};
use super::*;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::tooltip::Tooltip;
use regex::Regex;
use std::collections::HashSet;
use std::sync::LazyLock;
use std::time::Instant;
use winfer_backend::accounts as backend_accounts;

pub const ACCOUNT_PROVIDERS: [&str; 2] = ["claude", "codex"];

pub const CONTINUE_PROMPT: &str = "The previous request stopped because the account ran out of usage. winfer switched to another account: continue where you left off.";

/// What Claude Code and Codex print when the plan's limit is reached.
static LIMIT_TEXT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(usage limit reached|you've hit your (usage )?limit|you have hit your (usage )?limit|5-hour limit reached|weekly limit reached|out of extra usage)").unwrap()
});

pub fn is_limit_message(text: &str) -> bool {
    LIMIT_TEXT.is_match(text)
}

pub fn default_account(provider: &str) -> String {
    format!("{provider}:default")
}

pub fn is_default(id: &str) -> bool {
    id.ends_with(":default")
}

fn now_ms() -> f64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0.0, |d| d.as_millis() as f64)
}

#[derive(Default)]
pub(super) struct Failover {
    /// Panes waiting to move: pane id → when the account resets (if known).
    waiting: HashMap<String, Option<f64>>,
    moving: HashSet<String>,
    /// When a pane's limit message was last checked: matching output is checked once a minute.
    checked_at: HashMap<String, Instant>,
    /// Accounts already reported out, so panes are queued once per run-out.
    exhausted: HashSet<String>,
    /// Label editors on the Accounts page.
    labels: HashMap<String, Entity<InputState>>,
    adding: bool,
    seconds: u64,
}

impl Root {
    fn account_label(&self, id: &str) -> String {
        self.usage.accounts.iter().find(|a| a.id == id).map_or_else(|| if is_default(id) { "Default".into() } else { id.to_string() }, |a| a.label.clone())
    }

    /// Whether an agent has more than its default account to fall back on.
    fn has_alternatives(&self, provider: &str) -> bool {
        self.usage.accounts.iter().filter(|a| a.provider == provider && a.enabled).count() > 1
    }

    fn pane_account(pane: &Pane) -> String {
        pane.account_id.clone().unwrap_or_else(|| default_account(&pane.provider))
    }

    /// Output from a pane: the newest lines may carry the agent's usage-limit message.
    pub(super) fn check_limit_output(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(pane) = self.state.read(cx).pane(id).cloned() else { return };
        if !ACCOUNT_PROVIDERS.contains(&pane.provider.as_str()) || self.failover.moving.contains(id) || self.failover.waiting.contains_key(id) {
            return;
        }
        if self.failover.checked_at.get(id).is_some_and(|t| t.elapsed() < Duration::from_secs(60)) {
            return;
        }
        let Some(tail) = SESSIONS.get(id).map(|s| s.read_text(6)) else { return };
        if !is_limit_message(&tail) {
            return;
        }
        self.failover.checked_at.insert(id.to_string(), Instant::now());
        self.confirm_limit(pane, cx);
    }

    /// The limit message showed: move the pane if its account's limits agree (or can't be read).
    fn confirm_limit(&mut self, pane: Pane, cx: &mut Context<Self>) {
        if !self.has_alternatives(&pane.provider) {
            return;
        }
        let account = Self::pane_account(&pane);
        let key = usage_key(&account);
        self.refresh_one(pane.provider.clone(), (!is_default(&account)).then_some(account.clone()), cx);
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_secs(4)).await;
            let _ = this.update(cx, |this, cx| {
                let now = now_ms();
                let w = tightest(this.usage.snapshots.get(&key), now).cloned();
                if w.as_ref().is_some_and(|w| left_now(w, now) > 1.0) {
                    return;
                }
                this.failover.waiting.insert(pane.id.clone(), w.and_then(|w| w.resets_at));
                this.try_move(&pane.id, cx);
            });
        })
        .detach();
    }

    /// An account's limit was reached: panes working on it move once they stop.
    pub(super) fn on_account_exhausted(&mut self, key: &str, reset_at: Option<f64>, cx: &mut Context<Self>) {
        if !self.failover.exhausted.insert(format!("{key}:{}", reset_at.unwrap_or(0.0))) {
            return;
        }
        let panes: Vec<Pane> = self.state.read(cx).all_panes().filter(|p| {
            ACCOUNT_PROVIDERS.contains(&p.provider.as_str()) && usage_key(&Self::pane_account(p)) == key && p.status == AgentStatus::Working
        }).cloned().collect();
        for pane in panes {
            if self.has_alternatives(&pane.provider) {
                self.failover.waiting.insert(pane.id.clone(), reset_at);
                self.try_move(&pane.id, cx);
            }
        }
    }

    /// Every second: waiting panes move as soon as their agent stops; the accounts list stays fresh.
    pub(super) fn tick_failover(&mut self, cx: &mut Context<Self>) {
        self.failover.seconds += 1;
        if self.failover.seconds % 2 == 0 {
            let ids: Vec<String> = self.failover.waiting.keys().cloned().collect();
            for id in ids {
                self.try_move(&id, cx);
            }
        }
        // The Accounts page picks up a sign-in made in a terminal.
        let open = { let s = self.state.read(cx); s.settings_open && s.settings_page == "accounts" };
        if open && self.failover.seconds % 5 == 0 {
            self.reload_accounts(cx);
        }
    }

    fn try_move(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(pane) = self.state.read(cx).pane(id).cloned() else {
            self.failover.waiting.remove(id);
            return;
        };
        let settled = SESSIONS.get(id).is_none_or(|s| s.output_age_ms() >= 1500);
        if pane.status == AgentStatus::Working || !settled || self.failover.moving.contains(id) {
            return;
        }
        let reset_at = self.failover.waiting.remove(id).flatten();
        self.move_pane(pane, reset_at, cx);
    }

    fn move_pane(&mut self, pane: Pane, reset_at: Option<f64>, cx: &mut Context<Self>) {
        self.failover.moving.insert(pane.id.clone());
        let from = Self::pane_account(&pane);
        let name = self.state.read(cx).provider_name(&pane.provider);
        let provider = pane.provider.clone();
        cx.spawn(async move |this, cx| {
            let (f, p) = (from.clone(), provider.clone());
            let next = backend::background(move || {
                backend_accounts::accounts_mark_exhausted(f.clone(), reset_at.map(|r| r as i64));
                backend_accounts::accounts_pick(p, vec![f])
            })
            .await
            .ok()
            .flatten();
            let resets = |this: &Root| reset_at.map(|r| format!(" (\"{}\" resets in {})", this.account_label(&from), super::usage::duration(r - now_ms()))).unwrap_or_default();
            let Ok(Some(next)) = this.update(cx, |this, cx| {
                this.reload_accounts(cx);
                let Some(next) = next else {
                    let text = format!("{name}: every account is out of usage{}", resets(this));
                    this.toast(text, cx);
                    this.failover.moving.remove(&pane.id);
                    return None;
                };
                let id = pane.id.clone();
                let account = (!is_default(&next)).then(|| next.clone());
                this.update_state(cx, |s| {
                    if let Some(p) = s.pane_mut(&id) {
                        p.account_id = account;
                        p.resume = true;
                        p.prompt = None;
                    }
                });
                this.restart_pane(&id, cx);
                let text = format!("{name}: \"{}\" is out of usage — continuing on \"{}\"", this.account_label(&from), this.account_label(&next));
                this.toast(format!("{text}{}", resets(this)), cx);
                this.update_state(cx, |s| s.log_activity(&pane.workspace_id, text, Some(pane.provider.clone())));
                Some(next)
            }) else {
                return;
            };
            let _ = next;
            // Waits until the restarted agent is up: it printed something, then went quiet (at most 30s).
            let started = Instant::now();
            while started.elapsed() < Duration::from_secs(30) {
                cx.background_executor().timer(Duration::from_millis(500)).await;
                let age = SESSIONS.get(&pane.id).map_or(u64::MAX, |s| s.output_age_ms());
                if age != u64::MAX && age >= 1500 && started.elapsed() > Duration::from_secs(2) {
                    break;
                }
            }
            let _ = this.update(cx, |this, cx| {
                this.type_into(&pane.id, CONTINUE_PROMPT, true, cx);
                this.failover.moving.remove(&pane.id);
            });
        })
        .detach();
    }

    /// A plan ran out: offers a Codex reset, or to carry the work over to an agent with room.
    pub(super) fn limit_reached(&mut self, provider: &str, text: String, resets: i64, cx: &mut Context<Self>) {
        if provider == "codex" && resets > 0 {
            return self.toast_action(text, "Use a reset", Command::RedeemCodexReset, cx);
        }
        let target = self.handoff_target(provider, cx);
        let pane = target.as_ref().and_then(|_| self.pane_to_hand_off(provider, cx));
        match (target, pane) {
            (Some(to), Some(pane)) => {
                let name = self.state.read(cx).provider_name(&to);
                self.toast_with_label(text, format!("Continue in {name}"), Command::HandOff { pane, to }, cx);
            }
            _ => self.toast(text, cx),
        }
    }

    /// The installed agent with the most room left (OpenCode has no plan limits, so it's last).
    fn handoff_target(&self, from: &str, cx: &App) -> Option<String> {
        let now = now_ms();
        let s = self.state.read(cx);
        let mut candidates: Vec<(String, f64)> = s.providers.iter().filter(|p| p.installed && p.id != from && p.id != "shell" && p.command.is_some()).filter_map(|p| {
            let key = self.account_for_start(&p.id).map_or_else(|| p.id.clone(), |a| usage_key(&a));
            let left = tightest(self.usage.snapshots.get(&key), now).map_or(if p.id == "opencode" { 50.0 } else { -1.0 }, |w| left_now(w, now));
            (left >= 10.0).then(|| (p.id.clone(), left))
        }).collect();
        candidates.sort_by(|a, b| b.1.total_cmp(&a.1));
        candidates.into_iter().next().map(|c| c.0)
    }

    /// The pane to hand over: the focused one if it's this agent's, else its most recently busy one.
    fn pane_to_hand_off(&self, provider: &str, cx: &App) -> Option<String> {
        let s = self.state.read(cx);
        let panes: Vec<&Pane> = s.all_panes().filter(|p| p.provider == provider).collect();
        panes.iter().find(|p| s.focused_pane_id.as_deref() == Some(&p.id)).or(panes.iter().find(|p| matches!(p.status, AgentStatus::Working | AgentStatus::NeedsInput))).or(panes.first()).map(|p| p.id.clone())
    }

    /// Opens `to` next to the pane, in the same worktree (or folder), with the task.
    pub(super) fn continue_in(&mut self, id: &str, to: &str, cx: &mut Context<Self>) {
        let Some(pane) = self.state.read(cx).pane(id).cloned() else { return };
        let s = self.state.read(cx);
        let name = s.provider_name(&pane.provider);
        let progress = s.progress_of(id);
        let mut lines = vec![format!("{name} was working on this and hit its usage limit, so you're taking over.")];
        lines.extend(progress.task.as_ref().map(|t| format!("The task: {t}")));
        lines.extend(progress.percent.map(|p| format!("It was about {p}% done{}.", progress.step.as_ref().map_or(String::new(), |s| format!(" (last step: {s})")))));
        lines.push(match &pane.worktree {
            Some(t) => format!("Its work so far is in this folder, on branch {}. Check git status, the diff and recent commits, then continue where it left off.", t.branch),
            None => "Its work so far is in this folder. Check git status and the diff, then continue where it left off.".to_string(),
        });
        let tail = SESSIONS.get(id).map(|s| s.read_text(30)).unwrap_or_default();
        let tail = tail.trim();
        if !tail.is_empty() {
            let chars: Vec<char> = tail.chars().collect();
            lines.push(format!("Its last output:\n{}", chars[chars.len().saturating_sub(3000)..].iter().collect::<String>()));
        }
        let prompt = lines.join("\n\n");
        let to = to.to_string();
        self.update_state(cx, |s| {
            if s.active_workspace_id != pane.workspace_id {
                s.set_active_workspace(&pane.workspace_id);
            }
            let (existing, worktree, cwd) = match pane.worktree.clone() {
                Some(t) => (Some(t), None, None),
                None => (None, Some(false), Some(pane.cwd.clone())),
            };
            s.add_pane(&to, AddPane { workspace_id: Some(pane.workspace_id.clone()), split_from: Some(pane.id.clone()), prompt: Some(prompt), existing_worktree: existing, worktree, cwd, ..Default::default() });
        });
    }

    pub(super) fn redeem_codex_reset(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let result = tauri::async_runtime::spawn(winfer_backend::usage::usage_codex_redeem_reset(None, None)).await.map_err(|e| e.to_string()).and_then(|r| r).and_then(|r| serde_json::to_value(r).map_err(|e| e.to_string()));
            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok(v) => {
                        let code = v["code"].as_str().unwrap_or_default().to_string();
                        this.toast(reset_outcome_text(&code), cx);
                    }
                    Err(e) => this.toast(e, cx),
                }
                this.refresh_one("codex".into(), None, cx);
            });
        })
        .detach();
    }

    pub(super) fn reload_accounts(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let list = backend::background(|| serde_json::to_value(backend_accounts::accounts_list()).unwrap_or_default()).await.unwrap_or_default();
            let parsed: Vec<Account> = serde_json::from_value(list).unwrap_or_default();
            let _ = this.update(cx, |this, cx| {
                this.usage.accounts = parsed;
                cx.notify();
            });
        })
        .detach();
    }

    fn account_call(&mut self, f: impl FnOnce() -> Result<(), String> + Send + 'static, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let result = backend::background(f).await.unwrap_or_else(|e| Err(e.to_string()));
            let _ = this.update(cx, |this, cx| {
                if let Err(e) = result {
                    this.toast(e, cx);
                }
                this.reload_accounts(cx);
            });
        })
        .detach();
    }

    /// Opens a terminal that signs the account in (`claude /login`, `codex login`) in its own config dir.
    fn sign_in(&mut self, id: String, label: String, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let account = id.clone();
            let login = backend::background(move || backend_accounts::accounts_login(account).map(|l| serde_json::to_value(l).unwrap_or_default())).await;
            let _ = this.update(cx, |this, cx| match login.map_err(|e| e.to_string()).and_then(|r| r) {
                Ok(login) => {
                    let command = login["command"].as_str().unwrap_or_default().to_string();
                    let title = format!("Sign in · {label}");
                    this.update_state(cx, |s| {
                        let pane = s.add_pane("shell", AddPane { prompt: Some(command), custom_title: Some(title), ..Default::default() });
                        if !is_default(&id)
                            && let Some(p) = s.pane_mut(&pane)
                        {
                            p.account_id = Some(id.clone());
                        }
                        s.settings_open = false;
                    });
                    this.toast(format!("Sign in to \"{label}\" in the new terminal. Close it when you're done; winfer picks the account up on its own."), cx);
                }
                Err(e) => this.toast(e, cx),
            });
        })
        .detach();
    }

    fn label_input(&mut self, account: &Account, window: &mut Window, cx: &mut Context<Self>) -> Entity<InputState> {
        if let Some(input) = self.failover.labels.get(&account.id) {
            return input.clone();
        }
        let input = cx.new(|cx| InputState::new(window, cx).default_value(account.label.clone()));
        let id = account.id.clone();
        cx.subscribe(&input, move |this, input, event: &InputEvent, cx| {
            if matches!(event, InputEvent::PressEnter { .. } | InputEvent::Blur) {
                let label = input.read(cx).value().trim().to_string();
                let current = this.usage.accounts.iter().find(|a| a.id == id).map(|a| a.label.clone());
                if !label.is_empty() && Some(&label) != current.as_ref() {
                    let id = id.clone();
                    this.account_call(move || backend_accounts::accounts_update(id, Some(label), None), cx);
                }
            }
        })
        .detach();
        self.failover.labels.insert(account.id.clone(), input.clone());
        input
    }

    /// Settings → Accounts: several sign-ins per agent, used in order as each runs out.
    pub(super) fn page_accounts(&mut self, c: Colors, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let now = now_ms();
        let mut page = div().flex().flex_col().gap_6().child(
            div().flex().items_start().gap_3().px_1()
                .child(div().flex_1().min_w_0().text_xs().text_color(c.muted).child("When an account runs out of usage, agents move to the next one in this order and continue their session there: WinferCode chats right away, terminal panes once the agent stops (they restart on the next account and resume). Every account shares sessions, settings and skills with the default sign-in; only the sign-in is its own."))
                .child(small_button(c, "accounts-refresh", "Refresh").on_click(cx.listener(|this, _, _, cx| {
                    this.reload_accounts(cx);
                    this.refresh_all_usage(cx);
                }))),
        );
        for provider in ACCOUNT_PROVIDERS {
            let list: Vec<Account> = self.usage.accounts.iter().filter(|a| a.provider == provider).cloned().collect();
            let ids: Vec<String> = list.iter().map(|a| a.id.clone()).collect();
            let name = self.state.read(cx).provider_name(provider);
            let adding = self.failover.adding;
            let mut rows = div().flex().flex_col().overflow_hidden().rounded_lg().border_1().border_color(c.edge).bg(c.panel);
            for (i, account) in list.iter().enumerate() {
                let label = self.label_input(account, window, cx);
                let w = tightest(self.usage.snapshots.get(&usage_key(&account.id)), now).cloned();
                let out = account.exhausted_until.is_some();
                let mut facts = div().flex().flex_wrap().items_center().gap_x_2().text_size(px(11.0)).text_color(c.subtle);
                facts = facts.child(if is_default(&account.id) { "Default sign-in".to_string() } else { account.dir.split("/accounts/").last().map_or(account.dir.clone(), |d| format!("accounts/{d}")) });
                if !account.signed_in {
                    facts = facts.child(div().text_color(c.warning).child("Not signed in"));
                }
                if out {
                    let id = account.id.clone();
                    let resets = account.exhausted_until.filter(|u| *u > 0.0).map_or(String::new(), |u| format!(" · resets in {}", super::usage::duration(u - now)));
                    facts = facts.child(div().id(SharedString::from(format!("clear-{}", account.id))).text_color(c.danger).cursor_pointer()
                        .tooltip(|window, cx| Tooltip::new("Try this account again now").build(window, cx))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            let id = id.clone();
                            this.account_call(move || {
                                backend_accounts::accounts_clear_exhausted(id);
                                Ok(())
                            }, cx);
                        }))
                        .child(format!("Out of usage{resets}")));
                } else if let Some(w) = &w {
                    let left = left_now(w, now);
                    let color = if left <= 10.0 { c.danger } else if left <= 30.0 { c.warning } else { c.success };
                    facts = facts.child(div().text_color(color).child(format!("{}% of {} left", left.round(), w.label.to_lowercase())));
                }
                let icon = move |id: String, icon: IconName, tip: &'static str, enabled: bool| {
                    div().id(SharedString::from(id)).size_7().flex().items_center().justify_center().rounded_md().text_color(c.subtle)
                        .when(enabled, |d| d.cursor_pointer().hover(move |d| d.bg(c.hover).text_color(c.fg)))
                        .when(!enabled, |d| d.opacity(0.3))
                        .tooltip(move |window, cx| Tooltip::new(tip).build(window, cx))
                        .child(Icon::new(icon).size(px(13.0)))
                };
                let reorder = |by: isize| {
                    let (ids, provider, id) = (ids.clone(), provider.to_string(), account.id.clone());
                    cx.listener(move |this: &mut Self, _: &ClickEvent, _: &mut Window, cx: &mut Context<Self>| {
                        let mut next = ids.clone();
                        let Some(from) = next.iter().position(|x| *x == id) else { return };
                        let to = from as isize + by;
                        if to < 0 || to as usize >= next.len() {
                            return;
                        }
                        next.remove(from);
                        next.insert(to as usize, id.clone());
                        let provider = provider.clone();
                        this.account_call(move || backend_accounts::accounts_reorder(provider, next), cx);
                    })
                };
                let mut buttons = div().flex().items_center().gap_0p5()
                    .child(icon(format!("up-{}", account.id), IconName::ArrowUp, "Use earlier", i > 0).on_click(reorder(-1)))
                    .child(icon(format!("down-{}", account.id), IconName::ArrowDown, "Use later", i + 1 < list.len()).on_click(reorder(1)));
                if !is_default(&account.id) {
                    let (sid, slabel, rid, rlabel) = (account.id.clone(), account.label.clone(), account.id.clone(), account.label.clone());
                    buttons = buttons
                        .child(icon(format!("login-{}", account.id), IconName::LogIn, if account.signed_in { "Sign in again" } else { "Sign in" }, true)
                            .on_click(cx.listener(move |this, _, _, cx| this.sign_in(sid.clone(), slabel.clone(), cx))))
                        .child(icon(format!("remove-{}", account.id), IconName::Trash, "Remove account", true)
                            .on_click(cx.listener(move |this, _, window, cx| {
                                let id = rid.clone();
                                this.confirm("Remove account", format!("Remove \"{rlabel}\"? Its sign-in is deleted; sessions and settings stay."), "Remove", window, cx, move |this, cx| {
                                    this.account_call(move || backend_accounts::accounts_remove(id), cx);
                                });
                            })));
                }
                let (tid, enabled) = (account.id.clone(), account.enabled);
                buttons = buttons.child(div().ml_1().child(self.toggle(SharedString::from(format!("enable-{}", account.id)), enabled, c, cx, move |this, cx| {
                    let id = tid.clone();
                    this.account_call(move || backend_accounts::accounts_update(id, None, Some(!enabled)), cx);
                })));
                rows = rows.child(
                    div().flex().items_center().gap_3().px_3p5().py_2p5().when(i > 0, |d| d)
                        .child(div().w_4().text_center().text_size(px(11.0)).text_color(c.subtle).child((i + 1).to_string()))
                        .child(div().min_w_0().flex_1().flex().flex_col().gap_0p5().child(Input::new(&label)).child(facts))
                        .child(buttons),
                );
            }
            let add_provider = provider.to_string();
            page = page.child(
                div().flex().flex_col().gap_2()
                    .child(div().flex().items_center().gap_2().px_1()
                        .child(div().flex_1().text_size(px(13.0)).text_color(c.fg).child(name))
                        .child(small_button(c, SharedString::from(format!("add-{provider}")), "Add account").when(adding, |d| d.opacity(0.5)).on_click(cx.listener(move |this, _, _, cx| {
                            if this.failover.adding {
                                return;
                            }
                            this.failover.adding = true;
                            let provider = add_provider.clone();
                            cx.spawn(async move |this, cx| {
                                let added = backend::background(move || backend_accounts::accounts_add(provider, String::new()).map(|a| serde_json::to_value(a).unwrap_or_default())).await;
                                let _ = this.update(cx, |this, cx| {
                                    this.failover.adding = false;
                                    match added.map_err(|e| e.to_string()).and_then(|r| r) {
                                        Ok(a) => {
                                            let (id, label) = (a["id"].as_str().unwrap_or_default().to_string(), a["label"].as_str().unwrap_or_default().to_string());
                                            this.reload_accounts(cx);
                                            this.sign_in(id, label, cx);
                                        }
                                        Err(e) => this.toast(e, cx),
                                    }
                                });
                            })
                            .detach();
                        }))))
                    .child(rows),
            );
        }
        page
    }

    pub(super) fn refresh_all_usage(&mut self, cx: &mut Context<Self>) {
        let mut targets: Vec<(String, Option<String>)> = vec![("claude".into(), None), ("codex".into(), None), ("agy".into(), None)];
        targets.extend(self.usage.accounts.iter().filter(|a| a.enabled && a.signed_in && !is_default(&a.id)).map(|a| (a.provider.clone(), Some(a.id.clone()))));
        for (provider, account) in targets {
            self.refresh_one(provider, account, cx);
        }
    }
}

pub fn reset_outcome_text(code: &str) -> String {
    match code {
        "reset" => "Codex limits reset — full again.".into(),
        "nothing_to_reset" => "Nothing to reset: Codex's limits aren't used. The reset was kept.".into(),
        "no_credit" => "No Codex reset available.".into(),
        "already_redeemed" => "That reset was already used.".into(),
        other => format!("Codex answered “{other}”."),
    }
}

#[cfg(test)]
mod tests {
    use super::is_limit_message;

    #[test]
    fn limit_messages_from_both_clis() {
        assert!(is_limit_message("⎿ You've hit your limit · resets 5pm"));
        assert!(is_limit_message("■ Usage limit reached. Try again later."));
        assert!(is_limit_message("Weekly limit reached"));
        assert!(!is_limit_message("Checking the rate limiter code"));
    }
}
