use super::*;
use boa_core::model::{Column, Constraint, ConstraintKind, ForeignRef, Index, Schema, Table};
use std::sync::Arc;

use crate::models::catalogs::Catalog;

fn conn(id: &str, state: ConnState) -> ConnInfo {
    ConnInfo {
        id: id.into(),
        name: format!("{id} name"),
        kind: DbKind::Postgres,
        database: "app".into(),
        ssl_mode: SslMode::Prefer,
        user: "boa".into(),
        host: "localhost".into(),
        port: 5432,
        state,
        environment: None,
        color: None,
        read_only: false,
    }
}

fn connected(databases: &[&str]) -> ConnState {
    ConnState::Connected {
        server_version: "PostgreSQL 17".into(),
        databases: databases.iter().map(|d| (*d).to_owned()).collect(),
    }
}

fn col(name: &str, ty: &str, nullable: bool, pk: bool) -> Column {
    Column {
        name: name.into(),
        ty: ty.into(),
        nullable,
        pk,
        default: None,
    }
}

fn orders() -> Table {
    Table {
        name: "orders".into(),
        kind: TableKind::Table,
        rows: 182_000,
        size_bytes: 8192,
        columns: vec![
            col("id", "integer", false, true),
            col("customer_id", "integer", false, false),
            col("note", "text", true, false),
        ],
        indexes: vec![Index {
            name: "orders_pkey".into(),
            columns: vec!["id".into()],
            unique: true,
            primary: true,
            definition: "CREATE UNIQUE INDEX orders_pkey ON public.orders USING btree (id)".into(),
        }],
        constraints: vec![
            Constraint {
                name: "orders_customer_id_fkey".into(),
                kind: ConstraintKind::ForeignKey,
                definition: "FOREIGN KEY (customer_id) REFERENCES customers(id)".into(),
                columns: vec!["customer_id".into()],
                references: Some(ForeignRef {
                    schema: "public".into(),
                    table: "customers".into(),
                    columns: vec!["id".into()],
                }),
            },
            Constraint {
                name: "orders_pkey".into(),
                kind: ConstraintKind::PrimaryKey,
                definition: "PRIMARY KEY (id)".into(),
                columns: vec!["id".into()],
                references: None,
            },
        ],
    }
}

fn customers() -> Table {
    Table {
        name: "customers".into(),
        rows: 1_234_567,
        columns: vec![col("id", "integer", false, true)],
        ..Table::default()
    }
}

fn view() -> Table {
    Table {
        name: "big_orders".into(),
        kind: TableKind::View,
        rows: 0,
        columns: vec![col("id", "integer", true, false)],
        ..Table::default()
    }
}

fn catalogs() -> Catalogs {
    let mut c = Catalogs::new();
    c.catalogs.insert(
        DbKey::new("c1", "app"),
        Catalog {
            schema_names: vec!["audit".into(), "public".into()],
            schemas: Arc::new(vec![Schema {
                name: "public".into(),
                tables: vec![customers(), orders(), view()],
            }]),
            names_loaded: true,
            loaded: true,
            ..Catalog::default()
        },
    );
    c
}

fn prefs_expanded(ids: &[&str]) -> Prefs {
    let mut p = Prefs::default();
    for id in ids {
        p.set_expanded(id, true);
    }
    p
}

fn ids(nodes: &[Node]) -> Vec<&str> {
    nodes.iter().map(|n| n.id.as_str()).collect()
}

fn tree(conns: &[ConnInfo], prefs: &Prefs, cats: &Catalogs) -> Vec<Node> {
    build(&TreeInput {
        connections: conns,
        prefs,
        catalogs: cats,
        active: None,
        active_table: None,
    })
}

const ALL_OPEN: &[&str] = &[
    "c1",
    "c1/app",
    "c1/app/public",
    "c1/app/public/#table",
    "c1/app/public/#view",
    "c1/app/public/#table/orders",
    "c1/app/public/#table/orders/#keys",
    "c1/app/public/#table/orders/#foreign keys",
    "c1/app/public/#table/orders/#indexes",
];

