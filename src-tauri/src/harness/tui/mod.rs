//! The full-screen terminal UI.

pub mod anim;
pub mod app;
pub mod clipboard;
pub mod composer;
pub mod markdown;
pub mod picker;
pub mod selection;
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
        loop {
            terminal.draw(|f| view::draw(f, &mut app))?;
            if app.quit {
                break;
            }
            let wait = if app.animating() { Duration::from_millis(16) } else { Duration::from_millis(1000) };
            match rx.recv_timeout(wait) {
                Ok(m) => app::drain(&mut app, &rx, m),
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
