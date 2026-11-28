//! [`Connections`]: the saved connections and their live state (store.ts
//! connection + "Staying connected" parts, links.ts, reconnect.test.ts).
//!
//! - The saved list comes from `Backend::list_connections`, in sidebar order
//!   (prefs `connectionOrder`, `boa_core::state::prefs::apply_order`).
//! - Live state per connection ([`ConnState`]), latency, server version and
//!   databases of the last connect.
//! - Connect / disconnect / [`Connections::ensure_connected`] ("running a query
//!   connects first"); a connect in progress is joined, not repeated.
//! - The password prompt ([`PasswordPromptState`], opened by
//!   `views::password_prompt` on [`ConnectionsEvent::PasswordPrompt`]) when the
//!   server wants a password that isn't stored or refuses it (`password_required`,
//!   `28P01`, `28000`).
//! - The reconnect loop: `boa_core::state::reconnect::Recoveries` (the pure state
//!   machine) driven here with GPUI timers, on `Backend::link_events` + `ping`,
//!   the 5 s wake tick and the macOS wake / network notifications
//!   ([`crate::platform`]). Backoff `RETRY_DELAYS_MS` (~2 min), then
//!   `Failed` with a Retry toast. Statement retry-safety is boa-core's
//!   (only safe statements are re-run; others report `connection_lost` /
//!   `tx_lost` / commit-unknown, see [`describe_error`]).
//! - One status toast per connection (key [`toast_key`], source commit 61984bb).
//!
//! The backend is reached through [`ConnApi`] (implemented by
//! `boa_core::backend::Backend`), so tests can script it.
//!
//! Hooks for later phases: [`Connections::restore`] (Phase 7, connect at
//! launch), [`Connections::set_close_guard`] (Phase 6, ask before closing
//! sessions with an open transaction), [`ConnectionsEvent`] (explorer,
//! workspaces: connected / deleted / transaction lost).

use std::collections::{BTreeSet, HashMap};
use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use boa_core::backend::{Backend, LinkEvent};
use boa_core::connections::{ConnConfig, SavedConnection};
use boa_core::error::CmdError;
use boa_core::pg::{ConnectInfo, PingInfo};
use boa_core::state::guards::CloseAction;
use boa_core::state::prefs::apply_order;
use boa_core::state::reconnect::{
    self as rc, ConnLinkState, ConnectAttempt, Effect, RecoverOpts, RecoverReason, Recoveries,
};
use futures::StreamExt as _;
use futures::channel::oneshot;
use gpui::{App, Context, Entity, EventEmitter, Task};

use super::prefs::{Prefs, PrefsChanged};
use super::toasts::{NewToast, ToastKind, Toasts};

/// A connection's live state (store.ts `ConnState`).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum ConnState {
    #[default]
    Disconnected,
    Connecting,
    Connected {
        server_version: String,
        databases: Vec<String>,
    },
    Reconnecting {
        attempt: u32,
    },
    Failed {
        message: String,
    },
}

impl ConnState {
    /// The TypeScript name (`connected`, `reconnecting`, …).
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Disconnected => "disconnected",
            Self::Connecting => "connecting",
            Self::Connected { .. } => "connected",
            Self::Reconnecting { .. } => "reconnecting",
            Self::Failed { .. } => "failed",
        }
    }
}

/// The key of a connection's status toast (store.ts `connToastKey`).
pub fn toast_key(connection_id: &str) -> String {
    rc::conn_toast_key(connection_id)
}

/// What an error from a statement means for the user: the message, plus a
/// note for the two "staying connected" cases (USAGE.md): the transaction was
/// rolled back with a dropped session (`tx_lost`), or the connection dropped
/// while committing so the outcome is unknown. For consoles / grids.
pub fn describe_error(e: &CmdError) -> (String, Option<String>) {
    if e.code.as_deref() == Some(rc::codes::TX_LOST) {
        return (e.message.clone(), e.hint.clone());
    }
    if rc::is_commit_unknown(e) {
        return (
            e.message.clone(),
            Some(
                e.hint
                    .clone()
                    .unwrap_or_else(|| "Check the data to see whether it was committed.".into()),
            ),
        );
    }
    (
        e.message.clone(),
        e.detail.clone().or_else(|| e.hint.clone()),
    )
}

/// The password prompt to show (store.ts `passwordPrompt`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PasswordPromptState {
    pub connection_id: String,
    /// Shown under the field (a refused password).
    pub error: Option<String>,
}

/// What [`Connections`] tells its observers besides `cx.notify()`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnectionsEvent {
    /// A connect succeeded (also a reconnect that had to connect again).
    Connected { id: String },
    /// The user disconnected.
    Disconnected { id: String },
    /// Saved from the dialog (`created`: a new data source).
    Saved { id: String, created: bool },
    /// Deleted: drop its workspaces / tree state.
    Deleted { id: String },
    /// Open (or refresh) the password prompt for this connection.
    PasswordPrompt { id: String },
    /// A session of `database` dropped with a transaction open; the server
    /// rolled it back (its tx state is now idle).
    TxLost { id: String, database: String },
}

/// A boxed backend call.
pub type ApiFuture<T> = Pin<Box<dyn Future<Output = Result<T, CmdError>> + 'static>>;

/// The backend calls the model makes (`Backend` implements it; tests script one).
pub trait ConnApi: 'static {
    fn list(&self) -> ApiFuture<Vec<SavedConnection>>;
    /// The saved list read right now, on this thread, if the backend can
    /// (the first load: no task, so a test scheduler sees no foreign wake-up).
    fn list_now(&self) -> Option<Result<Vec<SavedConnection>, CmdError>> {
        None
    }
    fn save(
        &self,
        config: ConnConfig,
        password: Option<String>,
        save_password: bool,
    ) -> ApiFuture<()>;
    fn delete(&self, id: &str) -> ApiFuture<()>;
    fn test(&self, config: ConnConfig, password: Option<String>) -> ApiFuture<String>;
    fn connect(&self, id: &str, password: Option<String>) -> ApiFuture<ConnectInfo>;
    fn ping(&self, id: &str) -> ApiFuture<PingInfo>;
    fn disconnect(&self, id: &str) -> ApiFuture<()>;
}

