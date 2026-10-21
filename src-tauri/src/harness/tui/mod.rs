//! The full-screen terminal UI.

pub mod anim;
pub mod app;
pub mod clipboard;
pub mod composer;
pub mod markdown;
pub mod picker;
pub mod selection;
#[cfg(test)]
mod snapshot;
pub mod theme;
pub mod transcript;
pub mod view;

use std::io::Write;
use std::sync::mpsc::{RecvTimeoutError, channel};
use std::time::Duration;

use ratatui::crossterm::event::{
    DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture, KeyboardEnhancementFlags,
    PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use ratatui::crossterm::cursor::Hide;
use ratatui::crossterm::terminal::{BeginSynchronizedUpdate, EndSynchronizedUpdate};
use ratatui::crossterm::{execute, queue};

use app::{App, Launch, Msg};

/// Collects terminal output and writes each frame to stdout in one go. ratatui flushes several
/// times per frame (cells, then show cursor, then move it), so the terminal could show the cursor
/// jumping across the screen mid-frame; on Windows, ConPTY renders such in-between states, which
/// made the cursor flicker during animations. While `holding`, flushes are deferred to [`commit`].
///
/// [`commit`]: FrameOut::commit
/// Cloning shares the buffer, so the draw loop can hold and commit what ratatui writes.
#[derive(Clone, Default)]
struct FrameOut(std::rc::Rc<std::cell::RefCell<Frame>>);

#[derive(Default)]
struct Frame {
    buf: Vec<u8>,
    holding: bool,
}

impl FrameOut {
    fn hold(&self, holding: bool) {
        self.0.borrow_mut().holding = holding;
    }

    fn commit(&self) -> std::io::Result<()> {
        let mut frame = self.0.borrow_mut();
        let mut out = std::io::stdout().lock();
        let result = out.write_all(&frame.buf).and_then(|_| out.flush());
        frame.buf.clear();
        result
    }
}

impl Write for FrameOut {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.borrow_mut().buf.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        if self.0.borrow().holding { Ok(()) } else { self.commit() }
    }
}

type Term = ratatui::Terminal<ratatui::backend::CrosstermBackend<FrameOut>>;

/// Draws one frame as a single synchronized write, the cursor hidden until it's placed (ratatui
/// shows it again at the composer, or keeps it hidden).
fn draw(terminal: &mut Term, out: &mut FrameOut, app: &mut App) -> std::io::Result<()> {
    out.hold(true);
    let _ = queue!(out, BeginSynchronizedUpdate, Hide);
    let drawn = terminal.draw(|f| view::draw(f, app)).map(|_| ());
    let _ = queue!(out, EndSynchronizedUpdate);
    out.hold(false);
    drawn.and(out.commit())
}

/// The terminal title for a session: its topic without control characters, else "winfer code".
fn window_title(topic: &str) -> String {
    let clean: String = topic.chars().map(|c| if c.is_control() { ' ' } else { c }).collect();
    let clean = clean.trim();
    if clean.is_empty() { "winfer code".into() } else { clean.into() }
}

/// Takes the terminal back if a child with job control (an interactive shell, say) made itself the
/// foreground group and died without handing it back: input reads would fail forever otherwise.
fn reclaim_tty() {
    #[cfg(unix)]
    // SAFETY: plain syscalls on stdin; SIGTTOU is ignored around tcsetpgrp so a background caller
    // isn't stopped by it, then restored.
    unsafe {
        let me = libc::getpgrp();
        let fg = libc::tcgetpgrp(0);
        if fg > 0 && fg != me {
            let old = libc::signal(libc::SIGTTOU, libc::SIG_IGN);
            libc::tcsetpgrp(0, me);
            libc::signal(libc::SIGTTOU, old);
        }
    }
}

pub fn run(launch: Launch, mouse: bool) -> std::io::Result<()> {
    // Raw mode, the alternate screen and a panic hook that restores the terminal; its stdout
    // backend is swapped for one that writes whole frames.
    drop(ratatui::init());
    let mut frames = FrameOut::default();
    let mut terminal = Term::new(ratatui::backend::CrosstermBackend::new(frames.clone()))?;
    // Match the terminal's theme before anything else reads stdin.
    let (bg, fg) = theme::detect(std::time::Duration::from_millis(250));
    theme::init(bg, fg);
    let mut out = std::io::stdout();
    let enhanced = ratatui::crossterm::terminal::supports_keyboard_enhancement().unwrap_or(false);
    let _ = execute!(out, EnableBracketedPaste);
    if mouse {
        let _ = execute!(out, EnableMouseCapture);
    }
    if enhanced {
        let _ = execute!(out, PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES));
    }
    let (tx, rx) = channel::<Msg>();
    app::spawn_input(tx.clone());
    let mut app = App::new(tx, launch);
    // Title for the terminal tab: the session's topic once there is one (winfer names the pane
    // after it, and on Windows the console's own title would otherwise be cmd.exe's path).
    let mut titled: Option<String> = None;
    let result = (|| -> std::io::Result<()> {
        // Redraws at most every FRAME for streamed output; keys draw at once.
        const FRAME: Duration = Duration::from_millis(33);
        loop {
            reclaim_tty();
            draw(&mut terminal, &mut frames, &mut app)?;
            if titled.as_deref() != Some(app.saved.title.as_str()) {
                titled = Some(app.saved.title.clone());
                let _ = write!(out, "\x1b]0;{}\x07", window_title(&app.saved.title));
                let _ = out.flush();
            }
            let drawn = std::time::Instant::now();
            if app.quit {
                break;
            }
            // ~25 fps while something moves: smooth enough for the spinners and sweeps, and light on
            // the terminal, which has to draw every frame (several panes may animate at once).
            let wait = if app.animating() { Duration::from_millis(40) } else { Duration::from_millis(1000) };
            match rx.recv_timeout(wait) {
                Ok(m) => {
                    let mut key = matches!(m, Msg::Term(_));
                    app::drain(&mut app, &rx, m);
                    // Coalesce a burst of agent events (streamed text, tool output) into one frame.
                    while !key && !app.quit && drawn.elapsed() < FRAME {
                        match rx.recv_timeout(FRAME - drawn.elapsed()) {
                            Ok(m) => {
                                key = matches!(m, Msg::Term(_));
                                app::drain(&mut app, &rx, m);
                            }
                            Err(RecvTimeoutError::Timeout) => break,
                            Err(RecvTimeoutError::Disconnected) => return Ok(()),
                        }
                    }
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => break,
            }
            app.flush_usage();
            // The wait for a background report turn runs out without an event.
            app.report_status();
            if app.quit {
                break;
            }
        }
        Ok(())
    })();

    if let Some(h) = app.handle.take() {
        h.send(crate::harness::event::Cmd::Shutdown);
    }
    if enhanced {
        let _ = execute!(out, PopKeyboardEnhancementFlags);
    }
    if mouse {
        let _ = execute!(out, DisableMouseCapture);
    }
    let _ = execute!(out, DisableBracketedPaste);
    ratatui::restore();
    if !app.saved.transcript.is_empty() {
        println!("Resume this session with: winfer code --resume {}", app.saved.id);
    }
    result
}
