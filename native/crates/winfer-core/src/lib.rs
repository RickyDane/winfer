//! winfer's GPUI-free core, shared by the desktop app and the headless `winferd`.

pub mod accounts_sync;
pub mod backend;
pub mod connect;
pub mod connect_term;
pub mod hooks;
pub mod layout;
pub mod model;
pub mod request;
pub mod review;
pub mod status;
pub mod terminal;
pub mod theme;
pub mod version;

pub mod code {
    pub mod bg_note;
    pub mod names;
    pub mod protocol;
    pub mod state;
}