impl ConnApi for Backend {
    fn list(&self) -> ApiFuture<Vec<SavedConnection>> {
        Box::pin(self.list_connections())
    }
    fn list_now(&self) -> Option<Result<Vec<SavedConnection>, CmdError>> {
        // One small file in the config dir (what list_connections reads).
        Some(boa_core::connections::list(self.config_dir()))
    }
    fn save(
        &self,
        config: ConnConfig,
        password: Option<String>,
        save_password: bool,
    ) -> ApiFuture<()> {
        Box::pin(self.save_connection(config, password, save_password))
    }
    fn delete(&self, id: &str) -> ApiFuture<()> {
        Box::pin(self.delete_connection(id))
    }
    fn test(&self, config: ConnConfig, password: Option<String>) -> ApiFuture<String> {
        Box::pin(self.test_connection(config, password))
    }
    fn connect(&self, id: &str, password: Option<String>) -> ApiFuture<ConnectInfo> {
        Box::pin(Backend::connect(self, id, password))
    }
    fn ping(&self, id: &str) -> ApiFuture<PingInfo> {
        Box::pin(Backend::ping(self, id))
    }
    fn disconnect(&self, id: &str) -> ApiFuture<()> {
        Box::pin(Backend::disconnect(self, id))
    }
}

/// Asks before closing sessions with an open transaction (Phase 6's guard
/// dialog): resolves true to go ahead. Arguments: what closes them and the
/// connection id.
pub type CloseGuard = Rc<dyn Fn(CloseAction, &str, &mut App) -> Task<bool>>;

/// How a connect was asked for (store.ts `connectNow`'s options).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ConnectOpts {
    /// Never open the password prompt (restoring in the background).
    quiet: bool,
    /// Show the status toast for errors other than a refused login.
    toast: bool,
}

impl Default for ConnectOpts {
    fn default() -> Self {
        Self {
            quiet: false,
            toast: true,
        }
    }
}

type ConnectResult = Result<(), CmdError>;

/// Saved connections and their live state.
pub struct Connections {
    /// Saved connections, in sidebar order.
    pub saved: Vec<SavedConnection>,
    /// Their ids in the backend's order (connections.json, new ones last):
    /// store.ts `connectionOrder`, see [`Connections::first_listed`].
    listed: Vec<String>,
    /// Live state by connection id.
    pub state: HashMap<String, ConnState>,
    api: Option<Rc<dyn ConnApi>>,
    toasts: Option<Entity<Toasts>>,
    prefs: Option<Entity<Prefs>>,
    loaded: bool,
    /// Connections the user opened and hasn't disconnected (store.ts `wanted`):
    /// reconnected when they drop or the machine wakes, and at the next launch.
    wanted: BTreeSet<String>,
    latency_ms: HashMap<String, u64>,
    /// Server version and databases of the last successful connect.
    info: HashMap<String, ConnectInfo>,
    last_error: HashMap<String, String>,
    /// When the next retry runs (ms on [`Self::now_ms`]'s clock).
    retry_at: HashMap<String, u64>,
    recoveries: Recoveries,
    timers: HashMap<String, Task<()>>,
    /// Connects in flight and who waits for them.
    connecting: HashMap<String, Vec<oneshot::Sender<ConnectResult>>>,
    /// Waiting for a recovery to end (connected or given up).
    recovery_waiters: HashMap<String, Vec<oneshot::Sender<bool>>>,
    /// Waiting for a recovery's current attempt (store.ts `retryNow`).
    attempt_waiters: HashMap<String, Vec<oneshot::Sender<bool>>>,
    prompt: Option<PasswordPromptState>,
    close_guard: Option<CloseGuard>,
    clock_base: Option<Instant>,
    network_debounce: Option<Task<()>>,
    monitor: Option<crate::platform::Monitor>,
    tasks: Vec<Task<()>>,
    _subscriptions: Vec<gpui::Subscription>,
}

impl fmt::Debug for Connections {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Connections")
            .field("saved", &self.saved)
            .field("state", &self.state)
            .field("wanted", &self.wanted)
            .field("prompt", &self.prompt)
            .finish_non_exhaustive()
    }
}

impl Default for Connections {
    fn default() -> Self {
        Self::new()
    }
}

impl EventEmitter<ConnectionsEvent> for Connections {}

/// How long after a network change the connections are checked (the
/// interface settles; changes come in bursts).
pub const NETWORK_SETTLE: Duration = Duration::from_secs(2);

impl Connections {
    /// A model without a backend: lists nothing, connects nothing (previews,
    /// tests of other views). See [`Connections::with_backend`].
    pub fn new() -> Self {
        Self {
            saved: Vec::new(),
            listed: Vec::new(),
            state: HashMap::new(),
            api: None,
            toasts: None,
            prefs: None,
            loaded: false,
            wanted: BTreeSet::new(),
            latency_ms: HashMap::new(),
            info: HashMap::new(),
            last_error: HashMap::new(),
            retry_at: HashMap::new(),
            recoveries: Recoveries::new(),
            timers: HashMap::new(),
            connecting: HashMap::new(),
            recovery_waiters: HashMap::new(),
            attempt_waiters: HashMap::new(),
            prompt: None,
            close_guard: None,
            clock_base: None,
            network_debounce: None,
            monitor: None,
            tasks: Vec::new(),
            _subscriptions: Vec::new(),
        }
    }

    /// The app's model: loads the saved connections and listens to the
    /// backend's [`LinkEvent`]s (call [`Connections::watch`] too, for wake-ups).
    pub fn with_backend(
        backend: Backend,
        toasts: Option<Entity<Toasts>>,
        prefs: Option<Entity<Prefs>>,
        cx: &mut Context<Self>,
    ) -> Self {
        let events = backend.link_events();
        let mut this = Self::with_api(Rc::new(backend), toasts, prefs, cx);
        this.tasks.push(cx.spawn(async move |this, cx| {
            while let Ok(event) = events.recv().await {
                if this
                    .update(cx, |this, cx| this.on_link_event(&event, cx))
                    .is_err()
                {
                    break;
                }
            }
        }));
        this
    }

    /// Replaces the backend calls (tests of other models).
    #[cfg(test)]
    pub(crate) fn set_api(&mut self, api: Rc<dyn ConnApi>) {
        self.api = Some(api);
    }

    /// A model on any [`ConnApi`] (tests). Starts loading the saved list.
    pub fn with_api(
        api: Rc<dyn ConnApi>,
        toasts: Option<Entity<Toasts>>,
        prefs: Option<Entity<Prefs>>,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut this = Self::new();
        this.api = Some(api);
        this.clock_base = Some(cx.background_executor().now());
        if let Some(prefs) = &prefs {
            this._subscriptions
                .push(cx.subscribe(prefs, |this, prefs, e: &PrefsChanged, cx| {
                    if e.prev.connection_order != prefs.read(cx).get().connection_order {
                        this.sort_saved(cx);
                        cx.notify();
                    }
                }));
        }
        this.toasts = toasts;
        this.prefs = prefs;
        match this.api.as_ref().and_then(|api| api.list_now()) {
            Some(Ok(list)) => this.set_saved(list, cx),
            Some(Err(e)) => {
                this.loaded = true;
                tracing::error!("could not load connections: {}", e.message);
            }
            None => this.load(cx).detach(),
        }
        this
    }

