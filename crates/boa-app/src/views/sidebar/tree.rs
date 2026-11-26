//! The explorer's rows, as Sidebar.tsx `useNodes` builds them: the visible
//! nodes, depth first, honouring the expansion saved in prefs `treeExpanded`.
//! Pure (no GPUI), so the tree, its node ids and the keyboard navigation are
//! unit tested.
//!
//! Node ids are the TS `nodeId`s exactly (they are the `treeExpanded` keys, so
//! imported prefs keep working): `conn`, `conn/#status`, `conn/#none`,
//! `conn/db`, `conn/db/schema`, `conn/db/schema/#table` (or `#view`),
//! `…/#table/name`, `…/name/column`, `…/name/#keys` (`#foreign keys`,
//! `#indexes`, `#checks`) and `…/#keys/item`.

use boa_core::connections::Environment as SavedEnvironment;
use boa_core::grid::conn_tag::{ConnectionColor, Environment, connection_color};
use boa_core::grid::format::compact_count;
use boa_core::grid::table_meta::{KeyKind, MetaFolderKind, column_keys, meta_folders};
use boa_core::model::TableKind;
use boa_core::state::prefs::{Prefs, Scope};
use boa_core::state::session::schema_scope_key;

use crate::models::catalogs::{Catalogs, DbKey, picked_databases, picked_schemas};
use crate::models::connections::ConnState;

/// What the tree needs of a saved connection.
#[derive(Debug, Clone, PartialEq)]
pub struct ConnInfo {
    pub id: String,
    pub name: String,
    /// The database it connects to first (the default database pick).
    pub database: String,
    pub user: String,
    pub host: String,
    pub port: u16,
    pub state: ConnState,
    pub environment: Option<Environment>,
    pub color: Option<ConnectionColor>,
    pub read_only: bool,
}

/// The saved environment as connTag's.
pub fn environment(env: Option<SavedEnvironment>) -> Option<Environment> {
    env.map(|e| match e {
        SavedEnvironment::Development => Environment::Development,
        SavedEnvironment::Test => Environment::Test,
        SavedEnvironment::Staging => Environment::Staging,
        SavedEnvironment::Production => Environment::Production,
    })
}

impl ConnInfo {
    /// From a saved connection and its live state.
    pub fn new(saved: &boa_core::connections::SavedConnection, state: ConnState) -> Self {
        let c = &saved.config;
        let env = environment(c.environment);
        Self {
            id: c.id.clone(),
            name: c.name.clone(),
            database: c.database.clone(),
            user: c.user.clone(),
            host: c.host.clone(),
            port: c.port,
            state,
            environment: env,
            color: connection_color(env, c.color.as_deref()),
            read_only: c.read_only,
        }
    }

    pub fn connected(&self) -> bool {
        matches!(self.state, ConnState::Connected { .. })
    }

    /// The server's databases (connected only).
    pub fn databases(&self) -> &[String] {
        match &self.state {
            ConnState::Connected { databases, .. } => databases,
            _ => &[],
        }
    }
}

/// encodeURIComponent: everything but `A-Z a-z 0-9 - _ . ! ~ * ' ( )` is
/// percent-encoded (UTF-8).
pub fn encode_uri_component(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-_.!~*'()".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// store.ts `connectionString`: `postgresql://user@host:port/database` (no password).
pub fn connection_string(c: &ConnInfo, database: Option<&str>) -> String {
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
    format!(
        "postgresql://{auth}{}{port}/{}",
        c.host,
        database.unwrap_or(&c.database)
    )
}

/// A row-count estimate as the explorer shows it (`compactCount`: 182k, 1.2M).
pub fn row_count(rows: i64) -> String {
    if rows < 0 {
        rows.to_string()
    } else {
        compact_count(rows as u64)
    }
}

/// Which list a scope picker edits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScopeKind {
    Databases,
    Schemas,
}

impl ScopeKind {
    pub fn noun(self) -> &'static str {
        match self {
            Self::Databases => "databases",
            Self::Schemas => "schemas",
        }
    }
}

/// The "N of M" badge of a connection (databases) or a database (schemas).
#[derive(Debug, Clone, PartialEq)]
pub struct ScopeBadge {
    pub kind: ScopeKind,
    /// The prefs `scopes` key it edits.
    pub key: String,
    /// Every name, in display order.
    pub items: Vec<String>,
    /// The shown names (resolved, so the default counts).
    pub selected: Vec<String>,
    /// The saved scope is `'*'`.
    pub is_all: bool,
    pub default_item: Option<String>,
}

