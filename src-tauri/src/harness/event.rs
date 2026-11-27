//! What backends tell the TUI ([`Ev`]) and what the TUI asks of them ([`Cmd`]). Every backend
//! (ChatGPT, the Claude Code bridge, the Codex bridge) speaks only these, so the UI never knows
//! which one it is talking to.

use std::sync::mpsc::Sender;

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone)]
pub enum Ev {
    /// The backend's own session id (resumable with `--resume`).
    Session(String),
    /// The model actually answering (bridges resolve aliases).
    Model(String),
    /// The backend switched permission mode on its own (e.g. leaving plan mode once the plan is approved).
    Mode(Mode),
    TextDelta { id: String, text: String },
    /// The full text of a message, when the backend sends it whole (replaces the deltas).
    Text { id: String, text: String },
    ReasoningDelta { id: String, text: String },
    /// Reasoning block finished streaming (so the UI can leave "Thinking…" before the next block).
    ReasoningEnd { id: String },
    ToolStart { id: String, name: String, title: String, input: Value },
    ToolOutput { id: String, chunk: String },
    /// `diff` is a unified diff for file edits.
    ToolEnd { id: String, ok: bool, output: String, diff: Option<String> },
    Approval(Approval),
    /// Progress of a task the backend runs on its own (Claude Code's subagents, which keep
    /// running in the background after their tool call returns).
    Task(TaskEv),
    Todos(Vec<Todo>),
    /// Skills the user can invoke with `/name` (the backend's own discovery).
    Skills(Vec<Skill>),
    Usage(Usage),
    Notice(String),
    Error(String),
    TurnStart,
    TurnDone,
    /// The backend process ended; the session can't continue.
    Exited(String),
    /// Answer to [`Cmd::Undo`]: the last user turn is gone from the conversation, with the files
    /// the backend reverted itself (when asked to); or why it couldn't be undone.
    Undone(Result<Vec<String>, String>),
    /// Something the agent should hear about its `bg` tasks (one exited unseen, the user stopped
    /// one): the TUI sends it as the next message (backends with their own queue don't emit it).
    BgNote(String),
    /// The bridge moved to another Claude Code / Codex account (see accounts.rs), by id.
    Account(String),
    /// Out of usage on every account: the session waits until `until` (unix ms) to continue, with
    /// `queued` messages to send after it (see `backend::LimitWait`). `until` is None once it
    /// resumed (or stopped waiting) while messages may still be queued.
    LimitWait { until: Option<i64>, queued: usize },
}