    // ── Reading ───────────────────────────────────────────────────────────

    /// The live state of `id` (disconnected when unknown).
    pub fn state_of(&self, id: &str) -> ConnState {
        self.state.get(id).cloned().unwrap_or_default()
    }

    /// A saved connection by id.
    pub fn get(&self, id: &str) -> Option<&SavedConnection> {
        self.saved.iter().find(|c| c.config.id == id)
    }

    /// The databases of a connected connection (empty otherwise).
    pub fn databases(&self, id: &str) -> Vec<String> {
        match self.state.get(id) {
            Some(ConnState::Connected { databases, .. }) => databases.clone(),
            _ => Vec::new(),
        }
    }

    pub fn is_connected(&self, id: &str) -> bool {
        matches!(self.state.get(id), Some(ConnState::Connected { .. }))
    }

    /// The saved list has been read (or failed to be) at least once.
    pub fn is_loaded(&self) -> bool {
        self.loaded
    }

    /// Round trip of the last connect / ping, while connected.
    pub fn latency_ms(&self, id: &str) -> Option<u64> {
        self.latency_ms.get(id).copied()
    }

    /// `PostgreSQL 17.2`, from the last connect.
    pub fn server_version(&self, id: &str) -> Option<&str> {
        self.info.get(id).map(|i| i.server_version.as_str())
    }

    /// The user wants it open (connected and not disconnected since).
    pub fn is_wanted(&self, id: &str) -> bool {
        self.wanted.contains(id)
    }

    /// Ids of the connections the user wants open (for session.json, Phase 7).
    pub fn wanted_ids(&self) -> Vec<String> {
        self.wanted.iter().cloned().collect()
    }

    /// A connect is in flight.
    pub fn is_connecting(&self, id: &str) -> bool {
        self.connecting.contains_key(id)
    }

    /// The reconnect loop is bringing it back.
    pub fn is_recovering(&self, id: &str) -> bool {
        self.recoveries.is_recovering(id)
    }

    /// The last error connecting (shown on failed rows).
    pub fn last_error(&self, id: &str) -> Option<&str> {
        self.last_error.get(id).map(String::as_str)
    }

    /// Seconds until the next retry while reconnecting.
    pub fn retry_in_secs(&self, id: &str, cx: &App) -> Option<u64> {
        let at = *self.retry_at.get(id)?;
        Some(at.saturating_sub(self.now_ms(cx)).div_ceil(1000))
    }

    /// The password prompt to show, if any.
    pub fn password_prompt(&self) -> Option<&PasswordPromptState> {
        self.prompt.as_ref()
    }

    /// store.ts `connectionString`: `postgresql://user@host:port/db` (no
    /// password); Redis: `redis://user@host:port/<index>`.
    pub fn connection_string(&self, id: &str, database: Option<&str>) -> Option<String> {
        let c = &self.get(id)?.config;
        if c.kind.is_redis() {
            let scheme = match c.ssl_mode {
                boa_core::connections::SslMode::Disable
                | boa_core::connections::SslMode::Prefer => "redis",
                _ => "rediss",
            };
            let auth = if c.user.is_empty() {
                String::new()
            } else {
                format!("{}@", encode_uri_component(&c.user))
            };
            let db = database.map_or_else(|| c.database.clone(), str::to_owned);
            let index = boa_core::redis::db_index(&db).unwrap_or(0);
            return Some(format!("{scheme}://{auth}{}:{}/{index}", c.host, c.port));
        }
        let auth = if c.user.is_empty() {
            String::new()
        } else {
            format!("{}@", encode_uri_component(&c.user))
        };
        let port = if c.port == 0 {
            String::new()
        } else {
            format!(":{}", c.port)
        };
        Some(format!(
            "postgresql://{auth}{}{port}/{}",
            c.host,
            database.unwrap_or(&c.database)
        ))
    }

    fn name(&self, id: &str) -> String {
        self.get(id)
            .map_or_else(|| id.to_owned(), |c| c.config.name.clone())
    }

