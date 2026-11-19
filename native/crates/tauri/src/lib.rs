//! The parts of Tauri that winfer's backend modules use, implemented without a webview, so
//! `../src-tauri/src` compiles here unchanged. Events and channels end at closures the GPUI app
//! registers instead of at JavaScript.

pub use tauri_macros::command;
pub use url::Url;

use serde::Serialize;
use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::fmt;
use std::marker::PhantomData;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};

#[derive(Debug)]
pub struct Error(pub String);

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Error {}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error(e.to_string())
    }
}

impl From<serde_json::Error> for Error {
    fn from(e: serde_json::Error) -> Self {
        Error(e.to_string())
    }
}

impl From<String> for Error {
    fn from(e: String) -> Self {
        Error(e)
    }
}

pub type Result<T> = std::result::Result<T, Error>;

pub trait Runtime: Send + Sync + 'static {}

#[derive(Clone, Copy, Debug, Default)]
pub struct Wry;

impl Runtime for Wry {}

type Listener = Arc<dyn Fn(&str, &serde_json::Value) + Send + Sync>;

struct Inner {
    states: RwLock<HashMap<TypeId, &'static (dyn Any + Send + Sync)>>,
    listeners: RwLock<Vec<Listener>>,
    identifier: String,
    data_dir: PathBuf,
    focused: AtomicBool,
}

pub struct AppHandle<R: Runtime = Wry> {
    inner: Arc<Inner>,
    runtime: PhantomData<fn() -> R>,
}

impl<R: Runtime> Clone for AppHandle<R> {
    fn clone(&self) -> Self {
        Self { inner: self.inner.clone(), runtime: PhantomData }
    }
}

impl AppHandle<Wry> {
    /// The app's handle; its data lives in the platform data folder under `identifier`.
    pub fn new(identifier: &str) -> Self {
        let data_dir = dirs::data_dir().unwrap_or_else(std::env::temp_dir).join(identifier);
        Self {
            inner: Arc::new(Inner {
                states: RwLock::default(),
                listeners: RwLock::default(),
                identifier: identifier.to_string(),
                data_dir,
                focused: AtomicBool::new(true),
            }),
            runtime: PhantomData,
        }
    }
}

impl<R: Runtime> AppHandle<R> {
    /// Calls `listener` with every event the backend emits, on the emitting thread.
    pub fn listen_any(&self, listener: impl Fn(&str, &serde_json::Value) + Send + Sync + 'static) {
        self.inner.listeners.write().unwrap().push(Arc::new(listener));
    }

    /// Tells the backend whether the app has focus (it decides whether to push to the phone).
    pub fn set_focused(&self, focused: bool) {
        self.inner.focused.store(focused, Ordering::Relaxed);
    }

    pub fn config(&self) -> Config {
        Config { identifier: self.inner.identifier.clone() }
    }

    pub fn package_info(&self) -> PackageInfo {
        PackageInfo { name: "winfer".to_string(), version: env!("CARGO_PKG_VERSION").to_string() }
    }

    pub fn exit(&self, code: i32) {
        std::process::exit(code)
    }
}

pub struct PackageInfo {
    pub name: String,
    pub version: String,
}

pub struct Config {
    pub identifier: String,
}

pub struct PathResolver {
    data_dir: PathBuf,
}

impl PathResolver {
    pub fn app_data_dir(&self) -> Result<PathBuf> {
        Ok(self.data_dir.clone())
    }

    pub fn app_log_dir(&self) -> Result<PathBuf> {
        Ok(self.data_dir.join("logs"))
    }

    pub fn app_cache_dir(&self) -> Result<PathBuf> {
        Ok(self.data_dir.join("cache"))
    }

    pub fn app_config_dir(&self) -> Result<PathBuf> {
        Ok(self.data_dir.clone())
    }

    pub fn home_dir(&self) -> Result<PathBuf> {
        dirs::home_dir().ok_or_else(|| Error("no home directory".into()))
    }
}

/// Managed state, borrowed from the app (it lives for the whole process).
pub struct State<'r, T: Send + Sync + 'static>(&'r T);

impl<'r, T: Send + Sync + 'static> State<'r, T> {
    pub fn inner(&self) -> &'r T {
        self.0
    }
}