/// One update about a background task; unset fields are unchanged.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TaskEv {
    pub id: String,
    /// The tool call that started it (an Agent card), if any.
    pub tool_id: Option<String>,
    pub title: Option<String>,
    /// What a background task is for (winfer's `bg` tasks the agent described).
    pub description: Option<String>,
    /// Set for subagents (their type, e.g. "general-purpose"); unset for background shells.
    pub agent: Option<String>,
    pub tools: Option<u64>,
    pub last: Option<String>,
    /// "running", or how it ended: "completed", "failed", "stopped", "killed".
    pub status: Option<String>,
    /// File receiving a background shell's output.
    pub output_file: Option<String>,
    pub exit_code: Option<i32>,
    /// The model a subagent runs on, when the backend knows it.
    pub model: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct Approval {
    pub id: String,
    pub tool: String,
    pub title: String,
    pub detail: String,
    pub diff: Option<String>,
    /// Questions the agent asks the user (Claude Code's AskUserQuestion); answered with
    /// [`Cmd::Answer`] instead of a [`Decision`] (declining still goes through `Approve`).
    pub questions: Vec<Question>,
    /// The plan file a plan-mode hand-over (Claude Code's ExitPlanMode) asks to approve, for the
    /// app to show in its file viewer.
    pub plan_file: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Question {
    /// Short label, e.g. "Auth method".
    pub header: String,
    pub question: String,
    /// (label, description)
    pub options: Vec<(String, String)>,
    /// Whether more than one option can be picked.
    pub multi: bool,
}

impl Question {
    /// Questions in the shape Claude Code's AskUserQuestion and winfer code's `ask_user` take:
    /// `{ questions: [{ header, question, options: [{ label, description }], multiSelect }] }`.
    pub fn parse_all(input: &Value) -> Vec<Question> {
        let text = |v: &Value| v.as_str().unwrap_or("").trim().to_string();
        input["questions"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|q| Question {
                header: text(&q["header"]),
                question: text(&q["question"]),
                options: q["options"].as_array().into_iter().flatten().map(|o| (text(&o["label"]), text(&o["description"]))).filter(|(l, _)| !l.is_empty()).collect(),
                multi: q["multiSelect"].as_bool().unwrap_or(false),
            })
            .filter(|q| !q.question.is_empty())
            .collect()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    Allow,
    /// Allow this kind of call for the rest of the session.
    Always,
    Deny,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Usage {
    /// Tokens in the context right now (last request's input).
    pub context: u64,
    pub window: u64,
    pub input: u64,
    /// Session totals.
    pub output: u64,
    pub cached: u64,
    pub cost: Option<f64>,
    /// Output tokens of the running turn (or the last one, between turns), partly estimated
    /// while it streams.
    pub turn: u64,
    /// Input tokens of the running turn (or the last one), summed over its requests.
    pub turn_input: u64,
    /// The part of `turn_input` read from the prompt cache.
    pub turn_cached: u64,
    /// Estimated tokens of the main agent's text streamed so far this turn. Unlike `turn` it
    /// only grows as text arrives, never by an exact count landing (which also covers hidden
    /// reasoning and subagents), so the tok/s rate is taken from it.
    pub turn_streamed: u64,
    /// Milliseconds the main agent spent producing `turn_streamed`, when the backend times it
    /// (0: it doesn't, and the reader paces the count itself). Covers silent reasoning too.
    pub turn_active_ms: u64,
}

/// Counts a turn's output tokens as it runs. Backends report exact counts once per model
/// request; between those, streamed text counts at about 4 characters a token.
#[derive(Debug, Default)]
pub struct TurnTokens {
    exact: u64,
    chars: u64,
    shown: u64,
    /// Every streamed character of the main agent this turn, kept across exact counts.
    paced: u64,
    /// Finished main requests (or a finalized turn window) of a backend that times them:
    /// every token they made (text, reasoning, tool calls) and the time spent making them.
    made: u64,
    making: std::time::Duration,
    /// The running main request (per-request timed mode).
    request: Option<Request>,
    /// Turn-level active LLM window tracking (e.g. Cursor, OpenCode): sums time while the
    /// model is working, excluding client tool execution and approval waits.
    window_open: Option<std::time::Instant>,
    window_active: std::time::Duration,
    window_mode: bool,
    /// Accumulated stream-paced bursts (Muse, and live pacing before `turn_done_window`).
    burst_chars: u64,
    burst_time: std::time::Duration,
    burst: Option<Burst>,
    /// The backend times its output (`request_started`, `first_token`, `window_start`, or `stream_paced`).
    timed: bool,
}

/// A main-agent model request being timed.
#[derive(Debug)]
struct Request {
    start: std::time::Instant,
    /// Whether `start` came from [`TurnTokens::request_started`] (false when opened lazily on
    /// the first streamed delta because the backend didn't see the request start).
    explicit: bool,
    /// `TurnTokens::paced` when the request started (before any deltas).
    paced: u64,
    /// `TurnTokens::paced` right after `first` was stamped: equals `paced` when `first_token`
    /// stamped `first` before text arrived, or `paced + chunk1_chars` when `streamed_at` did.
    first_paced: u64,
    /// Its first and latest generated output (text, reasoning or tool-call arguments).
    first: Option<std::time::Instant>,
    last: Option<std::time::Instant>,
    /// How many pieces of output it streamed.
    pieces: u32,
}

/// One contiguous burst of streamed deltas (gaps <= `Tps::IDLE`), measured from chunk 1's
/// arrival (`first`) to the latest chunk (`last`), counting only characters that arrived
/// strictly after `first` so chunk 1 never inflates the rate and 0ms post-idle dumps are ignored.
#[derive(Debug)]
struct Burst {
    first: std::time::Instant,
    last: std::time::Instant,
    chars: u64,
}

impl TurnTokens {
    pub fn now(&self) -> u64 {
        self.exact + self.chars / 4
    }

    /// Plausible live `(chars, duration)` from the currently open request or burst, excluding
    /// chunk 1's pre-clock characters and holding back ultra-short / implausible bursts.
    fn live_paced(&self) -> (u64, std::time::Duration) {
        if let Some(r) = self.request.as_ref() {
            if let (Some(first), Some(last)) = (r.first, r.last) {
                let dur = last.saturating_duration_since(first);
                let chars = self.paced.saturating_sub(r.first_paced);
                if Self::plausible_span(chars as f64 / 4.0, dur) {
                    return (chars, dur);
                }
            }
            return (0, std::time::Duration::ZERO);
        }
        if let Some(b) = self.burst.as_ref() {
            let dur = b.last.saturating_duration_since(b.first);
            if Self::plausible_span(b.chars as f64 / 4.0, dur) {
                return (b.chars, dur);
            }
        }
        (0, std::time::Duration::ZERO)
    }

    fn plausible_span(tokens: f64, dur: std::time::Duration) -> bool {
        tokens > 0.0 && dur >= std::time::Duration::from_millis(Self::MIN_DECODE_MS) && tokens / dur.as_secs_f64() <= Self::MAX_RATE
    }

    /// The main agent's output so far, in tokens (`Usage::turn_streamed`): the exact counts of
    /// finished requests (or finalized turn window) plus any live/burst streamed estimate, or
    /// (for untimed backends) the raw streamed estimate.
    pub fn streamed_total(&self) -> u64 {
        if !self.timed {
            return self.paced / 4;
        }
        let (live_chars, _) = self.live_paced();
        self.made + (self.burst_chars + live_chars) / 4
    }

    /// The time behind [`Self::streamed_total`], in ms (`Usage::turn_active_ms`); 0 until a
    /// backend times its output with [`Self::request_started`], [`Self::first_token`],
    /// [`Self::window_start`], or [`Self::stream_paced`].
    pub fn active_ms(&self) -> u64 {
        if !self.timed {
            return 0;
        }
        let (_, live_dur) = self.live_paced();
        (self.making + self.burst_time + live_dur).as_millis() as u64
    }

    /// The fastest pace that's taken as generation: a stream faster than this was held back and
    /// delivered at once (e.g. buffered CLI flushes), so its tokens count from the request's
    /// start instead of its first piece (or are dropped if the start wasn't observed).
    /// 1000 tok/s covers fast local/hardware-accelerated models while catching buffered bursts.
    pub const MAX_RATE: f64 = 1000.0;

    /// Minimum decode span (`last - first` or `done - first`) required to time from the first
    /// streamed piece; shorter spans are too easily skewed by pipe/CLI buffering.
    pub const MIN_DECODE_MS: u64 = 100;

    /// The running request's first token arrived at `at` without text (Claude's `message_start`);
    /// opens a request when the backend didn't see one start.
    pub fn first_token(&mut self, at: std::time::Instant) {
        let paced = self.paced;
        let r = self.request.get_or_insert(Request {
            start: at,
            explicit: false,
            paced,
            first_paced: paced,
            first: None,
            last: None,
            pieces: 0,
        });
        if r.first.is_none() {
            r.first = Some(at);
            r.first_paced = paced;
        }
        r.last = Some(at);
        self.timed = true;
    }

    /// Whether a main-agent model request is currently open.
    pub fn has_request(&self) -> bool {
        self.request.is_some()
    }

    /// Whether the running main request has generated anything yet.
    pub fn generating(&self) -> bool {
        self.request.as_ref().is_some_and(|r| r.first.is_some())
    }

    /// A main-agent model request starts.
    pub fn request_started(&mut self, at: std::time::Instant) {
        self.request = Some(Request {
            start: at,
            explicit: true,
            paced: self.paced,
            first_paced: self.paced,
            first: None,
            last: None,
            pieces: 0,
        });
        self.timed = true;
    }

    /// The running main request ended having made `output` tokens. They count over the time from
    /// its first generated token on: processing the prompt before it isn't generating. Unless
    /// `from_start` (or `output` greatly exceeds what was streamed, indicating hidden reasoning
    /// or unstreamed tool-call arguments), or the decode span was too short/fast to be real
    /// generation: then its whole time counts if the request's start was observed (`explicit`),
    /// otherwise it is skipped to avoid inflating tok/s.
    pub fn request_done(&mut self, output: u64, at: std::time::Instant, from_start: bool) {
        let Some(r) = self.request.take() else { return };
        if output == 0 {
            return;
        }
        let streamed_chars = self.paced.saturating_sub(r.paced);
        let first_chars = r.first_paced.saturating_sub(r.paced);
        let streamed_est = streamed_chars / 4;
        // Guard against hidden reasoning or unstreamed tool-call arguments dividing by a short
        // trailing text stream: real BPE ratios stay well below 1.8x the 4-char estimate.
        let unstreamed = output > streamed_est + 50 && (output as f64) > (streamed_est as f64) * 1.8;
        let from_start = from_start || unstreamed;

        // When `r.first` was stamped on chunk 1 arrival (`first_chars > 0`), exclude chunk 1's
        // proportional share of `output` so the numerator matches `at - r.first`.
        let after_first = if first_chars == 0 {
            Some(output)
        } else if streamed_chars > first_chars {
            Some(output.saturating_mul(streamed_chars - first_chars) / streamed_chars)
        } else {
            None
        };

        let (from, counted) = match (r.first, after_first) {
            (Some(first), Some(tokens)) if !from_start && r.pieces >= 3 && Self::plausible_span(tokens as f64, at.saturating_duration_since(first)) => {
                (first, tokens)
            }
            _ if r.explicit => (r.start, output),
            // Implicit request (`start == first`) failed validation or had unstreamed/hidden tokens:
            // falling back to `r.start` would still time from the first delta and inflate tok/s.
            _ => return,
        };
        self.made += counted;
        self.making += at.saturating_duration_since(from);
    }

    /// Starts or resumes an active LLM window for turn-level backends (e.g. Cursor, OpenCode)
    /// that report exact output tokens once at turn end and expose tool/approval pauses.
    pub fn window_start(&mut self, at: std::time::Instant) {
        self.timed = true;
        self.window_mode = true;
        if self.window_open.is_none() {
            self.window_open = Some(at);
        }
    }

    /// Pauses the active LLM window (e.g. while tools run or approvals wait) and closes the
    /// current live stream burst so paused time is excluded.
    pub fn window_pause(&mut self, at: std::time::Instant) {
        if let Some(start) = self.window_open.take() {
            self.window_active += at.saturating_duration_since(start);
        }
        self.close_burst();
    }

    /// Ends a turn-level window backend's turn with exact `output` tokens: pauses any open window
    /// and reconciles `streamed_total()` / `active_ms()` to `output` over total active LLM time.
    /// If `output == 0` or no active window time elapsed, retains any closed stream-paced rate.
    pub fn turn_done_window(&mut self, output: u64, at: std::time::Instant) {
        self.window_pause(at);
        if output > 0 && self.window_active > std::time::Duration::ZERO {
            self.burst = None;
            self.burst_chars = 0;
            self.burst_time = std::time::Duration::ZERO;
            self.made = output;
            self.making = self.window_active;
        }
    }

    /// Backend stream-paced mode (e.g. Muse): paces visible text bursts directly on the backend
    /// without relying on client-side sampling or mixing cold-start / hidden usage.
    pub fn stream_paced(&mut self, text: &str) -> bool {
        self.stream_paced_at(text, std::time::Instant::now())
    }

    /// Like [`Self::stream_paced`], with an explicit timestamp for deterministic testing.
    pub fn stream_paced_at(&mut self, text: &str, at: std::time::Instant) -> bool {
        self.timed = true;
        let chars = text.chars().count() as u64;
        self.paced += chars;
        self.push_burst(chars, at);
        self.count_chars(chars)
    }

    /// Explicitly closes the current stream-paced burst (e.g. when a tool starts running).
    pub fn stream_pause(&mut self) {
        self.close_burst();
    }

    fn push_burst(&mut self, chars: u64, at: std::time::Instant) {
        if chars == 0 {
            return;
        }
        match self.burst.as_mut() {
            Some(b) if at.saturating_duration_since(b.last) <= Tps::IDLE => {
                // Ignore extra chunks that arrive at the exact same instant as chunk 1
                // (an instantaneous buffered flush at burst start), while accumulating
                // all subsequent chunks—including same-instant mid-burst reads—over `last - first`.
                if at > b.first {
                    b.chars += chars;
                    b.last = at;
                }
            }
            Some(_) => {
                self.close_burst();
                self.burst = Some(Burst { first: at, last: at, chars: 0 });
            }
            None => {
                self.burst = Some(Burst { first: at, last: at, chars: 0 });
            }
        }
    }

    fn close_burst(&mut self) {
        let Some(b) = self.burst.take() else { return };
        let dur = b.last.saturating_duration_since(b.first);
        if b.chars > 0 && Self::plausible_span(b.chars as f64 / 4.0, dur) {
            self.burst_chars += b.chars;
            self.burst_time += dur;
        }
    }

    /// Streamed text of a subagent: adds to the count but not to the pace, since subagents
    /// stream alongside the main agent and each other.
    pub fn streamed_sub(&mut self, text: &str) -> bool {
        self.count(text)
    }

    /// The backend's exact count for the turn so far; the estimate it covers is dropped.
    pub fn exact(&mut self, tokens: u64) {
        self.exact = tokens;
        self.chars = 0;
        self.shown = self.now();
    }

    /// Streamed text; true when the count moved enough (~25 tokens) to be worth sending.
    pub fn streamed(&mut self, text: &str) -> bool {
        self.streamed_at(text, std::time::Instant::now())
    }

    pub fn streamed_at(&mut self, text: &str, at: std::time::Instant) -> bool {
        let chars = text.chars().count() as u64;
        if chars == 0 {
            return false;
        }
        if self.window_mode && self.request.is_none() {
            self.paced += chars;
            self.push_burst(chars, at);
            return self.count_chars(chars);
        }
        // Output with no request open (one the backend couldn't see start) times from here.
        if self.timed && self.request.is_none() {
            self.request = Some(Request {
                start: at,
                explicit: false,
                paced: self.paced,
                first_paced: self.paced,
                first: None,
                last: None,
                pieces: 0,
            });
        }
        self.paced += chars;
        if let Some(r) = self.request.as_mut() {
            if r.first.is_none() {
                r.first = Some(at);
                r.first_paced = self.paced;
            }
            r.last = Some(at);
            r.pieces += 1;
        }
        self.count_chars(chars)
    }

    fn count(&mut self, text: &str) -> bool {
        self.count_chars(text.chars().count() as u64)
    }

    fn count_chars(&mut self, chars: u64) -> bool {
        self.chars += chars;
        if self.now() < self.shown + 25 {
            return false;
        }
        self.shown = self.now();
        true
    }
}

/// A turn's output speed in tokens per second, over the time tokens actually streamed in:
/// gaps between counts longer than `IDLE` (tool runs, waiting on the next request's first
/// token) don't count, so the rate is the model's, not the turn's. It's fed the streamed
/// count (`Usage::turn_streamed`), not `Usage::turn`: an exact count landing at a request's
/// end adds hidden reasoning (made during a silent gap) in an instant and would spike it.
#[derive(Debug, Default)]
pub struct Tps {
    last: Option<(std::time::Instant, u64)>,
    tokens: u64,
    active: std::time::Duration,
    /// Set on first sample, rebase, or after an `> IDLE` gap so same-instant (`gap == 0`)
    /// batched lines right after resuming cannot add tokens with `0ms` elapsed.
    resumed: bool,
    /// The backend's own pace (`Usage::turn_active_ms`), which covers silent reasoning.
    reported: Option<(u64, u64)>,
}

impl Tps {
    pub const IDLE: std::time::Duration = std::time::Duration::from_secs(3);

    /// The turn's streamed output count (`Usage::turn_streamed`) at `at`.
    pub fn sample(&mut self, at: std::time::Instant, turn: u64) {
        match self.last {
            None => {
                self.last = Some((at, turn));
                self.resumed = true;
            }
            Some((_, n)) if turn == n => {
                // Unchanged count (e.g. non-streaming usage updates): keep `self.last` intact so
                // elapsed generation time before the next token step is not erased.
            }
            Some((_, n)) if turn < n => {
                // Rebase when count dropped (e.g. exact count lower than estimate).
                self.last = Some((at, turn));
                self.resumed = true;
            }
            Some((t, n)) => {
                let gap = at.saturating_duration_since(t);
                if gap > Self::IDLE {
                    self.last = Some((at, turn));
                    self.resumed = true;
                } else if gap.is_zero() && self.resumed {
                    // Same-instant batched lines right after start/rebase/idle: advance the
                    // baseline without adding tokens at 0ms.
                    self.last = Some((at, turn));
                } else {
                    self.tokens += turn - n;
                    self.active += gap;
                    self.last = Some((at, turn));
                    self.resumed = false;
                }
            }
        }
    }

    /// The backend's count and the ms it took (`Usage::turn_streamed`, `turn_active_ms`): when
    /// it times its requests, its pace is used instead of the sampled one.
    pub fn reported(&mut self, tokens: u64, active_ms: u64) {
        self.reported = (active_ms > 0).then_some((tokens, active_ms));
    }

    /// Tokens per second, once there's enough streaming to tell.
    pub fn rate(&self) -> Option<f64> {
        if let Some((tokens, ms)) = self.reported {
            return (tokens > 0 && ms >= 500).then(|| tokens as f64 * 1000.0 / ms as f64);
        }
        (self.tokens > 0 && self.active >= std::time::Duration::from_millis(500)).then(|| self.tokens as f64 / self.active.as_secs_f64())
    }
}

#[cfg(test)]
mod tps_tests {
    use super::Tps;
    use std::time::{Duration, Instant};

    #[test]
    fn counts_streaming_time_only() {
        let t0 = Instant::now();
        let at = |ms: u64| t0 + Duration::from_millis(ms);
        let mut r = Tps::default();
        r.sample(at(0), 0);
        assert_eq!(r.rate(), None);
        r.sample(at(500), 25);
        r.sample(at(1000), 50);
        assert_eq!(r.rate(), Some(50.0));
        // A 10s tool run, then the next request streams: the gap isn't counted.
        r.sample(at(11_000), 75);
        r.sample(at(11_500), 100);
        assert_eq!(r.rate(), Some(75.0 / 1.5));
        // An exact count lower than the estimate just rebases.
        r.sample(at(12_000), 90);
        r.sample(at(12_500), 115);
        assert_eq!(r.rate(), Some(100.0 / 2.0));
    }

    #[test]
    fn zero_delta_sample_does_not_reset_clock() {
        let t0 = Instant::now();
        let at = |ms: u64| t0 + Duration::from_millis(ms);
        let mut r = Tps::default();
        r.sample(at(1_000), 25);
        // Unchanged turn_streamed mid-generation must not erase the 900ms already spent.
        r.sample(at(1_900), 25);
        r.sample(at(1_950), 50);
        let rate = r.rate().unwrap();
        assert!((rate - 25.0 / 0.95).abs() < 1e-6, "got {rate}");
    }

    #[test]
    fn post_idle_zero_ms_batch_does_not_inflate_rate() {
        let t0 = Instant::now();
        let at = |ms: u64| t0 + Duration::from_millis(ms);
        let mut r = Tps::default();
        r.sample(at(0), 0);
        r.sample(at(500), 25);
        assert_eq!(r.rate(), Some(50.0));
        // 3.5s stall followed by 4 batched lines arriving in the same instant.
        for count in [100, 150, 200, 225] {
            r.sample(at(4_000), count);
        }
        assert_eq!(r.rate(), Some(50.0), "same-instant post-idle batch must not add 0ms tokens");
        // Subsequent real streaming resumes normally.
        r.sample(at(4_500), 250);
        assert_eq!(r.rate(), Some(50.0));
    }

    #[test]
    fn generation_counts_from_the_first_token_without_fencepost_inflation() {
        use super::TurnTokens;
        let t0 = Instant::now();
        let at = |ms: u64| t0 + Duration::from_millis(ms);
        let mut turn = TurnTokens::default();
        assert_eq!(turn.active_ms(), 0, "untimed backends leave pacing to the reader");
        // 3s prompt prefill, then 3x25-token chunks every 400ms (true 62.5 tok/s).
        turn.request_started(at(0));
        for ms in [3_000, 3_400, 3_800] {
            turn.streamed_at(&"x".repeat(100), at(ms));
        }
        // Live excludes chunk 1 (arrived at r.first): 50 tokens over 800ms = 62.5 tok/s.
        assert_eq!((turn.streamed_total(), turn.active_ms()), (50, 800));
        // Exact completion right after chunk 3: chunk 1's proportional share (25 of 75) is excluded.
        turn.request_done(75, at(3_800), false);
        assert_eq!((turn.streamed_total(), turn.active_ms()), (50, 800));
        // Step 2 after a 10s tool run: chunk 1 arriving must NOT spike rate with 0ms added.
        turn.request_started(at(13_800));
        turn.streamed_at(&"x".repeat(200), at(14_800));
        assert_eq!((turn.streamed_total(), turn.active_ms()), (50, 800));
        // Request 2 streamed too little (1 piece): falls back to explicit r.start (1000ms).
        turn.request_done(100, at(14_800), false);
        assert_eq!((turn.streamed_total(), turn.active_ms()), (150, 1_800));
        let mut r = Tps::default();
        r.reported(turn.streamed_total(), turn.active_ms());
        assert!((r.rate().unwrap() - (150.0 / 1.8)).abs() < 1e-6);
    }

    #[test]
    fn first_token_before_text_counts_all_output_tokens() {
        use super::TurnTokens;
        let t0 = Instant::now();
        let at = |ms: u64| t0 + Duration::from_millis(ms);
        let mut turn = TurnTokens::default();
        // Claude's message_start marks first_token before text deltas arrive.
        turn.request_started(at(0));
        turn.first_token(at(2_000));
        for ms in [2_250, 2_500, 2_750, 3_000] {
            turn.streamed_at(&"x".repeat(100), at(ms));
        }
        assert_eq!((turn.streamed_total(), turn.active_ms()), (100, 1_000));
        turn.request_done(100, at(3_000), false);
        assert_eq!((turn.streamed_total(), turn.active_ms()), (100, 1_000));
    }

    #[test]
    fn held_back_and_implicit_bursts_do_not_inflate_rate() {
        use super::TurnTokens;
        let t0 = Instant::now();
        let at = |ms: u64| t0 + Duration::from_millis(ms);
        let mut turn = TurnTokens::default();
        // Explicit start at 0, nothing for 4.65s, then 400 tokens flushed in 350ms (1143 tok/s > MAX_RATE).
        turn.request_started(at(0));
        turn.first_token(at(4_650));
        for ms in [4_750, 4_850, 4_950] {
            turn.streamed_at(&"x".repeat(500), at(ms));
        }
        turn.request_done(400, at(5_000), false);
        assert_eq!((turn.streamed_total(), turn.active_ms()), (400, 5_000));
        // Implicit request (no request_started) with an implausible burst must be DROPPED, not timed from r.start == r.first.
        turn.first_token(at(8_000));
        for ms in [8_002, 8_005, 8_008] {
            turn.streamed_at(&"x".repeat(400), at(ms));
        }
        turn.request_done(400, at(8_010), false);
        assert_eq!((turn.streamed_total(), turn.active_ms()), (400, 5_000));
        // Implicit request with < 3 pieces is also dropped.
        turn.streamed_at(&"x".repeat(100), at(9_000));
        turn.request_done(25, at(9_020), false);
        assert_eq!((turn.streamed_total(), turn.active_ms()), (400, 5_000));
    }

    #[test]
    fn unstreamed_output_falls_back_to_request_start() {
        use super::TurnTokens;
        let t0 = Instant::now();
        let at = |ms: u64| t0 + Duration::from_millis(ms);
        let mut turn = TurnTokens::default();
        // Codex/local: 4s generating unstreamed tool args (500 tok), then 30 text tokens over 400ms.
        turn.request_started(at(0));
        for ms in [4_000, 4_200, 4_400] {
            turn.streamed_at(&"x".repeat(40), at(ms));
        }
        turn.request_done(530, at(4_400), false);
        assert_eq!((turn.streamed_total(), turn.active_ms()), (530, 4_400));
    }

    #[test]
    fn silent_reasoning_counts_over_its_own_time() {
        use super::TurnTokens;
        let t0 = Instant::now();
        let at = |ms: u64| t0 + Duration::from_millis(ms);
        let mut turn = TurnTokens::default();
        // 2s of reasoning nobody saw, then text over 500ms; the request made 225 tokens.
        turn.request_started(at(0));
        for ms in [2_000, 2_250, 2_500] {
            turn.streamed_at(&"x".repeat(50), at(ms));
        }
        turn.request_done(225, at(2_600), true);
        assert_eq!((turn.streamed_total(), turn.active_ms()), (225, 2_600));
        // A second request whose start the backend missed: silent reasoning arrived before the
        // first delta, so timing from the delta would inflate tok/s and must be skipped.
        for ms in [8_000, 8_100, 8_200] {
            turn.streamed_at(&"x".repeat(50), at(ms));
        }
        turn.request_done(800, at(8_300), true);
        assert_eq!((turn.streamed_total(), turn.active_ms()), (225, 2_600));
    }

    #[test]
    fn multibyte_utf8_counts_characters_not_bytes() {
        use super::TurnTokens;
        let mut turn = TurnTokens::default();
        // 40 em-dashes = 40 chars (120 UTF-8 bytes) → 10 est. tokens, not 30.
        turn.streamed(&"—".repeat(40));
        assert_eq!((turn.now(), turn.streamed_total()), (10, 10));
    }

    #[test]
    fn active_window_mode_excludes_tools_and_reconciles_at_turn_done() {
        use super::TurnTokens;
        let t0 = Instant::now();
        let at = |ms: u64| t0 + Duration::from_millis(ms);
        let mut turn = TurnTokens::default();
        turn.window_start(at(0));
        // Live streaming: 3x100-char chunks at 2000, 2400, 2800ms → 50 tok / 800ms (62.5 tok/s).
        for ms in [2_000, 2_400, 2_800] {
            turn.streamed_at(&"x".repeat(100), at(ms));
        }
        assert_eq!((turn.streamed_total(), turn.active_ms()), (50, 800));
        // Tool runs from 3_000 to 10_000ms (excluded from window_active).
        turn.window_pause(at(3_000));
        assert_eq!((turn.streamed_total(), turn.active_ms()), (50, 800));
        turn.window_start(at(10_000));
        // Step 2 chunk 1 does not spike live rate.
        turn.streamed_at(&"x".repeat(100), at(11_000));
        assert_eq!((turn.streamed_total(), turn.active_ms()), (50, 800));
        turn.streamed_at(&"x".repeat(100), at(11_500));
        assert_eq!((turn.streamed_total(), turn.active_ms()), (75, 1_300));
        // Turn ends at 12_000ms with 400 exact output tokens over 3000 + 2000 = 5000ms active window.
        turn.turn_done_window(400, at(12_000));
        assert_eq!((turn.streamed_total(), turn.active_ms()), (400, 5_000));
    }

    #[test]
    fn stream_paced_mode_skips_chunk1_and_zero_ms_dumps_without_dropping_mid_burst() {
        use super::TurnTokens;
        let t0 = Instant::now();
        let at = |ms: u64| t0 + Duration::from_millis(ms);
        let mut turn = TurnTokens::default();
        // Burst 1: 100 chars at t=0 (chunk 1 skipped), then two 50-char chunks at t=200ms, then 100 chars at t=400ms.
        // Total counted after chunk 1: 200 chars (50 tok) over 400ms = 125 tok/s.
        turn.stream_paced_at(&"x".repeat(100), at(0));
        assert_eq!((turn.streamed_total(), turn.active_ms()), (0, 0));
        turn.stream_paced_at(&"x".repeat(50), at(200));
        turn.stream_paced_at(&"x".repeat(50), at(200));
        assert_eq!((turn.streamed_total(), turn.active_ms()), (25, 200));
        turn.stream_paced_at(&"x".repeat(100), at(400));
        assert_eq!((turn.streamed_total(), turn.active_ms()), (50, 400));
        // 5s idle gap, then 4x100-char chunks dumped in the exact same ms (t=5400ms): must not spike!
        for _ in 0..4 {
            turn.stream_paced_at(&"x".repeat(100), at(5_400));
        }
        assert_eq!((turn.streamed_total(), turn.active_ms()), (50, 400));
        // Next real delta 500ms later adds only its own 100 chars (25 tok) over 500ms.
        turn.stream_paced_at(&"x".repeat(100), at(5_900));
        assert_eq!((turn.streamed_total(), turn.active_ms()), (75, 900));
    }

    #[test]
    fn exact_counts_dont_move_the_pace() {
        use super::TurnTokens;
        let t0 = Instant::now();
        let at = |ms: u64| t0 + Duration::from_millis(ms);
        let (mut turn, mut r) = (TurnTokens::default(), Tps::default());
        // 10s of hidden reasoning, then 100 chars (25 tokens) every 250ms: 100 tok/s.
        r.sample(at(0), turn.streamed_total());
        for i in 1..=8 {
            turn.streamed(&"x".repeat(100));
            r.sample(at(10_000 + i * 250), turn.streamed_total());
        }
        // The request ends: its exact count includes 1500 reasoning tokens.
        turn.exact(1700);
        r.sample(at(12_100), turn.streamed_total());
        assert_eq!(turn.now(), 1700);
        assert_eq!(r.rate(), Some(175.0 / 1.75));
        // A subagent's text adds to the count, not to the pace.
        turn.streamed_sub(&"x".repeat(4000));
        assert_eq!((turn.now(), turn.streamed_total()), (2700, 200));
    }

    #[test]
    fn empty_deltas_and_zero_output_requests_do_not_corrupt_timing() {
        use super::TurnTokens;
        let t0 = Instant::now();
        let at = |ms: u64| t0 + Duration::from_millis(ms);
        let mut turn = TurnTokens::default();
        turn.request_started(at(0));
        // Empty deltas must not stamp r.first, advance r.last, or inflate r.pieces.
        for ms in [500, 1_000, 1_500] {
            assert!(!turn.streamed_at("", at(ms)));
        }
        assert!(!turn.generating());
        assert_eq!((turn.streamed_total(), turn.active_ms()), (0, 0));
        // A failed/empty request with output == 0 must not add dead wait time to active_ms.
        turn.request_done(0, at(2_000), false);
        assert_eq!((turn.streamed_total(), turn.active_ms()), (0, 0));
        // Real request after an empty delta still excludes chunk 1 properly.
        turn.request_started(at(2_000));
        turn.streamed_at("", at(2_500));
        for ms in [3_000, 3_400, 3_800] {
            turn.streamed_at(&"x".repeat(100), at(ms));
        }
        assert_eq!((turn.streamed_total(), turn.active_ms()), (50, 800));
        turn.request_done(75, at(3_800), false);
        assert_eq!((turn.streamed_total(), turn.active_ms()), (50, 800));
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Todo {
    pub content: String,
    /// "pending" | "in_progress" | "completed"
    pub status: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelInfo {
    /// `backend/model`, e.g. `claude/opus`.
    pub spec: String,
    pub label: String,
    pub detail: String,
    /// The reasoning efforts the model takes, lowest first: None when the backend doesn't say,
    /// empty when the model has none.
    pub efforts: Option<Vec<String>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Skill {
    pub name: String,
    pub description: String,
}

/// One user turn: text plus attached image files.
#[derive(Debug, Clone, Default)]
pub struct UserMsg {
    pub text: String,
    pub images: Vec<std::path::PathBuf>,
    /// A skill the user invoked explicitly (`/name args`); `text` holds the args.
    pub skill: Option<String>,
}

#[derive(Debug)]
pub enum Cmd {
    Send(UserMsg),
    /// A message for the running turn: the model gets it at its next step (after the tool calls
    /// in flight) instead of after the turn. Without a running turn it's a [`Cmd::Send`].
    Steer(UserMsg),
    Interrupt,
    Approve(String, Decision),
    /// Answers to an approval's questions, one per question in order.
    Answer(String, Vec<String>),
    SetModel(String),
    SetEffort(Option<String>),
    /// Toggle Ultracode multi-agent mode (Claude Code's `ultracode`, Codex's multi-agent + ultra effort, Antigravity's `/swarm`).
    SetUltracode(bool),
    /// The model (None: the main one) and effort subagents run on from now on.
    SetSubagent { model: Option<String>, effort: Option<String> },
    SetMode(Mode),
    Compact,
    /// Context fill (%) for trimming and for full compaction; 0 = off.
    SetCompaction { trim: u8, compact: u8 },
    /// Undo the last user turn: drop it (the user's message and everything after) from the
    /// conversation, answered with [`Ev::Undone`]. With `files`, the backend also reverts the
    /// file changes it tracked (ChatGPT its tools' snapshots, Claude Code its file checkpoints,
    /// Codex the patches it reported); without, the TUI restores them from its own checkpoints.
    Undo { files: bool },
    /// Stop one background task (a subagent), by its `TaskEv` id.
    StopTask(String),
    Shutdown,
}

/// Permission mode, cycled with shift+tab.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Mode {
    /// Ask before edits, commands and network.
    #[default]
    Ask,
    /// File edits go through; commands still ask.
    AutoEdit,
    /// Nothing asks.
    Yolo,
    /// Read-only: plan without changing anything.
    Plan,
    /// Plan without asking: still no file edits, but commands and tools run without approval.
    /// Handing over the plan and questions still come to the user.
    YoloPlan,
}

impl Mode {
    pub fn next(self) -> Self {
        match self {
            Mode::Ask => Mode::AutoEdit,
            Mode::AutoEdit => Mode::Plan,
            Mode::Plan => Mode::YoloPlan,
            Mode::YoloPlan => Mode::Yolo,
            Mode::Yolo => Mode::Ask,
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Mode::Ask => "ask",
            Mode::AutoEdit => "auto-edit",
            Mode::Yolo => "yolo",
            Mode::Plan => "plan",
            Mode::YoloPlan => "yolo-plan",
        }
    }
    /// Plan mode, asking or not.
    pub fn plans(self) -> bool {
        matches!(self, Mode::Plan | Mode::YoloPlan)
    }
}

pub type Events = Sender<Ev>;
