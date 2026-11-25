//! winfer's GPUI-free core, shared by the desktop app and the headless `winferd`.

pub mod hooks;
pub mod layout;
pub mod request;

pub mod code {
    pub mod bg_note;
    pub mod names;
    pub mod protocol;
    pub mod state;
}