#[test]
fn node_ids_are_the_sidebar_tsx_ids() {
    let conns = [conn("c1", connected(&["postgres", "app"]))];
    let prefs = prefs_expanded(ALL_OPEN);
    let nodes = tree(&conns, &prefs, &catalogs());
    assert_eq!(
        ids(&nodes),
        [
            "c1",
            "c1/app",
            "c1/app/public",
            "c1/app/public/#table",
            "c1/app/public/#table/customers",
            "c1/app/public/#table/orders",
            "c1/app/public/#table/orders/id",
            "c1/app/public/#table/orders/customer_id",
            "c1/app/public/#table/orders/note",
            "c1/app/public/#table/orders/#keys",
            "c1/app/public/#table/orders/#keys/orders_pkey",
            "c1/app/public/#table/orders/#foreign keys",
            "c1/app/public/#table/orders/#foreign keys/orders_customer_id_fkey",
            "c1/app/public/#table/orders/#indexes",
            "c1/app/public/#table/orders/#indexes/orders_pkey",
            "c1/app/public/#view",
            "c1/app/public/#view/big_orders",
        ]
    );
    let depth: Vec<usize> = nodes.iter().map(|n| n.depth).collect();
    assert_eq!(depth, [0, 1, 2, 3, 4, 4, 5, 5, 5, 5, 6, 5, 6, 5, 6, 3, 4]);
    // Parents point at the row above in the hierarchy.
    assert_eq!(
        nodes[6].parent.as_deref(),
        Some("c1/app/public/#table/orders")
    );
    assert_eq!(nodes[1].parent.as_deref(), Some("c1"));
    assert_eq!(nodes[0].parent, None);
}

#[test]
fn child_counts_keys_and_details() {
    let conns = [conn("c1", connected(&["app"]))];
    let prefs = prefs_expanded(ALL_OPEN);
    let nodes = tree(&conns, &prefs, &catalogs());
    let by_id = |id: &str| nodes.iter().find(|n| n.id == id).unwrap();
    // A schema counts its tables and views; a group its own items.
    assert_eq!(by_id("c1/app/public").meta, Meta::Text("3".into()));
    assert_eq!(by_id("c1/app/public/#table").meta, Meta::Text("2".into()));
    assert_eq!(by_id("c1/app/public/#view").label, "views");
    // PK and FK columns.
    assert!(matches!(
        by_id("c1/app/public/#table/orders/id").kind,
        NodeKind::Column {
            key: Some(KeyKind::Primary),
            ..
        }
    ));
    assert!(matches!(
        by_id("c1/app/public/#table/orders/customer_id").kind,
        NodeKind::Column {
            key: Some(KeyKind::Foreign),
            ..
        }
    ));
    assert!(matches!(
        by_id("c1/app/public/#table/orders/note").kind,
        NodeKind::Column { key: None, .. }
    ));
    assert_eq!(
        by_id("c1/app/public/#table/orders/note").meta,
        Meta::Column {
            ty: "text".into(),
            not_null: false
        }
    );
    assert_eq!(
        by_id("c1/app/public/#table/orders/id").meta,
        Meta::Column {
            ty: "integer".into(),
            not_null: true
        }
    );
    assert_eq!(
        by_id("c1/app/public/#table/orders/#foreign keys/orders_customer_id_fkey").meta,
        Meta::Detail("(customer_id) → customers(id)".into())
    );
    assert_eq!(
        by_id("c1/app/public/#table/orders/#foreign keys").meta,
        Meta::Text("1".into())
    );
    // Columns are leaves; tables, folders expand.
    assert!(!by_id("c1/app/public/#table/orders/id").expandable);
    assert!(by_id("c1/app/public/#table/orders").expandable);
    assert!(by_id("c1/app/public/#table/orders/#keys").expandable);
    assert!(by_id("c1/app/public/#table/orders").mono);
}

#[test]
fn collapsed_rows_hide_their_children() {
    let conns = [
        conn("c1", connected(&["app"])),
        conn("c2", ConnState::Disconnected),
    ];
    let nodes = tree(&conns, &Prefs::default(), &catalogs());
    assert_eq!(ids(&nodes), ["c1", "c2"]);
    let prefs = prefs_expanded(&["c1"]);
    assert_eq!(
        ids(&tree(&conns, &prefs, &catalogs())),
        ["c1", "c1/app", "c2"]
    );
    // Expanded deeper rows under a collapsed parent stay hidden.
    let prefs = prefs_expanded(&["c1/app", "c1/app/public"]);
    assert_eq!(ids(&tree(&conns, &prefs, &catalogs())), ["c1", "c2"]);
}

