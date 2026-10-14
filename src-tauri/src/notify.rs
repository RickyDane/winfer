//! Native notifications you can act on: when an agent waits, the notification shows its
//! question with Approve / Decline (or, for open questions on macOS, a reply field).
//!
//! Tauri's notification plugin has no actions on desktop, so this uses the platform APIs:
//! NSUserNotification on macOS (mac-notification-sys) and toasts on Windows.

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter};

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ActionNotification {
    pane_id: String,
    title: String,
    body: String,
    /// "approval": a permission prompt (Approve / Decline). "question": anything else.
    kind: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ActionEvent {
    pane_id: String,
    /// "approve", "decline", "reply" or "open" (the notification itself was clicked).
    action: &'static str,
    text: Option<String>,
}

/// Shows the notification; the user's choice arrives as `notification://action`.
#[tauri::command]
pub fn notify_action(app: AppHandle, notification: ActionNotification) -> Result<(), String> {
    // Waiting for the click blocks, so every notification gets its own thread.
    std::thread::spawn(move || {
        if let Some((action, text)) = show(&app, &notification) {
            let _ = app.emit(
                "notification://action",
                ActionEvent {
                    pane_id: notification.pane_id.clone(),
                    action,
                    text,
                },
            );
        }
    });
    Ok(())
}

#[cfg(target_os = "macos")]
fn show(app: &AppHandle, n: &ActionNotification) -> Option<(&'static str, Option<String>)> {
    use mac_notification_sys::{MainButton, Notification, NotificationResponse};
    static INIT: std::sync::Once = std::sync::Once::new();
    INIT.call_once(|| {
        // Attributes notifications to winfer (its icon, its notification settings).
        let _ = mac_notification_sys::set_application(&app.config().identifier);
    });
    let approval = n.kind == "approval";
    let mut note = Notification::new();
    note.title(&n.title).message(&n.body).default_sound();
    if approval {
        note.main_button(MainButton::SingleAction("Approve")).close_button("Decline");
    } else {
        note.main_button(MainButton::Response("Reply…")).close_button("Dismiss");
    }
    match note.send() {
        Ok(NotificationResponse::ActionButton(_)) => Some(("approve", None)),
        Ok(NotificationResponse::CloseButton(_)) if approval => Some(("decline", None)),
        Ok(NotificationResponse::Reply(text)) if !text.trim().is_empty() => Some(("reply", Some(text))),
        Ok(NotificationResponse::Click) => Some(("open", None)),
        Ok(_) => None,
        Err(err) => {
            eprintln!("notification failed: {err}");
            None
        }
    }
}

#[cfg(windows)]
fn show(app: &AppHandle, n: &ActionNotification) -> Option<(&'static str, Option<String>)> {
    use tauri_winrt_notification::Toast;
    // Installed builds register winfer's identifier as its AppUserModelID (like Tauri's own
    // notification plugin); dev builds aren't installed, so they borrow PowerShell's.
    let app_id = if cfg!(debug_assertions) {
        Toast::POWERSHELL_APP_ID.to_string()
    } else {
        app.config().identifier.clone()
    };
    let (tx, rx) = std::sync::mpsc::channel();
    let mut toast = Toast::new(&app_id).title(&n.title).text1(&n.body);
    if n.kind == "approval" {
        toast = toast.add_button("Approve", "approve").add_button("Decline", "decline");
    }
    let result = toast
        .on_activated(move |action| {
            let _ = tx.send(match action.as_deref() {
                Some("approve") => "approve",
                Some("decline") => "decline",
                _ => "open",
            });
            Ok(())
        })
        .show();
    if let Err(err) = result {
        eprintln!("notification failed: {err}");
        return None;
    }
    // Toasts sit in the Action Center; wait a while for the user, then let go.
    rx.recv_timeout(std::time::Duration::from_secs(60 * 60))
        .ok()
        .map(|action| (action, None))
}

#[cfg(not(any(target_os = "macos", windows)))]
fn show(_: &AppHandle, _: &ActionNotification) -> Option<(&'static str, Option<String>)> {
    None
}
