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
use ratatui::crossterm::execute;

use app::{App, Launch, Msg};

pub fn run(launch: Launch, mouse: bool) -> std::io::Result<()> {
    let mut terminal = ratatui::init();
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
    // Title for the terminal tab.
    let _ = write!(out, "\x1b]0;winfer code\x07");

    let (tx, rx) = channel::<Msg>();
    app::spawn_input(tx.clone());
    let mut app = App::new(tx, launch);
    let result = (|| -> std::io::Result<()> {
        // Redraws at most every FRAME for streamed output; keys draw at once.
        const FRAME: Duration = Duration::from_millis(33);
        loop {
            terminal.draw(|f| view::draw(f, &mut app))?;
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