#[test]
fn unconnected_connections_show_their_state() {
    let conns = [
        conn("a", ConnState::Disconnected),
        conn("b", ConnState::Connecting),
        conn("c", ConnState::Reconnecting { attempt: 2 }),
        conn(
            "d",
            ConnState::Failed {
                message: "refused".into(),
            },
        ),
    ];
    let prefs = prefs_expanded(&["a", "b", "c", "d"]);
    let nodes = tree(&conns, &prefs, &Catalogs::new());
    let rows: Vec<(&str, &str)> = nodes
        .iter()
        .map(|n| (n.id.as_str(), n.label.as_str()))
        .collect();
    assert_eq!(
        rows,
        [
            ("a", "a name"),
            ("a/#status", "Not connected"),
            ("b", "b name"),
            ("b/#status", "Connecting…"),
            ("c", "c name"),
            ("c/#status", "Reconnecting…"),
            ("d", "d name"),
            ("d/#status", "Not connected"),
        ]
    );
    assert_eq!(nodes[0].meta, Meta::None);
    assert_eq!(nodes[4].meta, Meta::Reconnecting);
    assert_eq!(nodes[6].meta, Meta::Failed);
    assert!(nodes.iter().filter(|n| n.depth == 0).all(|n| n.expandable));
    assert!(!nodes[1].expandable);
}

#[test]
fn databases_follow_the_scope_pick() {
    let conns = [conn("c1", connected(&["postgres", "app", "analytics"]))];
    let mut prefs = prefs_expanded(&["c1"]);
    // Default: the connection's own database.
    let nodes = tree(&conns, &prefs, &Catalogs::new());
    assert_eq!(ids(&nodes), ["c1", "c1/app"]);
    let Meta::Scope(badge) = &nodes[0].meta else {
        panic!("no badge: {:?}", nodes[0].meta)
    };
    assert_eq!(badge.kind, ScopeKind::Databases);
    assert_eq!(badge.key, "c1");
    assert_eq!(badge.selected, ["app"]);
    assert_eq!(badge.items.len(), 3);
    assert!(!badge.is_all);
    assert_eq!(badge.default_item.as_deref(), Some("app"));
    // A pick, in server order.
    prefs.scopes.insert(
        "c1".into(),
        Scope::Some(vec!["analytics".into(), "postgres".into(), "gone".into()]),
    );
    assert_eq!(
        ids(&tree(&conns, &prefs, &Catalogs::new())),
        ["c1", "c1/postgres", "c1/analytics"]
    );
    // '*': everything.
    prefs.scopes.insert("c1".into(), Scope::All);
    let nodes = tree(&conns, &prefs, &Catalogs::new());
    assert_eq!(ids(&nodes), ["c1", "c1/postgres", "c1/app", "c1/analytics"]);
    assert!(matches!(&nodes[0].meta, Meta::Scope(b) if b.is_all));
    // Nothing picked.
    prefs.scopes.insert("c1".into(), Scope::Some(vec![]));
    let nodes = tree(&conns, &prefs, &Catalogs::new());
    assert_eq!(ids(&nodes), ["c1", "c1/#none"]);
    assert_eq!(nodes[1].label, "No databases picked: use the “of” badge");
}

#[test]
fn schemas_follow_the_scope_pick_and_show_loading() {
    let conns = [conn("c1", connected(&["app"]))];
    let mut prefs = prefs_expanded(&["c1", "c1/app"]);
    let mut cats = catalogs();
    let nodes = tree(&conns, &prefs, &cats);
    assert_eq!(ids(&nodes), ["c1", "c1/app", "c1/app/public"]);
    let Meta::Database {
        badge: Some(badge),
        loading: false,
    } = &nodes[1].meta
    else {
        panic!("{:?}", nodes[1].meta)
    };
    assert_eq!(badge.key, "c1/app");
    assert_eq!(badge.selected, ["public"]);
    assert_eq!(badge.items, ["audit", "public"]);
    assert_eq!(badge.default_item.as_deref(), Some("public"));
    // Unpicking public drops it at once, before any reload.
    prefs
        .scopes
        .insert("c1/app".into(), Scope::Some(vec!["audit".into()]));
    assert_eq!(ids(&tree(&conns, &prefs, &cats)), ["c1", "c1/app"]);
    // Before the names are read: no badge, "loading…".
    cats.catalogs.insert(
        DbKey::new("c1", "app"),
        Catalog {
            loading: true,
            ..Catalog::default()
        },
    );
    let nodes = tree(&conns, &prefs, &cats);
    assert_eq!(
        nodes[1].meta,
        Meta::Database {
            badge: None,
            loading: true
        }
    );
    // Not loaded at all: no meta.
    let nodes = tree(&conns, &prefs, &Catalogs::new());
    assert_eq!(nodes[1].meta, Meta::None);
    assert_eq!(ids(&nodes), ["c1", "c1/app"]);
}

