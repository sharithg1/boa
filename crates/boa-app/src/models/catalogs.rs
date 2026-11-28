//! [`Catalogs`]: introspection per (connection, database): store.ts
//! `loadDatabase`, `setScope` and the scope helpers, ScopePicker, Sidebar.
//!
//! Like DataGrip, only the picked databases and schemas are shown and
//! introspected (prefs `scopes`; defaults: the connection's own database, the
//! `public` schema). For each database this holds every schema name (the
//! scope picker's "N of M"), the introspected schemas in scope
//! (`Backend::introspect`: tables, views, columns, keys, indexes, row
//! estimates), and whether a load is running or failed.
//!
//! - [`Catalogs::load`] (`loadDatabase`): lists the schema names (once, or
//!   again with `force`), introspects the picked schemas not loaded yet and
//!   drops the unpicked ones (`force`: re-reads everything). Re-runs when the
//!   pick changed meanwhile, or when a forced load was asked for meanwhile.
//! - Scope changes (prefs `scopes`, from the picker, an import or a reset) load
//!   what became visible (`setScope`): a database's own pick reloads it; a
//!   connection's pick loads its newly shown databases that are expanded.
//! - When a connection connects (or reconnects) its schema names are re-read
//!   on the next load (what was introspected is kept, as the TS kept it), and
//!   its first shown database plus the shown ones left expanded load (store.ts
//!   `connect`). Catalogs of deleted connections are dropped.
//!
//! Redis databases hold their keys instead (`Backend::redis_keys`, in
//! [`Catalog::redis`]); they have no schemas.
//!
//! Feeds the explorer, the right panel, completion and highlighting.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::Arc;

use boa_core::model::{Schema, Table};
use boa_core::redis::RedisKeys;
use boa_core::state::prefs::{Prefs as CorePrefs, Scope};
use boa_core::state::session::{ScopeSel, database_schemas, schema_scope_key, shown_databases};
use gpui::{Context, Subscription, Task};

use super::prefs::PrefsChanged;
use crate::app_state::AppState;

/// A (connection id, database) pair (store.ts `dbKey`).
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct DbKey {
    pub connection_id: String,
    pub database: String,
}

impl DbKey {
    pub fn new(connection_id: impl Into<String>, database: impl Into<String>) -> Self {
        Self {
            connection_id: connection_id.into(),
            database: database.into(),
        }
    }

    /// The prefs `scopes` key of this database's schema pick (`connectionId/database`).
    pub fn scope_key(&self) -> String {
        schema_scope_key(&self.connection_id, &self.database)
    }
}

/// Introspection state of one database.
#[derive(Debug, Clone, Default)]
pub struct Catalog {
    /// All schema names, in server order (for the scope picker's "N of M").
    pub schema_names: Vec<String>,
    /// Introspected schemas in scope, in server order.
    pub schemas: Arc<Vec<Schema>>,
    pub loading: bool,
    /// The last load's error (also shown as a toast).
    pub error: Option<String>,
    /// `schema_names` has been read (store.ts `schemaNames` is set).
    pub names_loaded: bool,
    /// Introspected at least once.
    pub loaded: bool,
    /// A forced load was asked for while one was running: run it after.
    pub(crate) force_pending: bool,
    /// Redis databases: the keys (instead of schemas).
    pub redis: Option<Arc<RedisKeys>>,
    /// Redis databases: how many keys to list (0: [`boa_core::redis::KEY_LIMIT`]);
    /// "Load more keys" raises it.
    pub redis_limit: usize,
}

impl Catalog {
    /// An introspected table or view.
    pub fn find_table(&self, schema: &str, table: &str) -> Option<&Table> {
        self.schemas
            .iter()
            .find(|s| s.name == schema)?
            .tables
            .iter()
            .find(|t| t.name == table)
    }

    /// Tables and views in all loaded schemas.
    pub fn table_count(&self) -> usize {
        self.schemas.iter().map(|s| s.tables.len()).sum()
    }
}

/// `scopes[key]` as a borrowed pick.
pub fn scope_sel(scope: Option<&Scope>) -> Option<ScopeSel<'_>> {
    scope.map(|s| match s {
        Scope::All => ScopeSel::All,
        Scope::Some(names) => ScopeSel::Names(names),
    })
}

