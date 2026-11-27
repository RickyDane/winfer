//! Live voice: a spoken conversation with an OpenAI realtime model that operates winfer
//! (port of `features/voice/live`). The model's tools live in `ui/voice_tools.rs`; this side is
//! the audio and the connection, with no UI.

pub mod client;
pub mod io;
pub mod protocol;
pub mod resample;

/// Home's orchestrators run under made-up pane ids that no workspace lists (see `ui/home.rs`):
/// one conductor for the whole app, and one sub-orchestrator per forked task.
pub const CONDUCTOR: &str = "home-orch";
pub const TASK_PREFIX: &str = "home-task-";

/// Which Home orchestrator a pane id is, if it's one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HomePane<'a> {
    Conductor,
    Task(&'a str),
}

pub fn home_pane(pane_id: &str) -> Option<HomePane<'_>> {
    if pane_id == CONDUCTOR {
        return Some(HomePane::Conductor);
    }
    pane_id.strip_prefix(TASK_PREFIX).filter(|t| !t.is_empty()).map(HomePane::Task)
}

pub fn task_pane(task_id: &str) -> String {
    format!("{TASK_PREFIX}{task_id}")
}

/// Whether `pane_id` is one of Home's orchestrators.
pub fn is_orchestrator(pane_id: &str) -> bool {
    home_pane(pane_id).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn home_pane_ids() {
        assert_eq!(home_pane(CONDUCTOR), Some(HomePane::Conductor));
        assert_eq!(home_pane(&task_pane("t7")), Some(HomePane::Task("t7")));
        assert_eq!(home_pane("home-task-"), None);
        assert_eq!(home_pane("p-1"), None);
    }
}