#[test]
fn views_toggle_hides_the_views_folder() {
    let conns = [conn("c1", connected(&["app"]))];
    let mut prefs = prefs_expanded(ALL_OPEN);
    assert!(prefs.sidebar_views, "views show by default");
    prefs.sidebar_views = false;
    let nodes = tree(&conns, &prefs, &catalogs());
    assert!(!ids(&nodes).iter().any(|id| id.contains("#view")));
    // The schema still counts its views.
    assert_eq!(nodes[2].meta, Meta::Text("3".into()));
    // A schema with only views has nothing to expand without them.
    let mut cats = Catalogs::new();
    cats.catalogs.insert(
        DbKey::new("c1", "app"),
        Catalog {
            schema_names: vec!["public".into()],
            schemas: Arc::new(vec![Schema {
                name: "public".into(),
                tables: vec![view()],
            }]),
            names_loaded: true,
            ..Catalog::default()
        },
    );
    let nodes = tree(&conns, &prefs, &cats);
    assert!(!nodes[2].expandable);
    prefs.sidebar_views = true;
    assert!(tree(&conns, &prefs, &cats)[2].expandable);
}

#[test]
fn row_counts_follow_the_pref_and_format_like_the_source() {
    let conns = [conn("c1", connected(&["app"]))];
    let mut prefs = prefs_expanded(ALL_OPEN);
    prefs.sidebar_row_counts = true;
    let nodes = tree(&conns, &prefs, &catalogs());
    let meta = |id: &str| nodes.iter().find(|n| n.id == id).unwrap().meta.clone();
    assert_eq!(
        meta("c1/app/public/#table/orders"),
        Meta::Text("182k".into())
    );
    assert_eq!(
        meta("c1/app/public/#table/customers"),
        Meta::Text("1.2M".into())
    );
    assert_eq!(
        meta("c1/app/public/#view/big_orders"),
        Meta::Text("0".into())
    );
    prefs.sidebar_row_counts = false;
    let nodes = tree(&conns, &prefs, &catalogs());
    assert_eq!(
        nodes
            .iter()
            .find(|n| n.id == "c1/app/public/#table/orders")
            .unwrap()
            .meta,
        Meta::None
    );
    assert_eq!(row_count(999), "999");
    assert_eq!(row_count(1_250), "1.3k");
    assert_eq!(row_count(98_120_004), "98M");
    assert_eq!(row_count(-1), "-1");
}

#[test]
fn the_open_table_and_active_database_are_current() {
    let conns = [conn("c1", connected(&["app"]))];
    let prefs = prefs_expanded(ALL_OPEN);
    let cats = catalogs();
    let key = DbKey::new("c1", "app");
    let input = |table: Option<ActiveTable<'static>>| TreeInput {
        connections: &conns,
        prefs: &prefs,
        catalogs: &cats,
        active: Some(&key),
        active_table: table,
    };
    let nodes = build(&input(None));
    assert!(nodes[1].current, "the active database without a table tab");
    assert!(matches!(
        nodes[1].kind,
        NodeKind::Database { active: true, .. }
    ));
    let nodes = build(&input(Some(ActiveTable {
        schema: "public",
        table: "orders",
        preview: true,
    })));
    assert!(!nodes[1].current);
    let orders = nodes
        .iter()
        .find(|n| n.id == "c1/app/public/#table/orders")
        .unwrap();
    assert!(orders.current && orders.italic, "a preview tab is italic");
    let nodes = build(&input(Some(ActiveTable {
        schema: "public",
        table: "orders",
        preview: false,
    })));
    let orders = nodes
        .iter()
        .find(|n| n.id == "c1/app/public/#table/orders")
        .unwrap();
    assert!(orders.current && !orders.italic);
}