/// The dim text after a row's label.
#[derive(Debug, Clone, PartialEq)]
pub enum Meta {
    None,
    /// A child count, a row count.
    Text(String),
    /// Connection failed (red).
    Failed,
    /// Connection lost, reconnecting (amber).
    Reconnecting,
    /// A connection's databases badge.
    Scope(ScopeBadge),
    /// A database: its schemas badge once the names are read, and "loading…".
    Database {
        badge: Option<ScopeBadge>,
        loading: bool,
    },
    /// A column's type (SQL type colour) and " not null".
    Column {
        ty: String,
        not_null: bool,
    },
    /// A key / index / check detail, monospace.
    Detail(String),
}

/// What a row is (and so what clicking, ↵ and its context menu do).
#[derive(Debug, Clone, PartialEq)]
pub enum NodeKind {
    Connection(Box<ConnInfo>),
    /// "Connecting…" / "Reconnecting…" / "Not connected" under an expanded,
    /// unconnected connection.
    Status,
    /// "No databases picked".
    NoDatabases,
    Database {
        key: DbKey,
        active: bool,
    },
    Schema {
        key: DbKey,
        schema: String,
    },
    /// The tables / views folder.
    Group {
        kind: TableKind,
    },
    Table {
        key: DbKey,
        schema: String,
        table: String,
        kind: TableKind,
    },
    Column {
        name: String,
        key: Option<KeyKind>,
    },
    /// keys / foreign keys / indexes / checks.
    Folder(MetaFolderKind),
    MetaItem {
        folder: MetaFolderKind,
        name: String,
        definition: String,
        key: Option<KeyKind>,
    },
}

/// One visible row.
#[derive(Debug, Clone, PartialEq)]
pub struct Node {
    /// Tree path; the expansion key.
    pub id: String,
    pub parent: Option<String>,
    pub depth: usize,
    pub kind: NodeKind,
    pub label: String,
    pub meta: Meta,
    pub expandable: bool,
    /// Highlighted as the open table / active database.
    pub current: bool,
    pub mono: bool,
    pub italic: bool,
}

impl Node {
    fn new(id: String, parent: Option<&str>, depth: usize, kind: NodeKind, label: &str) -> Self {
        Self {
            id,
            parent: parent.map(str::to_owned),
            depth,
            kind,
            label: label.to_owned(),
            meta: Meta::None,
            expandable: false,
            current: false,
            mono: false,
            italic: false,
        }
    }

    /// Click (and ↵ without an open action): tables open a preview tab,
    /// databases and schemas select the database.
    pub fn selects(&self) -> bool {
        matches!(
            self.kind,
            NodeKind::Database { .. } | NodeKind::Schema { .. } | NodeKind::Table { .. }
        )
    }

    /// Double-click / ↵ opens it (tables: a pinned tab) instead of toggling.
    pub fn opens(&self) -> bool {
        matches!(self.kind, NodeKind::Table { .. })
    }
}

/// The table tab showing in the active database, if any.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ActiveTable<'a> {
    pub schema: &'a str,
    pub table: &'a str,
    pub preview: bool,
}

/// Everything `useNodes` reads.
#[derive(Debug, Clone, Copy)]
pub struct TreeInput<'a> {
    /// In sidebar order.
    pub connections: &'a [ConnInfo],
    pub prefs: &'a Prefs,
    pub catalogs: &'a Catalogs,
    pub active: Option<&'a DbKey>,
    pub active_table: Option<ActiveTable<'a>>,
}

