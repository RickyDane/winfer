//! winfer code as a native chat pane: what a winfer pane shows instead of the terminal TUI when
//! "Use TUI style" is off (port of `src/features/code`). `protocol` and `state` are the data
//! model (no UI, unit tested); `session` runs `winfer code --serve`.

pub use winfer_core::code::bg_note;
pub mod commands;
pub mod links;
pub use winfer_core::code::mobile;
pub mod toolbody;
pub mod viz;
pub mod viz_svg;
#[cfg(debug_assertions)]
pub mod demo;
pub use winfer_core::code::names;
pub use winfer_core::code::protocol;
pub mod session;
pub use winfer_core::code::state;