#[test]
fn keyboard_navigation_moves_expands_and_opens_as_the_source() {
    let conns = [conn("c1", connected(&["app"]))];
    let prefs = prefs_expanded(&["c1", "c1/app", "c1/app/public", "c1/app/public/#table"]);
    let nodes = tree(&conns, &prefs, &catalogs());
    let exp = |id: &str| prefs.is_expanded(id);
    let f = |s: &str| Nav::Focus(s.into());
    // No focus: ↓ ↑ → go to the first row; ← ↵ do nothing.
    assert_eq!(nav(&nodes, None, NavKey::Down, exp), f("c1"));
    assert_eq!(nav(&nodes, None, NavKey::Up, exp), f("c1"));
    assert_eq!(nav(&nodes, None, NavKey::Right, exp), f("c1"));
    assert_eq!(nav(&nodes, None, NavKey::Left, exp), Nav::None);
    assert_eq!(nav(&nodes, None, NavKey::Enter, exp), Nav::None);
    // ↓ ↑ stop at the ends.
    assert_eq!(nav(&nodes, Some("c1"), NavKey::Up, exp), f("c1"));
    assert_eq!(nav(&nodes, Some("c1"), NavKey::Down, exp), f("c1/app"));
    let last = nodes.last().unwrap().id.clone();
    assert_eq!(nav(&nodes, Some(&last), NavKey::Down, exp), f(&last));
    // → on an expanded row enters it; on a collapsed one expands it.
    assert_eq!(nav(&nodes, Some("c1"), NavKey::Right, exp), f("c1/app"));
    let orders = "c1/app/public/#table/orders";
    assert_eq!(
        nav(&nodes, Some(orders), NavKey::Right, exp),
        Nav::Expand(orders.into())
    );
    // ← collapses an expanded row, else goes to the parent.
    assert_eq!(
        nav(&nodes, Some("c1/app"), NavKey::Left, exp),
        Nav::Collapse("c1/app".into())
    );
    assert_eq!(
        nav(&nodes, Some(orders), NavKey::Left, exp),
        f("c1/app/public/#table")
    );
    assert_eq!(
        nav(&nodes, Some("c1"), NavKey::Left, exp),
        Nav::Collapse("c1".into())
    );
    // ↵: tables open (pinned), databases / schemas select, folders toggle.
    assert_eq!(
        nav(&nodes, Some(orders), NavKey::Enter, exp),
        Nav::Open(orders.into())
    );
    assert_eq!(
        nav(&nodes, Some("c1/app"), NavKey::Enter, exp),
        Nav::Select("c1/app".into())
    );
    assert_eq!(
        nav(&nodes, Some("c1/app/public"), NavKey::Enter, exp),
        Nav::Select("c1/app/public".into())
    );
    assert_eq!(
        nav(&nodes, Some("c1/app/public/#table"), NavKey::Enter, exp),
        Nav::Toggle("c1/app/public/#table".into())
    );
    assert_eq!(
        nav(&nodes, Some("c1"), NavKey::Enter, exp),
        Nav::Toggle("c1".into())
    );
    // A focus that left the tree counts as none.
    assert_eq!(nav(&nodes, Some("gone"), NavKey::Down, exp), f("c1"));
}

#[test]
fn connection_url_and_environment() {
    let mut c = conn("c1", ConnState::Disconnected);
    assert_eq!(
        connection_string(&c, None),
        "postgresql://boa@localhost:5432/app"
    );
    c.user = "me@corp:x y".into();
    assert_eq!(
        connection_string(&c, Some("other")),
        "postgresql://me%40corp%3Ax%20y@localhost:5432/other"
    );
    c.user.clear();
    c.port = 0;
    assert_eq!(connection_string(&c, None), "postgresql://localhost/app");
    assert_eq!(encode_uri_component("a-_.!~*'()é"), "a-_.!~*'()%C3%A9");
    assert_eq!(
        environment(Some(SavedEnvironment::Production)),
        Some(Environment::Production)
    );
    assert_eq!(environment(None), None);
}

