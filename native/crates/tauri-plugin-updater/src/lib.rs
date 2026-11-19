//! The native build has no signed update feed of its own yet: the Tauri app's releases can't
//! replace it, so a check always answers "nothing newer".
use tauri::{Manager, Runtime, Url, Webview};

pub trait UpdaterExt {
    fn updater_builder(&self) -> UpdaterBuilder;
}

impl<R: Runtime> UpdaterExt for Webview<R> {
    fn updater_builder(&self) -> UpdaterBuilder {
        let _ = self.app_handle();
        UpdaterBuilder
    }
}

pub struct UpdaterBuilder;

impl UpdaterBuilder {
    pub fn endpoints(self, _endpoints: Vec<Url>) -> Result<Self, String> {
        Ok(self)
    }

    pub fn build(self) -> Result<Updater, String> {
        Ok(Updater)
    }
}

pub struct Updater;

impl Updater {
    pub async fn check(&self) -> Result<Option<Update>, String> {
        Ok(None)
    }
}

pub struct Update {
    pub current_version: String,
    pub version: String,
    pub date: Option<Date>,
    pub body: Option<String>,
    pub raw_json: serde_json::Value,
}

/// A release date (never built, since `check` finds no update).
#[derive(Clone, Copy)]
pub struct Date;

impl std::fmt::Display for Date {
    fn fmt(&self, _: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        Ok(())
    }
}
