//! The Rust side of `macos.swift`: a boxed channel sender goes over as the callback's context and
//! is freed on the session's `ended` callback.

use super::Event;
use std::ffi::{CStr, c_char, c_void};

type Callback = extern "C" fn(*mut c_void, i32, *const c_char);

unsafe extern "C" {
    fn wf_dictation_start(ctx: *mut c_void, cb: Callback);
    fn wf_dictation_stop();
    fn wf_dictation_cancel();
}

extern "C" fn on_event(ctx: *mut c_void, kind: i32, text: *const c_char) {
    if ctx.is_null() {
        return;
    }
    let text = if text.is_null() { String::new() } else { unsafe { CStr::from_ptr(text) }.to_string_lossy().into_owned() };
    let tx = unsafe { &*(ctx as *const async_channel::Sender<Event>) };
    let ev = match kind {
        0 => Event::Text { text, done: false },
        1 => Event::Text { text, done: true },
        2 => Event::Error(text),
        _ => Event::Ended,
    };
    let ended = ev == Event::Ended;
    let _ = tx.try_send(ev);
    if ended {
        // Swift calls `ended` exactly once per session, after which it drops the context.
        drop(unsafe { Box::from_raw(ctx as *mut async_channel::Sender<Event>) });
    }
}

pub fn start(tx: async_channel::Sender<Event>) {
    let ctx = Box::into_raw(Box::new(tx)) as *mut c_void;
    unsafe { wf_dictation_start(ctx, on_event) };
}

pub fn stop() {
    unsafe { wf_dictation_stop() };
}

pub fn cancel() {
    unsafe { wf_dictation_cancel() };
}
