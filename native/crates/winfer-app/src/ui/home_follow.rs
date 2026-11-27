//! The panes on paired machines the conductor started or messaged (chores there): it hears when
//! they finish, need input or hit an error, as it does for its tasks' agents.

use super::connect_ui::{Mirror, RemotePane};
use super::home::{HomeTask, TaskMode, Watch, pane_event};
use super::*;
use std::collections::HashSet;
use std::time::{Duration, Instant};

/// How long a call may take to be answered, and a started pane to show up in the mirror.
const PATIENCE: Duration = Duration::from_secs(120);

#[derive(Default)]
pub(super) struct Follows {
    /// Tool calls on their way, by MCP call id: what to follow once one worked.
    asked: HashMap<String, (Asked, Instant)>,
    /// Started panes the mirror doesn't show yet.
    starting: Vec<(Starting, Instant)>,
    /// By (peer, pane id).
    panes: HashMap<(String, String), Watch>,
}

enum Asked {
    Spawn(Starting),
    Send { peer: String, pane: String, watch: Watch },
}

struct Starting {
    peer: String,
    provider: String,
    /// The machine's panes before the call.
    before: HashSet<String>,
    /// The new pane's number there, from the answer.
    number: Option<usize>,
}

impl Starting {
    /// The new pane in the mirror: the one at the answered number, else the only new one of its agent.
    fn find(&self, mirror: &Mirror) -> Option<String> {
        let fits = |p: &RemotePane| !self.before.contains(&p.id) && (p.provider == self.provider || self.provider == "default");
        let order = mirror.ordered();
        if let Some(p) = self.number.and_then(|n| order.get(n.checked_sub(1)?)).filter(|p| fits(p)) {
            return Some(p.id.clone());
        }
        let mut new = order.iter().filter(|p| fits(p));
        new.next().filter(|_| new.next().is_none()).map(|p| p.id.clone())
    }
}

impl Follows {
    /// `call` starts a `provider` pane on `peer`.
    pub fn spawning(&mut self, call: &str, peer: &str, provider: &str, mirror: Option<&Mirror>) {
        let before = mirror.map(|m| m.panes.iter().map(|p| p.id.clone()).collect()).unwrap_or_default();
        let starting = Starting { peer: peer.to_string(), provider: provider.to_string(), before, number: None };
        self.asked.insert(call.to_string(), (Asked::Spawn(starting), Instant::now()));
    }

    /// `call` types into `pane` on `peer`.
    pub fn sending(&mut self, call: &str, peer: &str, pane: &RemotePane) {
        let asked = Asked::Send { peer: peer.to_string(), pane: pane.id.clone(), watch: Watch::new(pane.status) };
        self.asked.insert(call.to_string(), (asked, Instant::now()));
    }

    /// The machine answered `call` (one of ours or not).
    pub fn answered(&mut self, call: &str, ok: bool, text: &str) {
        let Some((asked, _)) = self.asked.remove(call).filter(|_| ok) else { return };
        match asked {
            Asked::Spawn(mut s) => {
                // "Started claude as mac:3"
                s.number = text.rsplit_once(':').and_then(|(_, n)| n.trim().parse().ok());
                self.starting.push((s, Instant::now()));
            }
            Asked::Send { peer, pane, watch } => {
                self.panes.entry((peer, pane)).or_insert(watch);
            }
        }
    }

    /// One tick: the events to tell the conductor. `machine` names a peer.
    pub fn tick(&mut self, mirrors: &HashMap<String, Mirror>, tasks: &[HomeTask], machine: impl Fn(&str) -> String) -> Vec<String> {
        self.asked.retain(|_, (_, at)| at.elapsed() < PATIENCE);
        let panes = &mut self.panes;
        self.starting.retain(|(s, at)| {
            let Some(mirror) = mirrors.get(&s.peer).filter(|m| m.loaded) else { return at.elapsed() < PATIENCE };
            let Some(id) = s.find(mirror) else { return at.elapsed() < PATIENCE };
            let status = mirror.panes.iter().find(|p| p.id == id).map_or(AgentStatus::Idle, |p| p.status);
            panes.entry((s.peer.clone(), id)).or_insert_with(|| Watch::started(status));
            false
        });
        let mut events = Vec::new();
        panes.retain(|(peer, id), watch| {
            // Nothing heard from the machine (yet): wait.
            let Some(mirror) = mirrors.get(peer).filter(|m| m.loaded) else { return true };
            let Some(pane) = mirror.panes.iter().find(|p| p.id == *id) else { return false };
            // A remote task's orchestrator: its task reports it already.
            let orchestrates = |t: &HomeTask| {
                t.mode == TaskMode::Remote && t.stage.open() && t.machine.as_ref() == Some(peer) && (t.remote_pane.as_ref() == Some(id) || pane.title.starts_with(&format!("{} ·", t.title())))
            };
            if tasks.iter().any(orchestrates) {
                return false;
            }
            let Some(what) = watch.tick(pane.status) else { return true };
            let name = machine(peer);
            let place = match mirror.ordered().iter().position(|p| p.id == *id) {
                Some(i) => format!("{name}:{}", i + 1),
                None => format!("An archived pane on {name}"),
            };
            let agent = mirror.providers.iter().find(|p| p.0 == pane.provider).map_or(pane.provider.as_str(), |p| p.1.as_str());
            let message = match what {
                "finished" => pane.reply.as_ref().or(pane.message.as_ref()),
                "needs input" => pane.request_text.as_ref().or(pane.message.as_ref()),
                _ => pane.message.as_ref(),
            };
            events.push(pane_event(None, &place, agent, pane.branch.as_deref(), what, message.map(String::as_str)));
            true
        });
        events
    }
}