    fn now_ms(&self, cx: &App) -> u64 {
        let base = self
            .clock_base
            .unwrap_or_else(|| cx.background_executor().now());
        let elapsed = cx
            .background_executor()
            .now()
            .saturating_duration_since(base);
        u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX)
    }

    fn link_state(&self, id: &str) -> ConnLinkState {
        match self.state.get(id) {
            None | Some(ConnState::Disconnected) => ConnLinkState::Disconnected,
            Some(ConnState::Connecting) => ConnLinkState::Connecting,
            Some(ConnState::Connected { .. }) => ConnLinkState::Connected,
            Some(ConnState::Reconnecting { attempt }) => ConnLinkState::Reconnecting {
                attempt: *attempt,
                next_at_ms: self.retry_at.get(id).copied().unwrap_or(0),
            },
            Some(ConnState::Failed { .. }) => ConnLinkState::Failed,
        }
    }

    // ── The saved list ────────────────────────────────────────────────────

    /// (Re)loads the saved list from the backend (store.ts `loadConnections`).
    pub fn reload(&mut self, cx: &mut Context<Self>) {
        self.load(cx).detach();
    }

    /// [`Connections::reload`], resolving once the list is in.
    pub fn load(&mut self, cx: &mut Context<Self>) -> Task<()> {
        let Some(api) = self.api.clone() else {
            self.loaded = true;
            return Task::ready(());
        };
        let list = api.list();
        cx.spawn(async move |this, cx| {
            let result = list.await;
            this.update(cx, |this, cx| match result {
                Ok(list) => this.set_saved(list, cx),
                Err(e) => {
                    this.loaded = true;
                    this.report_error("Could not load connections", &e.message, cx);
                    cx.notify();
                }
            })
            .ok();
        })
    }

    fn set_saved(&mut self, list: Vec<SavedConnection>, cx: &mut Context<Self>) {
        self.listed = list.iter().map(|c| c.config.id.clone()).collect();
        self.saved = list;
        let ids: BTreeSet<&str> = self.saved.iter().map(|c| c.config.id.as_str()).collect();
        self.state.retain(|id, _| ids.contains(id.as_str()));
        self.wanted.retain(|id| ids.contains(id.as_str()));
        self.sort_saved(cx);
        self.loaded = true;
        cx.notify();
    }

    fn sort_saved(&mut self, cx: &App) {
        let order = self
            .prefs
            .as_ref()
            .map(|p| p.read(cx).get().connection_order.clone())
            .unwrap_or_default();
        let ids: Vec<&str> = self.saved.iter().map(|c| c.config.id.as_str()).collect();
        let rank: HashMap<String, usize> = apply_order(&ids, &order)
            .into_iter()
            .enumerate()
            .map(|(i, id)| (id, i))
            .collect();
        self.saved
            .sort_by_key(|c| rank.get(&c.config.id).copied().unwrap_or(usize::MAX));
    }

    /// The first saved connection in the backend's order (store.ts
    /// `connectionOrder[0]`, not the sidebar's): what the status bar shows
    /// when no database is selected. Connections added without a load (tests)
    /// count in sidebar order after the listed ones.
    pub fn first_listed(&self) -> Option<&SavedConnection> {
        self.listed
            .iter()
            .find_map(|id| self.get(id))
            .or_else(|| self.saved.first())
    }

    /// Sidebar drag-reorder: saves `ids` as prefs `connectionOrder` and re-sorts.
    pub fn reorder(&mut self, ids: Vec<String>, cx: &mut Context<Self>) {
        if let Some(prefs) = self.prefs.clone() {
            prefs.update(cx, |prefs, cx| {
                prefs.update(cx, |p| p.connection_order = ids.clone());
            });
        } else {
            let rank: HashMap<&str, usize> = ids
                .iter()
                .enumerate()
                .map(|(i, id)| (id.as_str(), i))
                .collect();
            self.saved.sort_by_key(|c| {
                rank.get(c.config.id.as_str())
                    .copied()
                    .unwrap_or(usize::MAX)
            });
        }
        self.sort_saved(cx);
        cx.notify();
    }

    /// Saves a new or edited connection (store.ts `saveConnection`). `password`:
    /// `Some` typed (stored when `save_password`), `None` keeps the stored one. A
    /// changed connection that is open reconnects (asking first, Phase 6,
    /// when a transaction is open).
    pub fn save(
        &mut self,
        config: ConnConfig,
        password: Option<String>,
        save_password: bool,
        cx: &mut Context<Self>,
    ) -> Task<Result<(), CmdError>> {
        let Some(api) = self.api.clone() else {
            return Task::ready(Err(no_backend()));
        };
        let write = api.save(config.clone(), password.clone(), save_password);
        cx.spawn(async move |this, cx| {
            write.await?;
            let reconnect = this
                .update(cx, |this, cx| {
                    let id = config.id.clone();
                    let prev = this.get(&id).cloned();
                    let has_password = save_password
                        && (password.is_some() || prev.as_ref().is_some_and(|p| p.has_password));
                    let name = config.name.clone();
                    let saved = SavedConnection {
                        config,
                        has_password,
                    };
                    match this.saved.iter_mut().find(|c| c.config.id == id) {
                        Some(slot) => *slot = saved,
                        None => this.saved.push(saved),
                    }
                    if !this.listed.contains(&id) {
                        this.listed.push(id.clone());
                    }
                    this.sort_saved(cx);
                    cx.emit(ConnectionsEvent::Saved {
                        id: id.clone(),
                        created: prev.is_none(),
                    });
                    cx.notify();
                    let open = this.is_connected(&id);
                    (open, id, name, this.close_guard.clone())
                })
                .map_err(gone)?;
            let (open, id, _name, guard) = reconnect;
            if !open {
                return Ok(());
            }
            // Reconnecting rolls back open transactions: ask first (declined:
            // the new settings apply at the next connect).
            let go = match guard {
                Some(guard) => cx.update(|cx| guard(CloseAction::Reconnect, &id, cx)).await,
                None => true,
            };
            if go {
                let done = this
                    .update(cx, |this, cx| {
                        this.stop_recovery(&id, cx);
                        let pw = if save_password { None } else { password };
                        this.connect_now(&id, pw, ConnectOpts::default(), cx)
                    })
                    .map_err(gone)?;
                let _ = done.await;
            }
            Ok(())
        })
    }

    /// Deletes a saved connection, its password and its query history
    /// (store.ts `deleteConnection`).
    pub fn delete(&mut self, id: &str, cx: &mut Context<Self>) -> Task<Result<(), CmdError>> {
        let Some(api) = self.api.clone() else {
            return Task::ready(Err(no_backend()));
        };
        let id = id.to_owned();
        let call = api.delete(&id);
        cx.spawn(async move |this, cx| {
            call.await?;
            this.update(cx, |this, cx| {
                this.stop_recovery(&id, cx);
                this.saved.retain(|c| c.config.id != id);
                this.listed.retain(|l| *l != id);
                this.state.remove(&id);
                this.wanted.remove(&id);
                this.latency_ms.remove(&id);
                this.info.remove(&id);
                this.last_error.remove(&id);
                this.retry_at.remove(&id);
                if this.prompt.as_ref().is_some_and(|p| p.connection_id == id) {
                    this.prompt = None;
                }
                if let Some(toasts) = this.toasts.clone() {
                    toasts.update(cx, |t, cx| t.dismiss_key(&toast_key(&id), cx));
                }
                cx.emit(ConnectionsEvent::Deleted { id: id.clone() });
                cx.notify();
            })
            .map_err(gone)?;
            Ok(())
        })
    }

    /// Connects with unsaved settings and returns the server version (the
    /// dialog's Test connection). `password: None` uses the stored one.
    pub fn test(
        &self,
        config: ConnConfig,
        password: Option<String>,
        cx: &mut Context<Self>,
    ) -> Task<Result<String, CmdError>> {
        let Some(api) = self.api.clone() else {
            return Task::ready(Err(no_backend()));
        };
        let call = api.test(config, password);
        cx.spawn(async move |_, _| call.await)
    }

    // ── Connecting ────────────────────────────────────────────────────────

    /// Connects with the stored password (opening the password prompt when
    /// none is stored or it is refused). While the connection is being
    /// re-established after a drop, retries that right away instead.
    pub fn connect(&mut self, id: &str, cx: &mut Context<Self>) {
        self.connect_with(id, None, cx).detach();
    }

    /// store.ts `connect(id, password)`: with `password` (typed in the prompt),
    /// else the stored one. Resolves once connected or failed.
    pub fn connect_with(
        &mut self,
        id: &str,
        password: Option<String>,
        cx: &mut Context<Self>,
    ) -> Task<Result<(), CmdError>> {
        if password.is_none() && self.recoveries.is_recovering(id) {
            let attempt = self.retry_now(id, cx);
            let err = self.not_connected_error(id);
            return cx.spawn(async move |_, _| if attempt.await { Ok(()) } else { Err(err) });
        }
        self.stop_recovery(id, cx);
        self.connect_now(id, password, ConnectOpts::default(), cx)
    }

    /// Connected → ready now; otherwise connects (password prompt if needed),
    /// joins a connect in progress or retries a reconnect now, and resolves
    /// when connected or failed ("running a query connects first").
    pub fn ensure_connected(
        &mut self,
        id: &str,
        cx: &mut Context<Self>,
    ) -> Task<Result<(), CmdError>> {
        if self.get(id).is_none() {
            return Task::ready(Err(CmdError::msg("Unknown connection")));
        }
        if self.is_connected(id) {
            return Task::ready(Ok(()));
        }
        if self.connecting.contains_key(id) {
            return self.connect_now(id, None, ConnectOpts::default(), cx);
        }
        self.connect_with(id, None, cx)
    }

    fn not_connected_error(&self, id: &str) -> CmdError {
        let message = self
            .last_error
            .get(id)
            .cloned()
            .unwrap_or_else(|| format!("Not connected to {}", self.name(id)));
        CmdError::code(rc::codes::NOT_CONNECTED, message)
    }

    /// store.ts `connectNow`: connects (joining one in flight), lists the
    /// databases; on failure applies `connect_failure` (state, prompt, toast).
    fn connect_now(
        &mut self,
        id: &str,
        password: Option<String>,
        opts: ConnectOpts,
        cx: &mut Context<Self>,
    ) -> Task<ConnectResult> {
        let (tx, rx) = oneshot::channel();
        let waiter = cx.background_executor().spawn(async move {
            rx.await
                .unwrap_or_else(|_| Err(CmdError::msg("The connection attempt was dropped")))
        });
        if let Some(waiters) = self.connecting.get_mut(id) {
            waiters.push(tx);
            return waiter;
        }
        let Some(conn) = self.get(id).cloned() else {
            let _ = tx.send(Err(CmdError::msg("Unknown connection")));
            return waiter;
        };
        let Some(api) = self.api.clone() else {
            let _ = tx.send(Err(no_backend()));
            return waiter;
        };
        self.connecting.insert(id.to_owned(), vec![tx]);
        self.state.insert(id.to_owned(), ConnState::Connecting);
        self.latency_ms.remove(id);
        cx.notify();
        let started = cx.background_executor().now();
        let password_given = password.is_some();
        let call = api.connect(id, password);
        let id = id.to_owned();
        cx.spawn(async move |this, cx| {
            let result = call.await;
            this.update(cx, |this, cx| {
                let how = ConnectAttempt {
                    password_given,
                    has_password: conn.has_password,
                    quiet: opts.quiet,
                    toast: opts.toast,
                };
                this.connect_finished(&id, how, started, result, cx);
            })
            .ok();
        })
        .detach();
        waiter
    }

    fn connect_finished(
        &mut self,
        id: &str,
        how: ConnectAttempt,
        started: Instant,
        result: Result<ConnectInfo, CmdError>,
        cx: &mut Context<Self>,
    ) {
        let waiters = self.connecting.remove(id).unwrap_or_default();
        let outcome = match result {
            Ok(info) => {
                let ms = cx
                    .background_executor()
                    .now()
                    .saturating_duration_since(started);
                self.latency_ms.insert(
                    id.to_owned(),
                    u64::try_from(ms.as_millis()).unwrap_or(u64::MAX),
                );
                self.state.insert(
                    id.to_owned(),
                    ConnState::Connected {
                        server_version: info.server_version.clone(),
                        databases: info.databases.clone(),
                    },
                );
                self.info.insert(id.to_owned(), info);
                self.last_error.remove(id);
                self.retry_at.remove(id);
                self.wanted.insert(id.to_owned());
                if self.prompt.as_ref().is_some_and(|p| p.connection_id == id) {
                    self.prompt = None;
                }
                self.clear_failure_toast(id, cx);
                cx.emit(ConnectionsEvent::Connected { id: id.to_owned() });
                Ok(())
            }
            Err(err) => {
                let name = self.name(id);
                let f = rc::connect_failure(id, &name, &err, how);
                self.last_error.insert(id.to_owned(), err.message.clone());
                self.apply_link_state(id, f.state, cx);
                if let Some(prompt) = f.prompt {
                    self.prompt = Some(PasswordPromptState {
                        connection_id: id.to_owned(),
                        error: prompt.error,
                    });
                    cx.emit(ConnectionsEvent::PasswordPrompt { id: id.to_owned() });
                }
                if let Some(toast) = f.toast {
                    self.push_toast(toast, cx);
                }
                Err(err)
            }
        };
        for w in waiters {
            let _ = w.send(outcome.clone());
        }
        cx.notify();
    }

    /// The password prompt's Connect (PasswordPrompt `submit`): saves the
    /// password first when asked to (a failed save shows under the field),
    /// then connects with it.
    pub fn submit_password(
        &mut self,
        id: &str,
        password: String,
        save: bool,
        cx: &mut Context<Self>,
    ) -> Task<Result<(), CmdError>> {
        let id = id.to_owned();
        let saving = match (save, self.get(&id)) {
            (true, Some(conn)) => {
                Some(self.save(conn.config.clone(), Some(password.clone()), true, cx))
            }
            _ => None,
        };
        cx.spawn(async move |this, cx| {
            if let Some(saving) = saving
                && let Err(e) = saving.await
            {
                this.update(cx, |this, cx| {
                    this.prompt = Some(PasswordPromptState {
                        connection_id: id.clone(),
                        error: Some(e.message.clone()),
                    });
                    cx.emit(ConnectionsEvent::PasswordPrompt { id: id.clone() });
                    cx.notify();
                })
                .map_err(gone)?;
                return Err(e);
            }
            let done = this
                .update(cx, |this, cx| this.connect_with(&id, Some(password), cx))
                .map_err(gone)?;
            done.await
        })
    }

    /// Opens the password prompt for `id` (with `error` under the field).
    pub fn ask_password(&mut self, id: &str, error: Option<String>, cx: &mut Context<Self>) {
        self.prompt = Some(PasswordPromptState {
            connection_id: id.to_owned(),
            error,
        });
        cx.emit(ConnectionsEvent::PasswordPrompt { id: id.to_owned() });
        cx.notify();
    }

    /// Closes the password prompt (Cancel / Esc).
    pub fn dismiss_password_prompt(&mut self, cx: &mut Context<Self>) {
        if self.prompt.take().is_some() {
            cx.notify();
        }
    }

    /// Disconnects (store.ts `disconnect`): asks first when a transaction is
    /// open ([`Connections::set_close_guard`], Phase 6), stops reconnecting,
    /// forgets the connection is wanted.
    pub fn disconnect(&mut self, id: &str, cx: &mut Context<Self>) {
        self.disconnect_task(id, cx).detach();
    }

    /// [`Connections::disconnect`], resolving true once disconnected (false: declined).
    pub fn disconnect_task(&mut self, id: &str, cx: &mut Context<Self>) -> Task<bool> {
        let id = id.to_owned();
        let ask = self
            .close_guard
            .clone()
            .map(|guard| guard(CloseAction::Disconnect, &id, cx));
        cx.spawn(async move |this, cx| {
            if let Some(ask) = ask
                && !ask.await
            {
                return false;
            }
            let call = this.update(cx, |this, cx| this.disconnect_now(&id, cx));
            match call {
                Ok(Some(call)) => {
                    if let Err(e) = call.await {
                        this.update(cx, |this, cx| {
                            this.report_error("Disconnect failed", &e.message, cx)
                        })
                        .ok();
                    }
                    true
                }
                Ok(None) => true,
                Err(_) => false,
            }
        })
    }

    fn disconnect_now(&mut self, id: &str, cx: &mut Context<Self>) -> Option<ApiFuture<()>> {
        self.stop_recovery(id, cx);
        self.wanted.remove(id);
        self.clear_failure_toast(id, cx);
        self.state.insert(id.to_owned(), ConnState::Disconnected);
        self.latency_ms.remove(id);
        self.retry_at.remove(id);
        cx.emit(ConnectionsEvent::Disconnected { id: id.to_owned() });
        cx.notify();
        self.api.as_ref().map(|api| api.disconnect(id))
    }

    /// Installs Phase 6's guard: asked before disconnecting or reconnecting
    /// a connection (it decides whether a transaction is open).
    pub fn set_close_guard(&mut self, guard: Option<CloseGuard>) {
        self.close_guard = guard;
    }

    /// Phase 7 (session restore, session.ts `startSession`): brings back the
    /// connections that were open at the last quit, retrying while the
    /// server can't be reached (`RETRY_DELAYS_MS`). They are wanted again at
    /// once (a quit before they are back keeps them for the next launch).
    /// `selected` (the selected database's connection) may ask for a
    /// password; the others are quiet (a refused login unwants them).
    /// Returns the selected one's recovery (true once connected).
    pub fn restore(
        &mut self,
        ids: Vec<String>,
        selected: Option<&str>,
        cx: &mut Context<Self>,
    ) -> Option<Task<bool>> {
        let mut selected_task = None;
        for id in ids {
            if self.get(&id).is_none() {
                continue;
            }
            self.wanted.insert(id.clone());
            let is_selected = selected == Some(id.as_str());
            let task = self.recover(
                &id,
                RecoverOpts {
                    reason: RecoverReason::Restore,
                    quiet: !is_selected,
                    background: false,
                },
                cx,
            );
            if is_selected {
                selected_task = Some(task);
            } else {
                task.detach();
            }
        }
        cx.notify();
        selected_task
    }

    // ── Staying connected ─────────────────────────────────────────────────

    /// Brings a connection back (store.ts `recover`): resolves true once
    /// connected, false when given up. Joins a recovery in progress.
    pub fn recover(&mut self, id: &str, opts: RecoverOpts, cx: &mut Context<Self>) -> Task<bool> {
        if self.get(id).is_none() || self.api.is_none() {
            return Task::ready(false);
        }
        let (tx, rx) = oneshot::channel();
        self.recovery_waiters
            .entry(id.to_owned())
            .or_default()
            .push(tx);
        let effects = self.recoveries.recover(id, opts);
        self.run_effects(id, effects, cx);
        cx.background_executor()
            .spawn(async move { rx.await.unwrap_or(false) })
    }

    /// Runs the pending attempt of a recovery now (store.ts `retryNow`); true
    /// if that attempt got the connection back. Not recovering: whether it
    /// is connected.
    pub fn retry_now(&mut self, id: &str, cx: &mut Context<Self>) -> Task<bool> {
        if !self.recoveries.is_recovering(id) {
            return Task::ready(self.is_connected(id));
        }
        let (tx, rx) = oneshot::channel();
        self.attempt_waiters
            .entry(id.to_owned())
            .or_default()
            .push(tx);
        let effects = self.recoveries.retry_now(id);
        self.run_effects(id, effects, cx);
        cx.background_executor()
            .spawn(async move { rx.await.unwrap_or(false) })
    }

    /// Checks every wanted connection (store.ts `checkConnections`; after sleep,
    /// or when the network changed).
    pub fn check_connections(&mut self, cx: &mut Context<Self>) {
        let wanted: Vec<(String, ConnLinkState)> = self
            .wanted
            .iter()
            .map(|id| (id.clone(), self.link_state(id)))
            .collect();
        for (id, effects) in self.recoveries.check(&wanted) {
            self.run_effects(&id, effects, cx);
        }
    }

    /// A [`LinkEvent`] from the backend (store.ts `onLinkEvent`).
    pub fn on_link_event(&mut self, event: &LinkEvent, cx: &mut Context<Self>) {
        let id = match event {
            LinkEvent::Lost { connection_id } | LinkEvent::TxLost { connection_id, .. } => {
                connection_id.clone()
            }
        };
        let state = self.link_state(&id);
        let (_, effects) = self.recoveries.on_link_event(event, state);
        self.run_effects(&id, effects, cx);
    }

    /// Starts watching for wake-ups (the 5 s tick noticing it ran late, and
    /// the macOS did-wake notification) and network changes; each checks
    /// the wanted connections (store.ts `watchConnections`). Once.
    pub fn watch(&mut self, cx: &mut Context<Self>) {
        if self.monitor.is_some() {
            return;
        }
        self.tasks.push(cx.spawn(async move |this, cx| {
            let mut last = wall_ms();
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(rc::WAKE_TICK_MS))
                    .await;
                let now = wall_ms();
                if rc::woke_up(last, now) {
                    tracing::info!("woke up (timer gap): checking connections");
                    if this
                        .update(cx, |this, cx| this.check_connections(cx))
                        .is_err()
                    {
                        break;
                    }
                }
                last = now;
            }
        }));
        let (monitor, mut events) = crate::platform::watch();
        self.monitor = Some(monitor);
        self.tasks.push(cx.spawn(async move |this, cx| {
            while let Some(event) = events.next().await {
                tracing::info!(?event, "checking connections");
                let alive = this.update(cx, |this, cx| this.on_platform_event(event, cx));
                if alive.is_err() {
                    break;
                }
            }
        }));
    }

    /// A wake-up checks at once; a network change once it settles
    /// ([`NETWORK_SETTLE`]).
    pub fn on_platform_event(
        &mut self,
        event: crate::platform::PlatformEvent,
        cx: &mut Context<Self>,
    ) {
        match event {
            crate::platform::PlatformEvent::Woke => self.check_connections(cx),
            crate::platform::PlatformEvent::NetworkChanged => {
                self.network_debounce = Some(cx.spawn(async move |this, cx| {
                    cx.background_executor().timer(NETWORK_SETTLE).await;
                    this.update(cx, |this, cx| {
                        this.network_debounce = None;
                        this.check_connections(cx);
                    })
                    .ok();
                }));
            }
        }
    }

    /// store.ts `stopRecovery`.
    fn stop_recovery(&mut self, id: &str, cx: &mut Context<Self>) {
        if self.recoveries.stop(id) {
            self.timers.remove(id);
            let ok = self.is_connected(id);
            self.resolve_waiters(id, ok);
            cx.notify();
        }
    }

    fn resolve_waiters(&mut self, id: &str, ok: bool) {
        for w in self.recovery_waiters.remove(id).unwrap_or_default() {
            let _ = w.send(ok);
        }
        self.resolve_attempt(id, ok);
    }

    fn resolve_attempt(&mut self, id: &str, ok: bool) {
        for w in self.attempt_waiters.remove(id).unwrap_or_default() {
            let _ = w.send(ok);
        }
    }

    /// Performs what [`Recoveries`] asks for.
    fn run_effects(&mut self, id: &str, effects: Vec<Effect>, cx: &mut Context<Self>) {
        if effects.is_empty() {
            return;
        }
        for effect in effects {
            match effect {
                Effect::Ping { token } => self.spawn_ping(id, token, cx),
                Effect::Connect { token, quiet } => {
                    let done = self.connect_now(
                        id,
                        None,
                        ConnectOpts {
                            quiet,
                            toast: false,
                        },
                        cx,
                    );
                    let id = id.to_owned();
                    cx.spawn(async move |this, cx| {
                        let result = done.await;
                        this.update(cx, |this, cx| {
                            let now = this.now_ms(cx);
                            let name = this.name(&id);
                            let next = this.recoveries.on_connect(
                                &id,
                                token,
                                &name,
                                result.as_ref().map(|_| ()),
                                now,
                            );
                            this.run_effects(&id, next, cx);
                        })
                        .ok();
                    })
                    .detach();
                }
                Effect::Schedule { token, delay_ms } => {
                    // The attempt failed: whoever waited for it learns so.
                    self.resolve_attempt(id, false);
                    let key = id.to_owned();
                    let timer = cx.spawn(async move |this, cx| {
                        cx.background_executor()
                            .timer(Duration::from_millis(delay_ms))
                            .await;
                        this.update(cx, |this, cx| {
                            let next = this.recoveries.on_timer(&key, token);
                            this.run_effects(&key, next, cx);
                        })
                        .ok();
                    });
                    self.timers.insert(id.to_owned(), timer);
                }
                Effect::CancelTimer => {
                    self.timers.remove(id);
                }
                Effect::SetState(state) => self.apply_link_state(id, state, cx),
                Effect::Toast(toast) => self.push_toast(toast, cx),
                Effect::ClearFailureToast => self.clear_failure_toast(id, cx),
                Effect::TxLost { databases } => self.note_lost_transactions(id, databases, cx),
                Effect::Unwant => {
                    self.wanted.remove(id);
                }
                Effect::Finish { ok } => self.resolve_waiters(id, ok),
            }
        }
        cx.notify();
    }

    fn spawn_ping(&mut self, id: &str, token: u64, cx: &mut Context<Self>) {
        let Some(api) = self.api.clone() else { return };
        let started = cx.background_executor().now();
        let call = api.ping(id);
        let id = id.to_owned();
        cx.spawn(async move |this, cx| {
            let result = call.await;
            this.update(cx, |this, cx| {
                let now = this.now_ms(cx);
                let name = this.name(&id);
                let next = match &result {
                    Ok(info) => {
                        let ms = cx
                            .background_executor()
                            .now()
                            .saturating_duration_since(started);
                        this.latency_ms.insert(
                            id.clone(),
                            u64::try_from(ms.as_millis()).unwrap_or(u64::MAX),
                        );
                        this.recoveries
                            .on_ping(&id, token, &name, Ok(&info.lost_tx), now)
                    }
                    Err(e) => {
                        if e.code.as_deref() != Some(rc::codes::NOT_CONNECTED) {
                            this.last_error.insert(id.clone(), e.message.clone());
                        }
                        this.recoveries.on_ping(&id, token, &name, Err(e), now)
                    }
                };
                this.run_effects(&id, next, cx);
            })
            .ok();
        })
        .detach();
    }

    fn apply_link_state(&mut self, id: &str, state: ConnLinkState, cx: &mut Context<Self>) {
        let next = match state {
            ConnLinkState::Disconnected => ConnState::Disconnected,
            ConnLinkState::Connecting => ConnState::Connecting,
            ConnLinkState::Connected => {
                let (server_version, databases) = self.info.get(id).map_or_else(
                    || (String::new(), Vec::new()),
                    |i| (i.server_version.clone(), i.databases.clone()),
                );
                ConnState::Connected {
                    server_version,
                    databases,
                }
            }
            ConnLinkState::Reconnecting {
                attempt,
                next_at_ms,
            } => {
                self.retry_at.insert(id.to_owned(), next_at_ms);
                ConnState::Reconnecting { attempt }
            }
            ConnLinkState::Failed => ConnState::Failed {
                message: self
                    .last_error
                    .get(id)
                    .cloned()
                    .unwrap_or_else(|| "Could not connect".into()),
            },
        };
        if !matches!(next, ConnState::Connected { .. }) {
            self.latency_ms.remove(id);
        }
        if !matches!(next, ConnState::Reconnecting { .. }) {
            self.retry_at.remove(id);
        }
        tracing::debug!(id, state = next.as_str(), "connection state");
        self.state.insert(id.to_owned(), next);
        cx.notify();
    }

    // ── Toasts ────────────────────────────────────────────────────────────

    fn push_toast(&mut self, toast: rc::Toast, cx: &mut Context<Self>) {
        tracing::debug!(title = %toast.title, key = ?toast.key, "connection toast");
        let Some(toasts) = self.toasts.clone() else {
            return;
        };
        let kind = match toast.kind {
            rc::ToastKind::Info => ToastKind::Info,
            rc::ToastKind::Success => ToastKind::Success,
            rc::ToastKind::Error => ToastKind::Error,
        };
        let mut new = NewToast::new(kind, toast.title);
        if let Some(key) = toast.key {
            new = new.key(key);
        }
        if let Some(body) = toast.body {
            new = new.body(body);
        }
        if let Some(action) = toast.action {
            let label = action.label();
            let rc::ToastAction::Retry { connection_id } = action;
            let this = cx.entity().downgrade();
            new = new.action(label, move |_, cx| {
                let id = connection_id.clone();
                this.update(cx, |this, cx| {
                    this.recover(&id, RecoverOpts::default(), cx).detach();
                })
                .ok();
            });
        }
        let ttl = (toast.ttl_ms > 0).then(|| Duration::from_millis(toast.ttl_ms));
        toasts.update(cx, |t, cx| {
            t.push_for(new, ttl, cx);
        });
    }

    /// Drops the connection's status toast if it is a failure (store.ts `clearFailureToast`).
    fn clear_failure_toast(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(toasts) = self.toasts.clone() else {
            return;
        };
        let key = toast_key(id);
        toasts.update(cx, |t, cx| {
            if let Some(found) = t.find_key(&key).filter(|t| t.kind == ToastKind::Error) {
                let found = found.id;
                t.dismiss(found, cx);
            }
        });
    }

    fn report_error(&mut self, title: &str, message: &str, cx: &mut Context<Self>) {
        if let Some(toasts) = self.toasts.clone() {
            let (title, message) = (title.to_owned(), message.to_owned());
            toasts.update(cx, |t, cx| {
                t.report_error(title, message, cx);
            });
        }
    }

    /// store.ts `noteLostTransactions`: a toast per database, its tx state idle.
    fn note_lost_transactions(&mut self, id: &str, databases: Vec<String>, cx: &mut Context<Self>) {
        let name = self.name(id);
        for database in databases {
            self.push_toast(rc::tx_lost_toast(&name, &database), cx);
            // Transactions follows this event (its session goes idle).
            cx.emit(ConnectionsEvent::TxLost {
                id: id.to_owned(),
                database,
            });
        }
    }
}