impl<T: Send + Sync + 'static> std::ops::Deref for State<'_, T> {
    type Target = T;
    fn deref(&self) -> &T {
        self.0
    }
}

impl<T: Send + Sync + 'static> Clone for State<'_, T> {
    fn clone(&self) -> Self {
        Self(self.0)
    }
}

pub type ResourceId = u32;

/// Tauri keeps resources here for JavaScript to refer to; nothing here needs them.
pub struct ResourceTable;

impl ResourceTable {
    pub fn add<T>(&self, _resource: T) -> ResourceId {
        0
    }
}

pub trait Manager<R: Runtime>: Sized {
    fn app_handle(&self) -> &AppHandle<R>;

    fn manage<T: Send + Sync + 'static>(&self, state: T) -> bool {
        let leaked: &'static T = Box::leak(Box::new(state));
        self.app_handle().inner.states.write().unwrap().insert(TypeId::of::<T>(), leaked).is_none()
    }

    fn try_state<T: Send + Sync + 'static>(&self) -> Option<State<'_, T>> {
        let state = *self.app_handle().inner.states.read().unwrap().get(&TypeId::of::<T>())?;
        Some(State(state.downcast_ref::<T>()?))
    }

    fn state<T: Send + Sync + 'static>(&self) -> State<'_, T> {
        self.try_state().unwrap_or_else(|| panic!("{} is not managed", std::any::type_name::<T>()))
    }

    fn path(&self) -> PathResolver {
        PathResolver { data_dir: self.app_handle().inner.data_dir.clone() }
    }

    fn resources_table(&self) -> ResourceTable {
        ResourceTable
    }

    fn webview_windows(&self) -> HashMap<String, WebviewWindow<R>> {
        HashMap::from([("main".to_string(), WebviewWindow { app: self.app_handle().clone() })])
    }

    fn get_webview_window(&self, label: &str) -> Option<WebviewWindow<R>> {
        self.webview_windows().remove(label)
    }
}

impl<R: Runtime> Manager<R> for AppHandle<R> {
    fn app_handle(&self) -> &AppHandle<R> {
        self
    }
}

pub trait Emitter<R: Runtime> {
    fn emit<S: Serialize + Clone>(&self, event: &str, payload: S) -> Result<()>;
}

impl<R: Runtime> Emitter<R> for AppHandle<R> {
    fn emit<S: Serialize + Clone>(&self, event: &str, payload: S) -> Result<()> {
        let value = serde_json::to_value(payload)?;
        let listeners = self.inner.listeners.read().unwrap().clone();
        for listener in listeners {
            listener(event, &value);
        }
        Ok(())
    }
}

/// The app's one window, as the backend sees it.
pub struct WebviewWindow<R: Runtime = Wry> {
    app: AppHandle<R>,
}

impl<R: Runtime> Clone for WebviewWindow<R> {
    fn clone(&self) -> Self {
        Self { app: self.app.clone() }
    }
}

impl<R: Runtime> WebviewWindow<R> {
    pub fn is_focused(&self) -> Result<bool> {
        Ok(self.app.inner.focused.load(Ordering::Relaxed))
    }

    pub fn unminimize(&self) -> Result<()> {
        Ok(())
    }

    pub fn show(&self) -> Result<()> {
        Ok(())
    }

    pub fn set_focus(&self) -> Result<()> {
        Ok(())
    }
}

impl<R: Runtime> Manager<R> for WebviewWindow<R> {
    fn app_handle(&self) -> &AppHandle<R> {
        &self.app
    }
}

impl<R: Runtime> Emitter<R> for WebviewWindow<R> {
    fn emit<S: Serialize + Clone>(&self, event: &str, payload: S) -> Result<()> {
        self.app.emit(event, payload)
    }
}

/// What a command was called from; here only the app behind it matters.
pub struct Webview<R: Runtime = Wry> {
    app: AppHandle<R>,
}

impl<R: Runtime> Webview<R> {
    pub fn new(app: AppHandle<R>) -> Self {
        Self { app }
    }
}