/// The database's picked schemas, in server order (store.ts `databaseSchemas`).
pub fn picked_schemas(prefs: &CorePrefs, key: &DbKey, names: &[String]) -> Vec<String> {
    database_schemas(scope_sel(prefs.scopes.get(&key.scope_key())), names)
}

/// The connection's picked databases, in server order (store.ts `shownDatabases`).
pub fn picked_databases(
    prefs: &CorePrefs,
    connection_id: &str,
    own_database: &str,
    names: &[String],
) -> Vec<String> {
    shown_databases(
        scope_sel(prefs.scopes.get(connection_id)),
        own_database,
        names,
    )
}

/// The `scopes` keys whose pick differs between two prefs.
pub fn changed_scope_keys(prev: &CorePrefs, now: &CorePrefs) -> Vec<String> {
    let keys: BTreeSet<&String> = prev.scopes.keys().chain(now.scopes.keys()).collect();
    keys.into_iter()
        .filter(|k| prev.scopes.get(*k) != now.scopes.get(*k))
        .cloned()
        .collect()
}

/// Catalogs of every open database.
#[derive(Default)]
pub struct Catalogs {
    pub catalogs: HashMap<DbKey, Catalog>,
    /// Connections last seen connected (to spot connects and reconnects).
    connected: HashSet<String>,
    tasks: HashMap<DbKey, Task<()>>,
    watching: bool,
    /// Pretend connections of tests (no server behind them): never loaded.
    pub(crate) offline: HashSet<String>,
    _subscriptions: Vec<Subscription>,
}

impl std::fmt::Debug for Catalogs {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Catalogs")
            .field("catalogs", &self.catalogs)
            .field("connected", &self.connected)
            .finish_non_exhaustive()
    }
}

impl Catalogs {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn get(&self, key: &DbKey) -> Option<&Catalog> {
        self.catalogs.get(key)
    }

    /// An introspected table or view (store.ts `findTable`).
    pub fn find_table(&self, key: &DbKey, schema: &str, table: &str) -> Option<&Table> {
        self.catalogs.get(key)?.find_table(schema, table)
    }

    /// Whether `key` is being loaded.
    pub fn is_loading(&self, key: &DbKey) -> bool {
        self.catalogs.get(key).is_some_and(|c| c.loading)
    }

    /// Starts following the prefs (scope changes) and the connections
    /// (connects / reconnects). Called once by [`AppState::init`].
    pub fn watch(&mut self, cx: &mut Context<Self>) {
        if self.watching {
            return;
        }
        let Some(state) = AppState::try_global(cx).cloned() else {
            return;
        };
        self.watching = true;
        self._subscriptions.push(cx.subscribe(
            &state.prefs,
            |this, prefs, event: &PrefsChanged, cx| {
                let now = prefs.read(cx).get().clone();
                this.scopes_changed(&event.prev, &now, cx);
            },
        ));
        self._subscriptions
            .push(cx.observe(&state.connections, |this, _, cx| {
                this.connections_changed(cx)
            }));
        self.connections_changed(cx);
    }

    /// `setScope`'s second half: loads what a changed pick made visible.
    fn scopes_changed(&mut self, prev: &CorePrefs, now: &CorePrefs, cx: &mut Context<Self>) {
        let Some(state) = AppState::try_global(cx).cloned() else {
            return;
        };
        for key in changed_scope_keys(prev, now) {
            if let Some((connection_id, database)) = key.split_once('/') {
                self.load(DbKey::new(connection_id, database), false, cx);
                continue;
            }
            let (own, names) = {
                let connections = state.connections.read(cx);
                let Some(conn) = connections.get(&key) else {
                    continue;
                };
                (conn.config.default_database(), connections.databases(&key))
            };
            for database in picked_databases(now, &key, &own, &names) {
                if now.is_expanded(&schema_scope_key(&key, &database)) {
                    self.load(DbKey::new(key.clone(), database), false, cx);
                }
            }
        }
    }