/// The model went away while a call ran.
fn gone(_: anyhow::Error) -> CmdError {
    CmdError::msg("The app is closing")
}

fn no_backend() -> CmdError {
    CmdError::msg("No backend")
}

fn wall_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

/// JavaScript's `encodeURIComponent`.
fn encode_uri_component(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-_.!~*'()".contains(&b) {
            out.push(char::from(b));
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

// ── Dev hook ──────────────────────────────────────────────────────────────

/// `BOA_DEV_CONNECTION=1` (feature `dev-hooks`): a "Local dev" connection to the
/// local server (localhost:5432, boa / boa, boa_dev, password saved) for
/// development runs and screenshots, connected at launch.
#[cfg(feature = "dev-hooks")]
pub mod dev {
    use std::path::Path;

    use boa_core::connections::{self as store, ConnConfig, SslMode};
    use gpui::{App, Entity};

    use super::Connections;
    use crate::models::prefs::Prefs;

    /// The env var.
    pub const ENV: &str = "BOA_DEV_CONNECTION";
    /// The seeded connection's id.
    pub const DEV_CONNECTION_ID: &str = "local-dev";
    /// `BOA_DEV_RECONNECT=1` also seeds this one (localhost:1, nothing
    /// listens) and starts recovering it at launch: "reconnecting…", then
    /// after the ~2 min backoff the "Lost connection" toast with Retry.
    pub const RECONNECT_ENV: &str = "BOA_DEV_RECONNECT";
    pub const UNREACHABLE_ID: &str = "dev-unreachable";

    /// The seeded config.
    pub fn config() -> ConnConfig {
        ConnConfig {
            kind: Default::default(),
            id: DEV_CONNECTION_ID.into(),
            name: "Local dev".into(),
            host: "localhost".into(),
            port: 5432,
            user: "boa".into(),
            database: "boa_dev".into(),
            ssl_mode: SslMode::Prefer,
            read_only: false,
            environment: None,
            color: None,
            ssh: None,
        }
    }

    /// With `BOA_DEV_CONNECTION=1`: writes the "Local dev" connection into
    /// `dir`'s connections.json when it has none (never into a config dir
    /// that already has connections). Returns its id when it is there.
    pub fn seed(dir: &Path) -> Option<String> {
        if std::env::var(ENV).ok().as_deref() != Some("1") {
            return None;
        }
        let list = store::list(dir).ok()?;
        if list.is_empty() {
            if let Err(e) = store::upsert(dir, config(), Some("boa".into()), true) {
                tracing::warn!("{ENV}: seeding failed: {}", e.message);
                return None;
            }
            if std::env::var(RECONNECT_ENV).ok().as_deref() == Some("1") {
                let unreachable = ConnConfig {
                    kind: Default::default(),
                    id: UNREACHABLE_ID.into(),
                    name: "Unreachable".into(),
                    port: 1,
                    ..config()
                };
                let _ = store::upsert(dir, unreachable, Some("x".into()), true);
            }
        } else if !list.iter().any(|c| c.config.id == DEV_CONNECTION_ID) {
            tracing::info!("{ENV}: the config dir has connections; not seeding");
            return None;
        }
        Some(DEV_CONNECTION_ID.into())
    }

    /// Connects `id` once the saved list is loaded, and expands it in the explorer.
    pub fn connect_at_launch(
        connections: &Entity<Connections>,
        prefs: &Entity<Prefs>,
        id: String,
        cx: &mut App,
    ) {
        prefs.update(cx, |p, cx| p.update(cx, |p| p.set_expanded(&id, true)));
        let load = connections.update(cx, |c, cx| c.load(cx));
        let connections = connections.downgrade();
        cx.spawn(async move |cx| {
            load.await;
            connections
                .update(cx, |c, cx| {
                    c.connect(&id, cx);
                    if c.get(UNREACHABLE_ID).is_some() {
                        c.recover(UNREACHABLE_ID, Default::default(), cx).detach();
                    }
                })
                .ok();
        })
        .detach();
    }
}

#[cfg(test)]
mod tests;
