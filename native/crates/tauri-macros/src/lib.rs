//! `#[tauri::command]` for the backend modules compiled here: commands are plain functions the UI
//! calls directly, so the attribute leaves them as they are.
use proc_macro::TokenStream;

#[proc_macro_attribute]
pub fn command(_args: TokenStream, item: TokenStream) -> TokenStream {
    item
}