    /// Spots connections that just connected (store.ts `connect`: names
    /// re-read, the first shown database and the expanded ones loaded) and
    /// forgets catalogs of deleted connections.
    fn connections_changed(&mut self, cx: &mut Context<Self>) {
        let Some(state) = AppState::try_global(cx).cloned() else {
            return;
        };
        type Newly = Vec<(String, String, Vec<String>)>;
        let (saved, newly): (HashSet<String>, Newly) = {
            let connections = state.connections.read(cx);
            let saved = connections
                .saved
                .iter()
                .map(|c| c.config.id.clone())
                .collect();
            let mut newly = Vec::new();
            let mut now = HashSet::new();
            for c in &connections.saved {
                let id = &c.config.id;
                if connections.is_connected(id) {
                    now.insert(id.clone());
                    if !self.connected.contains(id) {
                        newly.push((
                            id.clone(),
                            c.config.default_database(),
                            connections.databases(id),
                        ));
                    }
                }
            }
            self.connected = now;
            (saved, newly)
        };
        let before = self.catalogs.len();
        self.catalogs
            .retain(|k, _| saved.contains(&k.connection_id));
        self.tasks.retain(|k, _| saved.contains(&k.connection_id));
        let mut changed = before != self.catalogs.len();
        let prefs = state.prefs.read(cx).get().clone();
        for (id, own, names) in newly {
            // Keep what was introspected; re-read the names on the next load.
            for (key, catalog) in self.catalogs.iter_mut() {
                if key.connection_id == id && !catalog.loading {
                    catalog.names_loaded = false;
                    changed = true;
                }
            }
            for (i, database) in picked_databases(&prefs, &id, &own, &names)
                .into_iter()
                .enumerate()
            {
                if i == 0 || prefs.is_expanded(&schema_scope_key(&id, &database)) {
                    self.load(DbKey::new(id.clone(), database), false, cx);
                }
            }
        }
        if changed {
            cx.notify();
        }
    }

    /// Lists more of a Redis database's keys (the explorer's "Load more keys").
    pub fn load_more_keys(&mut self, key: DbKey, cx: &mut Context<Self>) {
        if let Some(c) = self.catalogs.get_mut(&key) {
            if c.loading {
                return;
            }
            let shown = c.redis.as_ref().map_or(0, |k| k.keys.len());
            c.redis_limit = shown.max(c.redis_limit).max(boa_core::redis::KEY_LIMIT)
                + boa_core::redis::KEY_STEP;
        }
        self.load(key, true, cx);
    }

    /// Re-reads a database from scratch (the explorer's Refresh).
    pub fn refresh(&mut self, key: DbKey, cx: &mut Context<Self>) {
        self.load(key, true, cx);
    }

