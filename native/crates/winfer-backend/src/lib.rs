//! The Tauri app's backend, compiled unchanged from `../src-tauri/src`. Only `lib.rs`, `menu.rs`,
//! `undock.rs` are left out: they set up Tauri itself, which the GPUI app replaces.
#![allow(dead_code, unused_imports, unused_variables)]

#[path = "../../../../src-tauri/src/accounts.rs"] pub mod accounts;
#[path = "../../../../src-tauri/src/android.rs"] pub mod android;
#[path = "../../../../src-tauri/src/app_updates.rs"] pub mod app_updates;
#[path = "../../../../src-tauri/src/browser/mod.rs"] pub mod browser;
#[path = "../../../../src-tauri/src/chatgpt.rs"] pub mod chatgpt;
#[path = "../../../../src-tauri/src/context_breakdown.rs"] pub mod context_breakdown;
#[path = "../../../../src-tauri/src/editors.rs"] pub mod editors;
#[path = "../../../../src-tauri/src/env.rs"] pub mod env;
#[path = "../../../../src-tauri/src/files.rs"] pub mod files;
#[path = "../../../../src-tauri/src/harness/mod.rs"] pub mod harness;
#[path = "../../../../src-tauri/src/hooks.rs"] pub mod hooks;
#[path = "../../../../src-tauri/src/llm.rs"] pub mod llm;
#[path = "../../../../src-tauri/src/local_llm.rs"] pub mod local_llm;
#[path = "../../../../src-tauri/src/mcp.rs"] pub mod mcp;
#[path = "../../../../src-tauri/src/mobile/mod.rs"] pub mod mobile;
#[path = "../../../../src-tauri/src/notify.rs"] pub mod notify;
#[path = "../../../../src-tauri/src/plugins.rs"] pub mod plugins;
#[path = "../../../../src-tauri/src/skills.rs"] pub mod skills;
#[path = "../../../../src-tauri/src/provider_updates.rs"] pub mod provider_updates;
#[path = "../../../../src-tauri/src/providers.rs"] pub mod providers;
#[path = "../../../../src-tauri/src/pty.rs"] pub mod pty;
#[path = "../../../../src-tauri/src/review.rs"] pub mod review;
#[path = "../../../../src-tauri/src/store.rs"] pub mod store;
#[path = "../../../../src-tauri/src/tool_stats.rs"] pub mod tool_stats;
#[path = "../../../../src-tauri/src/usage.rs"] pub mod usage;
#[path = "../../../../src-tauri/src/usage_history.rs"] pub mod usage_history;
#[path = "../../../../src-tauri/src/vault.rs"] pub mod vault;
#[path = "../../../../src-tauri/src/viz.rs"] pub mod viz;
#[path = "../../../../src-tauri/src/realtime.rs"] pub mod realtime;
#[path = "../../../../src-tauri/src/chatgpt_voice.rs"] pub mod chatgpt_voice;
#[path = "../../../../src-tauri/src/diagnostics.rs"] pub mod diagnostics;
#[path = "../../../../src-tauri/src/transfer.rs"] pub mod transfer;
#[path = "../../../../src-tauri/src/worktree.rs"] pub mod worktree;