impl Root {
    pub(super) fn tick_follows(&mut self, cx: &mut Context<Self>) {
        let name = |peer: &str| crate::connect::peer_name(peer).unwrap_or_else(|| "the other machine".into());
        let events = self.home.follows.tick(&self.connect.mirrors, &self.home.tasks, name);
        if !events.is_empty() {
            self.queue_for(crate::voice::CONDUCTOR, events.join("\n"), cx);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{AgentStatus, Follows, HomeTask, Mirror, RemotePane, TaskMode};
    use std::collections::HashMap;

    fn pane(id: &str, provider: &str, status: AgentStatus) -> RemotePane {
        RemotePane { id: id.into(), title: "web".into(), workspace: "web".into(), workspace_id: "w".into(), archived: false, cwd: String::new(), provider: provider.into(), status, message: None, request_kind: None, request_text: None, reply: None, branch: None }
    }

    fn mirrors(panes: Vec<RemotePane>) -> HashMap<String, Mirror> {
        let mirror = Mirror { panes, workspaces: vec![("w".into(), "web".into())], providers: vec![("winfer".into(), "WinferCode".into())], loaded: true, ..Default::default() };
        HashMap::from([("peer".to_string(), mirror)])
    }

    fn ticks(f: &mut Follows, m: &HashMap<String, Mirror>, tasks: &[HomeTask]) -> Vec<String> {
        (0..4).flat_map(|_| f.tick(m, tasks, |_| "mac".into())).collect()
    }

    #[test]
    fn a_started_pane_reports_once_it_finishes() {
        let mut f = Follows::default();
        let old = mirrors(vec![pane("p1", "winfer", AgentStatus::Idle)]);
        f.spawning("c", "peer", "winfer", old.get("peer"));
        f.answered("c", true, "Started winfer as mac:2");
        // Not in the mirror yet.
        assert!(ticks(&mut f, &old, &[]).is_empty());
        let working = mirrors(vec![pane("p1", "winfer", AgentStatus::Idle), pane("p2", "winfer", AgentStatus::Working)]);
        assert!(ticks(&mut f, &working, &[]).is_empty());
        // A pane before it closed: it's followed by id, and named by its number now.
        let mut done = pane("p2", "winfer", AgentStatus::Done);
        done.reply = Some("Updated the app".into());
        assert_eq!(ticks(&mut f, &mirrors(vec![done]), &[]), ["[winfer] mac:1 (WinferCode) finished: Updated the app."]);
    }

    #[test]
    fn a_pane_already_done_when_first_seen_still_reports() {
        let mut f = Follows::default();
        f.spawning("c", "peer", "winfer", None);
        f.answered("c", true, "Started it (on mac; see list_remote_panes for its address)");
        let m = mirrors(vec![pane("p1", "winfer", AgentStatus::Done)]);
        assert_eq!(ticks(&mut f, &m, &[]), ["[winfer] mac:1 (WinferCode) finished."]);
    }

    #[test]
    fn a_messaged_pane_reports_questions_and_errors_and_goes_with_its_pane() {
        let mut f = Follows::default();
        f.sending("c", "peer", &pane("p1", "claude", AgentStatus::Done));
        f.answered("c", true, "Sent.");
        assert!(ticks(&mut f, &mirrors(vec![pane("p1", "claude", AgentStatus::Done)]), &[]).is_empty());
        let mut asks = pane("p1", "claude", AgentStatus::NeedsInput);
        asks.request_text = Some("Run apt upgrade?".into());
        assert_eq!(ticks(&mut f, &mirrors(vec![asks]), &[]), ["[winfer] mac:1 (claude) needs input: Run apt upgrade?"]);
        assert_eq!(ticks(&mut f, &mirrors(vec![pane("p1", "claude", AgentStatus::Error)]), &[]), ["[winfer] mac:1 (claude) hit an error."]);
        // An unheard-from machine keeps the watch; a closed pane drops it.
        ticks(&mut f, &HashMap::new(), &[]);
        assert_eq!(f.panes.len(), 1);
        ticks(&mut f, &mirrors(Vec::new()), &[]);
        assert!(f.panes.is_empty());
    }

    #[test]
    fn failed_calls_and_task_orchestrators_are_not_followed() {
        let mut f = Follows::default();
        f.sending("c", "peer", &pane("p1", "winfer", AgentStatus::Working));
        f.answered("c", false, "No such pane.");
        assert!(f.panes.is_empty());
        f.sending("d", "peer", &pane("p1", "winfer", AgentStatus::Working));
        f.answered("d", true, "Sent.");
        let task = HomeTask { mode: TaskMode::Remote, machine: Some("peer".into()), remote_pane: Some("p1".into()), ..Default::default() };
        assert!(ticks(&mut f, &mirrors(vec![pane("p1", "winfer", AgentStatus::Done)]), &[task]).is_empty());
        assert!(f.panes.is_empty());
    }
}
