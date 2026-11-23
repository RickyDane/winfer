//! Dictation on Windows: a continuous `Windows.Media.SpeechRecognition` session with the dictation
//! grammar, on a thread of its own. Segments it finishes are kept; the current hypothesis follows them.

use super::Event;
use parking_lot::Mutex;
use std::sync::Arc;
use std::sync::mpsc;
use windows::Foundation::TypedEventHandler;
use windows::Media::SpeechRecognition::{
    SpeechContinuousRecognitionCompletedEventArgs, SpeechContinuousRecognitionResultGeneratedEventArgs, SpeechContinuousRecognitionSession,
    SpeechRecognitionHypothesisGeneratedEventArgs, SpeechRecognitionResultStatus, SpeechRecognizer,
};
use windows::Win32::System::WinRT::{RO_INIT_MULTITHREADED, RoInitialize};

enum Ctl {
    Stop,
    Cancel,
}

static CURRENT: Mutex<Option<mpsc::Sender<Ctl>>> = Mutex::new(None);

/// HRESULT when "Online speech recognition" is off in Settings → Privacy → Speech.
const PRIVACY_DECLINED: i32 = 0x80045509_u32 as i32;

pub fn start(tx: async_channel::Sender<Event>) {
    let (ctl_tx, ctl_rx) = mpsc::channel();
    if let Some(prev) = CURRENT.lock().replace(ctl_tx) {
        let _ = prev.send(Ctl::Cancel);
    }
    std::thread::spawn(move || {
        if let Err(e) = run(&tx, ctl_rx) {
            let msg = if e.code().0 == PRIVACY_DECLINED {
                "Turn on Online speech recognition under Windows Settings → Privacy & security → Speech.".to_string()
            } else {
                format!("Dictation failed: {}", e.message())
            };
            let _ = tx.try_send(Event::Error(msg));
        }
        let _ = tx.try_send(Event::Ended);
    });
}

pub fn stop() {
    if let Some(ctl) = CURRENT.lock().take() {
        let _ = ctl.send(Ctl::Stop);
    }
}

pub fn cancel() {
    if let Some(ctl) = CURRENT.lock().take() {
        let _ = ctl.send(Ctl::Cancel);
    }
}

#[derive(Default)]
struct Heard {
    done: Vec<String>,
    hypothesis: String,
}

impl Heard {
    fn text(&self, with_hypothesis: bool) -> String {
        let mut parts: Vec<&str> = self.done.iter().map(String::as_str).collect();
        if with_hypothesis && !self.hypothesis.is_empty() {
            parts.push(&self.hypothesis);
        }
        parts.join(" ")
    }
}

fn run(tx: &async_channel::Sender<Event>, ctl: mpsc::Receiver<Ctl>) -> windows::core::Result<()> {
    unsafe { RoInitialize(RO_INIT_MULTITHREADED)? };
    let recognizer = SpeechRecognizer::new()?;
    let compiled = recognizer.CompileConstraintsAsync()?.join()?;
    if compiled.Status()? != SpeechRecognitionResultStatus::Success {
        let _ = tx.try_send(Event::Error("Windows couldn't prepare speech recognition for this language.".into()));
        return Ok(());
    }
    let heard = Arc::new(Mutex::new(Heard::default()));
    let (done_tx, done_rx) = mpsc::channel::<()>();
    {
        let (heard, tx) = (heard.clone(), tx.clone());
        recognizer.HypothesisGenerated(&TypedEventHandler::<SpeechRecognizer, SpeechRecognitionHypothesisGeneratedEventArgs>::new(move |_, args| {
            if let Some(args) = args.as_ref() {
                let mut h = heard.lock();
                h.hypothesis = args.Hypothesis()?.Text()?.to_string();
                let _ = tx.try_send(Event::Text { text: h.text(true), done: false });
            }
            Ok(())
        }))?;
    }
    let session: SpeechContinuousRecognitionSession = recognizer.ContinuousRecognitionSession()?;
    {
        let (heard, tx) = (heard.clone(), tx.clone());
        session.ResultGenerated(&TypedEventHandler::<SpeechContinuousRecognitionSession, SpeechContinuousRecognitionResultGeneratedEventArgs>::new(move |_, args| {
            if let Some(args) = args.as_ref() {
                let text = args.Result()?.Text()?.to_string();
                let mut h = heard.lock();
                h.hypothesis.clear();
                if !text.trim().is_empty() {
                    h.done.push(text.trim().to_string());
                }
                let _ = tx.try_send(Event::Text { text: h.text(false), done: false });
            }
            Ok(())
        }))?;
    }
    session.Completed(&TypedEventHandler::<SpeechContinuousRecognitionSession, SpeechContinuousRecognitionCompletedEventArgs>::new(move |_, _| {
        let _ = done_tx.send(());
        Ok(())
    }))?;
    session.StartAsync()?.join()?;
    match ctl.recv().unwrap_or(Ctl::Cancel) {
        Ctl::Stop => {
            // Stopping lets it finish the segment it's on; Completed follows.
            if session.StopAsync().and_then(|op| op.join()).is_ok() {
                let _ = done_rx.recv_timeout(std::time::Duration::from_secs(3));
            }
            let text = heard.lock().text(true);
            let _ = tx.try_send(Event::Text { text, done: true });
        }
        Ctl::Cancel => {
            let _ = session.CancelAsync().and_then(|op| op.join());
        }
    }
    let _ = recognizer.Close();
    Ok(())
}