impl<R: Runtime> Manager<R> for Webview<R> {
    fn app_handle(&self) -> &AppHandle<R> {
        &self.app
    }
}

pub mod ipc {
    use super::{Error, Result};
    use serde::Serialize;
    use std::sync::Arc;

    #[derive(Debug, Clone)]
    pub enum InvokeResponseBody {
        Json(String),
        Raw(Vec<u8>),
    }

    #[derive(Debug, Clone)]
    pub enum InvokeBody {
        Json(serde_json::Value),
        Raw(Vec<u8>),
    }

    pub trait IpcResponse {
        fn body(self) -> Result<InvokeResponseBody>;
    }

    impl<T: Serialize> IpcResponse for T {
        fn body(self) -> Result<InvokeResponseBody> {
            Ok(InvokeResponseBody::Json(serde_json::to_string(&self).map_err(Error::from)?))
        }
    }

    impl IpcResponse for InvokeResponseBody {
        fn body(self) -> Result<InvokeResponseBody> {
            Ok(self)
        }
    }

    #[derive(Debug, Clone)]
    pub struct Response(InvokeResponseBody);

    impl Response {
        pub fn new(body: impl IpcResponse) -> Self {
            Self(body.body().unwrap_or(InvokeResponseBody::Raw(Vec::new())))
        }

        pub fn into_body(self) -> InvokeResponseBody {
            self.0
        }
    }

    impl IpcResponse for Response {
        fn body(self) -> Result<InvokeResponseBody> {
            Ok(self.0)
        }
    }

    /// A stream from a command to whoever called it.
    pub struct Channel<T = InvokeResponseBody> {
        send: Arc<dyn Fn(InvokeResponseBody) -> Result<()> + Send + Sync>,
        message: std::marker::PhantomData<fn(T)>,
    }

    impl<T> Clone for Channel<T> {
        fn clone(&self) -> Self {
            Self { send: self.send.clone(), message: std::marker::PhantomData }
        }
    }

    impl<T: IpcResponse> Channel<T> {
        pub fn new(send: impl Fn(InvokeResponseBody) -> Result<()> + Send + Sync + 'static) -> Self {
            Self { send: Arc::new(send), message: std::marker::PhantomData }
        }

        pub fn send(&self, message: T) -> Result<()> {
            (self.send)(message.body()?)
        }
    }

    /// A raw-body command call.
    pub struct Request<'a> {
        body: &'a InvokeBody,
        headers: &'a http::HeaderMap,
    }

    impl<'a> Request<'a> {
        pub fn new(body: &'a InvokeBody, headers: &'a http::HeaderMap) -> Self {
            Self { body, headers }
        }

        pub fn body(&self) -> &InvokeBody {
            self.body
        }

        pub fn headers(&self) -> &http::HeaderMap {
            self.headers
        }
    }
}

pub mod async_runtime {
    use std::future::Future;
    use std::pin::Pin;
    use std::sync::OnceLock;
    use std::task::{Context, Poll};

    fn runtime() -> &'static tokio::runtime::Runtime {
        static RUNTIME: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
        RUNTIME.get_or_init(|| tokio::runtime::Runtime::new().expect("tokio runtime"))
    }

    pub struct JoinHandle<T>(tokio::task::JoinHandle<T>);

    impl<T> JoinHandle<T> {
        pub fn abort(&self) {
            self.0.abort()
        }
    }

    impl<T> Future for JoinHandle<T> {
        type Output = super::Result<T>;
        fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
            Pin::new(&mut self.0).poll(cx).map(|r| r.map_err(|e| super::Error(e.to_string())))
        }
    }

    pub fn spawn<F>(future: F) -> JoinHandle<F::Output>
    where
        F: Future + Send + 'static,
        F::Output: Send + 'static,
    {
        JoinHandle(runtime().spawn(future))
    }

    pub fn spawn_blocking<F, R>(f: F) -> JoinHandle<R>
    where
        F: FnOnce() -> R + Send + 'static,
        R: Send + 'static,
    {
        JoinHandle(runtime().spawn_blocking(f))
    }

    pub fn block_on<F: Future>(future: F) -> F::Output {
        runtime().block_on(future)
    }
}