#[test]
fn redis_databases_list_their_keys_in_folders() {
    use boa_core::redis::{RedisKey, RedisKeys};
    let mut r = conn("r1", connected(&["db0", "db1"]));
    r.kind = DbKind::Redis;
    r.database = "db0".into();
    r.port = 6379;
    r.user = String::new();
    let mut cats = Catalogs::new();
    let key = |name: &str, ty: &str| RedisKey {
        name: name.into(),
        ty: ty.into(),
    };
    cats.catalogs.insert(
        DbKey::new("r1", "db0"),
        Catalog {
            redis: Some(Arc::new(RedisKeys {
                keys: vec![
                    key("0e57", "string"),
                    key("cache::img-1", "string"),
                    key("session:1", "hash"),
                    key("session:2", "hash"),
                    key("session:eu:3", "hash"),
                ],
                total: 12_000,
                truncated: true,
            })),
            names_loaded: true,
            loaded: true,
            ..Catalog::default()
        },
    );
    // Folders start collapsed: folders first, then the loose keys.
    let prefs = prefs_expanded(&["r1", "r1/db0"]);
    let nodes = tree(std::slice::from_ref(&r), &prefs, &cats);
    assert_eq!(
        ids(&nodes),
        [
            "r1",
            "r1/db0",
            "r1/db0/#keys/cache::",
            "r1/db0/#keys/session:",
            "r1/db0/#key/0e57",
            "r1/db0/#more",
        ]
    );
    // The database row shows the key count (all of them, not just the listed).
    assert_eq!(nodes[1].meta, Meta::Text("12k".into()));
    assert_eq!(nodes[3].label, "session");
    // Only the first keys are listed: folder counts are lower bounds.
    assert_eq!(nodes[3].meta, Meta::Text("3+".into()));
    assert!(nodes[3].expandable && !nodes[3].opens());
    assert!(matches!(&nodes[5].kind, NodeKind::MoreKeys { .. }));
    assert_eq!(nodes[5].label, "Load more keys (5 of 12k shown)");
    assert!(nodes[5].opens());

    // Expanded folders nest, keys labelled without the folder's prefix.
    let prefs = prefs_expanded(&[
        "r1",
        "r1/db0",
        "r1/db0/#keys/session:",
        "r1/db0/#keys/session:eu:",
    ]);
    let nodes = tree(std::slice::from_ref(&r), &prefs, &cats);
    let rows: Vec<(&str, usize, &str)> = nodes
        .iter()
        .map(|n| (n.id.as_str(), n.depth, n.label.as_str()))
        .collect();
    assert_eq!(
        &rows[3..8],
        [
            ("r1/db0/#keys/session:", 2, "session"),
            ("r1/db0/#keys/session:eu:", 3, "eu"),
            ("r1/db0/#key/session:eu:3", 4, "3"),
            ("r1/db0/#key/session:1", 3, "1"),
            ("r1/db0/#key/session:2", 3, "2"),
        ]
    );
    assert!(matches!(
        &nodes[5].kind,
        NodeKind::RedisKey { name, ty, .. } if name == "session:eu:3" && ty == "hash"
    ));
    assert_eq!(nodes[5].meta, Meta::Text("hash".into()));
    assert!(nodes[5].opens() && nodes[5].selects() && !nodes[5].expandable);
    assert_eq!(nodes[5].parent.as_deref(), Some("r1/db0/#keys/session:eu:"));
    // ↵ on a key opens it; ← goes to its folder.
    assert_eq!(
        nav(&nodes, Some("r1/db0/#key/session:1"), NavKey::Enter, |_| {
            true
        }),
        Nav::Open("r1/db0/#key/session:1".into())
    );
    assert_eq!(
        nav(&nodes, Some("r1/db0/#key/session:1"), NavKey::Left, |_| {
            true
        }),
        Nav::Focus("r1/db0/#keys/session:".into())
    );

    // Loading, then empty.
    cats.catalogs
        .get_mut(&DbKey::new("r1", "db0"))
        .unwrap()
        .loading = true;
    let nodes = tree(std::slice::from_ref(&r), &prefs, &cats);
    assert_eq!(
        nodes[1].meta,
        Meta::Database {
            badge: None,
            loading: true
        }
    );
    let c = cats.catalogs.get_mut(&DbKey::new("r1", "db0")).unwrap();
    c.loading = false;
    c.redis = Some(Arc::new(RedisKeys::default()));
    let nodes = tree(std::slice::from_ref(&r), &prefs, &cats);
    assert_eq!(ids(&nodes), ["r1", "r1/db0", "r1/db0/#empty"]);
    assert_eq!(nodes[1].meta, Meta::Text("0".into()));

    assert_eq!(connection_string(&r, None), "redis://localhost:6379/0");
    assert_eq!(
        connection_string(&r, Some("db1")),
        "redis://localhost:6379/1"
    );
    r.user = "app".into();
    r.ssl_mode = SslMode::Require;
    assert_eq!(connection_string(&r, None), "rediss://app@localhost:6379/0");
}
