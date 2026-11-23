//! winfer code as a native chat pane: what a winfer pane shows instead of the terminal TUI when
//! "Use TUI style" is off (port of `src/features/code`). `protocol` and `state` are the data
//! model (no UI, unit tested); `session` runs `winfer code --serve`.

pub mod bg_note;
pub mod commands;
pub mod links;
pub mod mobile;
pub mod toolbody;
pub mod viz;
pub mod viz_svg;
#[cfg(debug_assertions)]
pub mod demo;
pub mod names;
pub mod protocol;
pub mod session;
pub mod state;