/// The visible rows, depth first, honouring expansion (Sidebar.tsx `useNodes`).
pub fn build(input: &TreeInput<'_>) -> Vec<Node> {
    let prefs = input.prefs;
    let expanded = |id: &str| prefs.is_expanded(id);
    let mut out = Vec::new();
    for conn in input.connections {
        let cid = conn.id.as_str();
        let connected = conn.connected();
        let mut node = Node::new(
            cid.to_owned(),
            None,
            0,
            NodeKind::Connection(Box::new(conn.clone())),
            &conn.name,
        );
        node.expandable = true;
        node.meta = match &conn.state {
            ConnState::Connected { databases, .. } => Meta::Scope(ScopeBadge {
                kind: ScopeKind::Databases,
                key: cid.to_owned(),
                items: databases.clone(),
                selected: picked_databases(prefs, cid, &conn.database, databases),
                is_all: prefs.scopes.get(cid) == Some(&Scope::All),
                default_item: Some(conn.database.clone()),
            }),
            ConnState::Failed { .. } => Meta::Failed,
            ConnState::Reconnecting { .. } => Meta::Reconnecting,
            _ => Meta::None,
        };
        out.push(node);
        if !expanded(cid) {
            continue;
        }
        if !connected {
            let label = match conn.state {
                ConnState::Connecting => "Connecting…",
                ConnState::Reconnecting { .. } => "Reconnecting…",
                _ => "Not connected",
            };
            out.push(Node::new(
                format!("{cid}/#status"),
                Some(cid),
                1,
                NodeKind::Status,
                label,
            ));
            continue;
        }
        let names = conn.databases();
        let shown = picked_databases(prefs, cid, &conn.database, names);
        if shown.is_empty() {
            out.push(Node::new(
                format!("{cid}/#none"),
                Some(cid),
                1,
                NodeKind::NoDatabases,
                "No databases picked: use the “of” badge",
            ));
        }
        for db in names {
            if !shown.contains(db) {
                continue;
            }
            let did = format!("{cid}/{db}");
            let key = DbKey::new(cid, db.as_str());
            let is_active = input.active == Some(&key);
            let catalog = input.catalogs.get(&key);
            let loading = catalog.is_some_and(|c| c.loading);
            let badge = catalog.filter(|c| c.names_loaded).map(|c| ScopeBadge {
                kind: ScopeKind::Schemas,
                key: schema_scope_key(cid, db),
                items: c.schema_names.clone(),
                selected: picked_schemas(prefs, &key, &c.schema_names),
                is_all: prefs.scopes.get(&schema_scope_key(cid, db)) == Some(&Scope::All),
                default_item: if c.schema_names.iter().any(|n| n == "public") {
                    Some("public".to_owned())
                } else {
                    c.schema_names.first().cloned()
                },
            });
            let picked: Vec<String> = badge
                .as_ref()
                .map(|b| b.selected.clone())
                .unwrap_or_default();
            let mut node = Node::new(
                did.clone(),
                Some(cid),
                1,
                NodeKind::Database {
                    key: key.clone(),
                    active: is_active,
                },
                db,
            );
            node.meta = if badge.is_some() || loading {
                Meta::Database { badge, loading }
            } else {
                Meta::None
            };
            node.expandable = true;
            node.current = is_active && input.active_table.is_none();
            out.push(node);
            if !expanded(&did) {
                continue;
            }
            let Some(catalog) = catalog else { continue };
            // Unpicked schemas drop at once; newly picked ones appear as they load.
            for schema in catalog.schemas.iter() {
                if !picked.contains(&schema.name) {
                    continue;
                }
                let sid = format!("{did}/{}", schema.name);
                let groups: Vec<(TableKind, Vec<&boa_core::model::Table>)> =
                    [TableKind::Table, TableKind::View]
                        .into_iter()
                        .map(|kind| {
                            (
                                kind,
                                schema.tables.iter().filter(|t| t.kind == kind).collect(),
                            )
                        })
                        .filter(|(kind, items): &(TableKind, Vec<_>)| {
                            !items.is_empty() && (*kind == TableKind::Table || prefs.sidebar_views)
                        })
                        .collect();
                let mut node = Node::new(
                    sid.clone(),
                    Some(&did),
                    2,
                    NodeKind::Schema {
                        key: key.clone(),
                        schema: schema.name.clone(),
                    },
                    &schema.name,
                );
                node.meta = Meta::Text(schema.tables.len().to_string());
                node.expandable = !groups.is_empty();
                out.push(node);
                if !expanded(&sid) {
                    continue;
                }
                for (kind, items) in groups {
                    let gid = format!("{sid}/#{}", kind.as_str());
                    let mut node = Node::new(
                        gid.clone(),
                        Some(&sid),
                        3,
                        NodeKind::Group { kind },
                        match kind {
                            TableKind::Table => "tables",
                            TableKind::View => "views",
                        },
                    );
                    node.meta = Meta::Text(items.len().to_string());
                    node.expandable = true;
                    out.push(node);
                    if !expanded(&gid) {
                        continue;
                    }
                    for t in items {
                        let tid = format!("{gid}/{}", t.name);
                        let is_cur = is_active
                            && input
                                .active_table
                                .is_some_and(|a| a.schema == schema.name && a.table == t.name);
                        let mut node = Node::new(
                            tid.clone(),
                            Some(&gid),
                            4,
                            NodeKind::Table {
                                key: key.clone(),
                                schema: schema.name.clone(),
                                table: t.name.clone(),
                                kind: t.kind,
                            },
                            &t.name,
                        );
                        node.mono = true;
                        node.italic = is_cur && input.active_table.is_some_and(|a| a.preview);
                        node.meta = if prefs.sidebar_row_counts {
                            Meta::Text(row_count(t.rows))
                        } else {
                            Meta::None
                        };
                        node.expandable = true;
                        node.current = is_cur;
                        out.push(node);
                        if !expanded(&tid) {
                            continue;
                        }
                        let keys = column_keys(t);
                        for c in &t.columns {
                            let mut node = Node::new(
                                format!("{tid}/{}", c.name),
                                Some(&tid),
                                5,
                                NodeKind::Column {
                                    name: c.name.clone(),
                                    key: keys.get(&c.name).copied(),
                                },
                                &c.name,
                            );
                            node.mono = true;
                            node.meta = Meta::Column {
                                ty: c.ty.clone(),
                                not_null: !c.nullable,
                            };
                            out.push(node);
                        }
                        // DataGrip's folders after the columns.
                        for f in meta_folders(&schema.name, t) {
                            let fid = format!("{tid}/#{}", f.kind.label());
                            let mut node = Node::new(
                                fid.clone(),
                                Some(&tid),
                                5,
                                NodeKind::Folder(f.kind),
                                f.kind.label(),
                            );
                            node.meta = Meta::Text(f.items.len().to_string());
                            node.expandable = true;
                            out.push(node);
                            if !expanded(&fid) {
                                continue;
                            }
                            for it in f.items {
                                let mut node = Node::new(
                                    format!("{fid}/{}", it.name),
                                    Some(&fid),
                                    6,
                                    NodeKind::MetaItem {
                                        folder: f.kind,
                                        name: it.name.clone(),
                                        definition: it.definition.clone(),
                                        key: it.key,
                                    },
                                    &it.name,
                                );
                                node.mono = true;
                                node.meta = Meta::Detail(it.detail);
                                out.push(node);
                            }
                        }
                    }
                }
            }
        }
    }
    out
}