    /// (Re)loads a database's schemas in scope (store.ts `loadDatabase`; `force`:
    /// ignore the cache). Does nothing unless its connection is connected and
    /// lists the database.
    pub fn load(&mut self, key: DbKey, force: bool, cx: &mut Context<Self>) {
        if self.offline.contains(&key.connection_id) {
            return;
        }
        let Some(state) = AppState::try_global(cx).cloned() else {
            return;
        };
        let is_redis = {
            let connections = state.connections.read(cx);
            if !connections.is_connected(&key.connection_id)
                || !connections
                    .databases(&key.connection_id)
                    .contains(&key.database)
            {
                return;
            }
            connections
                .get(&key.connection_id)
                .is_some_and(|c| c.config.kind.is_redis())
        };
        let catalog = self.catalogs.entry(key.clone()).or_default();
        if catalog.loading {
            catalog.force_pending |= force;
            return;
        }
        catalog.loading = true;
        catalog.error = None;
        let redis_limit = catalog.redis_limit.max(boa_core::redis::KEY_LIMIT);
        let known_names = (catalog.names_loaded && !force).then(|| catalog.schema_names.clone());
        cx.notify();

        let backend = state.backend.clone();
        let task_key = key.clone();
        if is_redis {
            let task = cx.spawn(async move |this, cx| {
                let result = backend
                    .redis_keys(&key.connection_id, &key.database, redis_limit)
                    .await;
                this.update(cx, |this, cx| match result {
                    Ok(keys) => {
                        let Some(c) = this.catalogs.get_mut(&key) else {
                            return;
                        };
                        c.redis = Some(Arc::new(keys));
                        c.names_loaded = true;
                        c.loaded = true;
                        c.loading = false;
                        let again = std::mem::take(&mut c.force_pending);
                        this.tasks.remove(&key);
                        cx.notify();
                        if again {
                            this.load(key.clone(), true, cx);
                        }
                    }
                    Err(e) => this.failed(&key, e.message, cx),
                })
                .ok();
            });
            self.tasks.insert(task_key, task);
            return;
        }
        let task = cx.spawn(async move |this, cx| {
            let DbKey {
                connection_id: id,
                database,
            } = key.clone();
            // 1. The schema names (cheap).
            let names = match known_names {
                Some(names) => names,
                None => match backend.list_schemas(&id, &database).await {
                    Ok(names) => {
                        let stored = this.update(cx, |this, cx| {
                            if let Some(c) = this.catalogs.get_mut(&key) {
                                c.schema_names = names.clone();
                                c.names_loaded = true;
                                cx.notify();
                            }
                        });
                        if stored.is_err() {
                            return;
                        }
                        names
                    }
                    Err(e) => {
                        this.update(cx, |this, cx| this.failed(&key, e.message, cx))
                            .ok();
                        return;
                    }
                },
            };
            // 2. The picked schemas not loaded yet (all of them with force).
            let Ok((want, kept)) = this.update(cx, |this, cx| {
                let prefs = AppState::try_global(cx)
                    .map(|s| s.prefs.read(cx).get().clone())
                    .unwrap_or_default();
                let want = picked_schemas(&prefs, &key, &names);
                let kept: Vec<Schema> = if force {
                    Vec::new()
                } else {
                    this.catalogs
                        .get(&key)
                        .map(|c| {
                            c.schemas
                                .iter()
                                .filter(|s| want.contains(&s.name))
                                .cloned()
                                .collect()
                        })
                        .unwrap_or_default()
                };
                (want, kept)
            }) else {
                return;
            };
            let missing: Vec<String> = want
                .iter()
                .filter(|n| !kept.iter().any(|s| &s.name == *n))
                .cloned()
                .collect();
            let fetched = if missing.is_empty() {
                Vec::new()
            } else {
                match backend.introspect(&id, &database, missing).await {
                    Ok(fetched) => fetched,
                    Err(e) => {
                        this.update(cx, |this, cx| this.failed(&key, e.message, cx))
                            .ok();
                        return;
                    }
                }
            };
            let mut by_name: HashMap<String, Schema> = kept
                .into_iter()
                .chain(fetched)
                .map(|s| (s.name.clone(), s))
                .collect();
            let schemas: Vec<Schema> = names.iter().filter_map(|n| by_name.remove(n)).collect();
            // 3. Store; re-run if the pick changed meanwhile (or a refresh waited).
            this.update(cx, |this, cx| {
                let Some(c) = this.catalogs.get_mut(&key) else {
                    return;
                };
                c.schemas = Arc::new(schemas);
                c.loaded = true;
                c.loading = false;
                let force_pending = std::mem::take(&mut c.force_pending);
                let names = c.schema_names.clone();
                this.tasks.remove(&key);
                cx.notify();
                let now = AppState::try_global(cx)
                    .map(|s| picked_schemas(s.prefs.read(cx).get(), &key, &names))
                    .unwrap_or_default();
                if force_pending {
                    this.load(key.clone(), true, cx);
                } else if now != want {
                    this.load(key.clone(), false, cx);
                }
            })
            .ok();
        });
        self.tasks.insert(task_key, task);
    }

    /// A load failed: keep what was there and show the error (store.ts
    /// `reportError("Could not read <database>", e)`).
    fn failed(&mut self, key: &DbKey, message: String, cx: &mut Context<Self>) {
        if let Some(c) = self.catalogs.get_mut(key) {
            c.loading = false;
            c.force_pending = false;
            c.error = Some(message.clone());
        }
        self.tasks.remove(key);
        cx.notify();
        if let Some(state) = AppState::try_global(cx) {
            let toasts = state.toasts.clone();
            let title = format!("Could not read {}", key.database);
            cx.defer(move |cx| {
                toasts.update(cx, |t, cx| {
                    t.report_error(title, message, cx);
                });
            });
        }
    }
}