/// A navigation key of the tree (Sidebar.tsx `Tree` `onKeyDown`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NavKey {
    Up,
    Down,
    Left,
    Right,
    Enter,
}

/// What a key does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Nav {
    /// Nothing (no focused row, or nowhere to go).
    None,
    /// Move the focus to this row.
    Focus(String),
    /// Expand / collapse this row.
    Expand(String),
    Collapse(String),
    /// Open it (tables: pinned tab).
    Open(String),
    /// Select it (databases, schemas).
    Select(String),
    /// Toggle its expansion.
    Toggle(String),
}

/// ↑ ↓ move, → expands or enters, ← collapses or goes to the parent, ↵ opens
/// (else selects, else toggles). With no focused row ↑ ↓ → focus the first.
pub fn nav(
    nodes: &[Node],
    focus: Option<&str>,
    key: NavKey,
    expanded: impl Fn(&str) -> bool,
) -> Nav {
    let i = focus.and_then(|f| nodes.iter().position(|n| n.id == f));
    let go = |ix: Option<usize>| match ix.and_then(|ix| nodes.get(ix)) {
        Some(n) => Nav::Focus(n.id.clone()),
        None => Nav::None,
    };
    let last = nodes.len().saturating_sub(1);
    match key {
        NavKey::Down => go(Some(i.map_or(0, |i| (i + 1).min(last)))),
        NavKey::Up => go(Some(i.map_or(0, |i| i.saturating_sub(1)))),
        NavKey::Right => {
            let Some(i) = i else { return go(Some(0)) };
            let n = &nodes[i];
            if n.expandable && !expanded(&n.id) {
                Nav::Expand(n.id.clone())
            } else if nodes
                .get(i + 1)
                .is_some_and(|next| next.parent.as_deref() == Some(n.id.as_str()))
            {
                go(Some(i + 1))
            } else {
                Nav::None
            }
        }
        NavKey::Left => {
            let Some(i) = i else { return Nav::None };
            let n = &nodes[i];
            if n.expandable && expanded(&n.id) {
                Nav::Collapse(n.id.clone())
            } else {
                match &n.parent {
                    Some(p) if nodes.iter().any(|x| &x.id == p) => Nav::Focus(p.clone()),
                    _ => Nav::None,
                }
            }
        }
        NavKey::Enter => {
            let Some(i) = i else { return Nav::None };
            let n = &nodes[i];
            if n.opens() {
                Nav::Open(n.id.clone())
            } else if n.selects() {
                Nav::Select(n.id.clone())
            } else {
                Nav::Toggle(n.id.clone())
            }
        }
    }
}

#[cfg(test)]
#[path = "tree_tests.rs"]
mod tests;
