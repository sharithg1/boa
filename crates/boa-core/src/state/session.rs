//! Port of \`ui/src/state/session.ts\` (+ \`session.test.ts\`, and the pure
//! scope rules of \`state/store.ts\` covered by \`state/scope.test.ts\`).
//!
//! The last session, restored at startup: the selected database, each
//! database's tabs (consoles with their SQL, open tables with their filter
//! and sort) and its active tab, recent tables, which panels were open and
//! which connections were open (reconnected, retrying while unreachable).
//! Results are not kept: statements are not re-run on launch.
//!
//! The Tauri app kept it in WebView localStorage under \`boa.session.v1\`; here
//! it is \`<config dir>/session.json\` with the very same JSON (field names,
//! key order, \`dbKey\` / \`tableKey\` keys joined with U+0000), so a value
//! imported from localStorage (\`state::local_storage\`) reads as is. The GPUI
//! \`Session\` model saves it (debounced, [\`SAVE_DELAY_MS\`]) whenever it changes.

use indexmap::{IndexMap, IndexSet};
use serde::de::{self, Deserialize, Deserializer, MapAccess, SeqAccess, Visitor};
use serde::ser::{Serialize, SerializeMap, Serializer};
use std::collections::HashSet;
use std::fmt;
use std::path::Path;

/// The session file in the config dir.
pub const FILE: &str = "session.json";

/// The localStorage key the Tauri app used (imported once).
pub const LOCAL_STORAGE_KEY: &str = "boa.session.v1";

/// How long the app waits after a change before writing the session.
pub const SAVE_DELAY_MS: u64 = 250;

// ── Keys (state/store.ts) ─────────────────────────────────────────────────

/// A database's key (\`dbKey\`): \`connectionId\` U+0000 \`database\`.
pub fn db_key(connection_id: &str, database: &str) -> String {
    format!("{connection_id}\u{0}{database}")
}

/// A table's key (\`tableKey\`): connection, database, schema, table joined with U+0000.
pub fn table_key(connection_id: &str, database: &str, schema: &str, table: &str) -> String {
    format!("{connection_id}\u{0}{database}\u{0}{schema}\u{0}{table}")
}

/// The parts of a [\`table_key\`] (\`parseTableKey\`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableRef {
    pub connection_id: String,
    pub database: String,
    pub schema: String,
    pub table: String,
}

/// Splits a [\`table_key\`]; missing parts are empty.
pub fn parse_table_key(key: &str) -> TableRef {
    let mut parts = key.split('\u{0}').map(str::to_owned);
    TableRef {
        connection_id: parts.next().unwrap_or_default(),
        database: parts.next().unwrap_or_default(),
        schema: parts.next().unwrap_or_default(),
        table: parts.next().unwrap_or_default(),
    }
}

/// Connection id of a \`dbKey\` / \`tableKey\`.
pub fn connection_of(key: &str) -> &str {
    key.split('\u{0}').next().unwrap_or(key)
}

// ── Types (state/store.ts) ────────────────────────────────────────────────

/// The selected database.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Active {
    pub connection_id: String,
    pub database: String,
}

/// A console tab.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueryTab {
    pub id: String,
    pub title: String,
    pub sql: String,
}

/// An open table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableTab {
    pub id: String,
    pub schema: String,
    pub table: String,
    /// Italic preview tab, replaced by the next single-click open (double-click pins).
    pub preview: bool,
    /// Filter bar clauses (\`grid::table_query\`), applied server-side; \`None\` when empty.
    pub r#where: Option<String>,
    pub order_by: Option<String>,
}

/// A tab of a database's workspace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Tab {
    Query(QueryTab),
    Table(TableTab),
}

impl Tab {
    pub fn id(&self) -> &str {
        match self {
            Tab::Query(t) => &t.id,
            Tab::Table(t) => &t.id,
        }
    }
}

impl Serialize for Tab {
    /// \`{id, kind: "query", title, sql}\` / \`{id, kind: "table", schema, table,
    /// preview, where?, orderBy?}\`, in the TypeScript key order.
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            Tab::Query(t) => {
                let mut m = s.serialize_map(Some(4))?;
                m.serialize_entry("id", &t.id)?;
                m.serialize_entry("kind", "query")?;
                m.serialize_entry("title", &t.title)?;
                m.serialize_entry("sql", &t.sql)?;
                m.end()
            }
            Tab::Table(t) => {
                let mut m = s.serialize_map(None)?;
                m.serialize_entry("id", &t.id)?;
                m.serialize_entry("kind", "table")?;
                m.serialize_entry("schema", &t.schema)?;
                m.serialize_entry("table", &t.table)?;
                m.serialize_entry("preview", &t.preview)?;
                if let Some(w) = &t.r#where {
                    m.serialize_entry("where", w)?;
                }
                if let Some(o) = &t.order_by {
                    m.serialize_entry("orderBy", o)?;
                }
                m.end()
            }
        }
    }
}

/// One database's tabs.
#[derive(Debug, Clone, PartialEq, Eq, Default, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Workspace {
    pub tabs: Vec<Tab>,
    pub active_tab: Option<String>,
}

/// A saved session (\`Session\` of session.ts).
#[derive(Debug, Clone, PartialEq, Eq, Default, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Session {
    pub active: Option<Active>,
    /// By [\`db_key\`], in saved order.
    pub workspaces: IndexMap<String, Workspace>,
    /// Recently opened tables, newest first, as [\`table_key\`]s.
    pub mru: Vec<String>,
    /// When each table was last opened (ms since the epoch), by [\`table_key\`].
    pub seen: IndexMap<String, i64>,
    /// Connections open at quit (\`None\` in sessions saved before this was
    /// kept). Saved as \`{"<id>": true, …}\`.
    #[serde(
        skip_serializing_if = "Option::is_none",
        serialize_with = "serialize_wanted"
    )]
    pub wanted: Option<IndexSet<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sidebar_open: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub right_panel_open: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bottom_open: Option<bool>,
}

fn serialize_wanted<S: Serializer>(
    wanted: &Option<IndexSet<String>>,
    s: S,
) -> Result<S::Ok, S::Error> {
    let ids = wanted.as_ref().map_or(0, IndexSet::len);
    let mut m = s.serialize_map(Some(ids))?;
    for id in wanted.iter().flatten() {
        m.serialize_entry(id, &true)?;
    }
    m.end()
}

impl Session {
    /// The JSON written to \`session.json\` (compact, like \`JSON.stringify\`).
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_else(|_| "{}".into())
    }
}

// ── Parsing ───────────────────────────────────────────────────────────────

/// A parsed JSON value that keeps object key order (as \`JSON.parse\` does:
/// a repeated key keeps its first position and its last value).
#[derive(Debug, Clone, PartialEq)]
enum Json {
    Null,
    Bool(bool),
    Num(f64),
    Str(String),
    Arr(Vec<Json>),
    Obj(IndexMap<String, Json>),
}

impl Json {
    fn get(&self, key: &str) -> Option<&Json> {
        match self {
            Json::Obj(m) => m.get(key),
            _ => None,
        }
    }
    fn str(&self, key: &str) -> Option<&str> {
        match self.get(key) {
            Some(Json::Str(s)) => Some(s),
            _ => None,
        }
    }
    fn obj(&self) -> Option<&IndexMap<String, Json>> {
        match self {
            Json::Obj(m) => Some(m),
            _ => None,
        }
    }
}

impl<'de> Deserialize<'de> for Json {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = Json;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("JSON")
            }
            fn visit_unit<E: de::Error>(self) -> Result<Json, E> {
                Ok(Json::Null)
            }
            fn visit_none<E: de::Error>(self) -> Result<Json, E> {
                Ok(Json::Null)
            }
            fn visit_some<D: Deserializer<'de>>(self, d: D) -> Result<Json, D::Error> {
                Json::deserialize(d)
            }
            fn visit_bool<E: de::Error>(self, v: bool) -> Result<Json, E> {
                Ok(Json::Bool(v))
            }
            fn visit_i64<E: de::Error>(self, v: i64) -> Result<Json, E> {
                Ok(Json::Num(v as f64))
            }
            fn visit_u64<E: de::Error>(self, v: u64) -> Result<Json, E> {
                Ok(Json::Num(v as f64))
            }
            fn visit_f64<E: de::Error>(self, v: f64) -> Result<Json, E> {
                Ok(Json::Num(v))
            }
            fn visit_str<E: de::Error>(self, v: &str) -> Result<Json, E> {
                Ok(Json::Str(v.to_owned()))
            }
            fn visit_string<E: de::Error>(self, v: String) -> Result<Json, E> {
                Ok(Json::Str(v))
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut a: A) -> Result<Json, A::Error> {
                let mut out = Vec::new();
                while let Some(v) = a.next_element()? {
                    out.push(v);
                }
                Ok(Json::Arr(out))
            }
            fn visit_map<A: MapAccess<'de>>(self, mut a: A) -> Result<Json, A::Error> {
                let mut out = IndexMap::new();
                while let Some((k, v)) = a.next_entry::<String, Json>()? {
                    out.insert(k, v);
                }
                Ok(Json::Obj(out))
            }
        }
        d.deserialize_any(V)
    }
}

/// JavaScript's \`String.prototype.trim\` whitespace.
fn is_js_space(c: char) -> bool {
    matches!(
        c,
        '\u{9}'..='\u{d}'
            | ' '
            | '\u{a0}'
            | '\u{1680}'
            | '\u{2000}'..='\u{200a}'
            | '\u{2028}'
            | '\u{2029}'
            | '\u{202f}'
            | '\u{205f}'
            | '\u{3000}'
            | '\u{feff}'
    )
}

/// A string that is not blank (JS \`trim()\` rules).
fn clause(v: Option<&Json>) -> Option<String> {
    match v {
        Some(Json::Str(s)) if !s.trim_matches(is_js_space).is_empty() => Some(s.clone()),
        _ => None,
    }
}

fn parse_tab(v: &Json) -> Option<Tab> {
    let id = v.str("id")?.to_owned();
    match v.str("kind") {
        Some("query") => Some(Tab::Query(QueryTab {
            id,
            title: v.str("title")?.to_owned(),
            sql: v.str("sql")?.to_owned(),
        })),
        Some("table") => Some(Tab::Table(TableTab {
            id,
            schema: v.str("schema")?.to_owned(),
            table: v.str("table")?.to_owned(),
            preview: v.get("preview") == Some(&Json::Bool(true)),
            r#where: clause(v.get("where")),
            order_by: clause(v.get("orderBy")),
        })),
        _ => None,
    }
}

/// Reads a saved session, keeping only what still makes sense: well-formed
/// tabs with unique ids, and nothing of connections that no longer exist.
/// \`None\` when \`raw\` is missing, not JSON, or not an object (\`parseSession\`).
pub fn parse_session<'a>(
    raw: Option<&str>,
    connection_ids: impl IntoIterator<Item = &'a str>,
) -> Option<Session> {
    let raw = raw.filter(|r| !r.is_empty())?;
    let data: Json = serde_json::from_str(raw).ok()?;
    data.obj()?;
    let known: HashSet<&str> = connection_ids.into_iter().collect();
    let mut ids: HashSet<String> = HashSet::new();

    let mut workspaces = IndexMap::new();
    for (key, ws) in data
        .get("workspaces")
        .and_then(Json::obj)
        .into_iter()
        .flatten()
    {
        if !known.contains(connection_of(key)) {
            continue;
        }
        let Some(Json::Arr(list)) = ws.get("tabs") else {
            continue;
        };
        let mut tabs = Vec::new();
        for t in list {
            let Some(tab) = parse_tab(t) else { continue };
            if !ids.insert(tab.id().to_owned()) {
                continue;
            }
            tabs.push(tab);
        }
        let Some(first) = tabs.first() else { continue };
        let active_tab = match ws.str("activeTab") {
            Some(a) if tabs.iter().any(|t| t.id() == a) => a.to_owned(),
            _ => first.id().to_owned(),
        };
        workspaces.insert(
            key.clone(),
            Workspace {
                tabs,
                active_tab: Some(active_tab),
            },
        );
    }

    let active = data.get("active").and_then(|a| {
        let connection_id = a.str("connectionId")?;
        let database = a.str("database")?;
        known.contains(connection_id).then(|| Active {
            connection_id: connection_id.to_owned(),
            database: database.to_owned(),
        })
    });

    let mru = match data.get("mru") {
        Some(Json::Arr(list)) => list
            .iter()
            .filter_map(|k| match k {
                Json::Str(k) if known.contains(connection_of(k)) => Some(k.clone()),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    };
    let seen = data
        .get("seen")
        .and_then(Json::obj)
        .into_iter()
        .flatten()
        .filter_map(|(k, v)| match v {
            Json::Num(n) if known.contains(connection_of(k)) => Some((k.clone(), *n as i64)),
            _ => None,
        })
        .collect();
    let flag = |key: &str| match data.get(key) {
        Some(Json::Bool(b)) => Some(*b),
        _ => None,
    };
    let wanted = data.get("wanted").and_then(Json::obj).map(|w| {
        w.iter()
            .filter(|(id, v)| known.contains(id.as_str()) && **v == Json::Bool(true))
            .map(|(id, _)| id.clone())
            .collect()
    });
    Some(Session {
        active,
        workspaces,
        mru,
        seen,
        wanted,
        sidebar_open: flag("sidebarOpen"),
        right_panel_open: flag("rightPanelOpen"),
        bottom_open: flag("bottomOpen"),
    })
}

/// One past the highest number used in the session's tab ids (\`q12\` → 13).
/// New tab ids start there (\`reserveIds\`), clear of restored ones.
pub fn next_tab_number(session: &Session) -> u64 {
    let mut max = 0u64;
    for ws in session.workspaces.values() {
        for t in &ws.tabs {
            let id = t.id();
            let n = &id[id.trim_end_matches(|c: char| c.is_ascii_digit()).len()..];
            if !n.is_empty() {
                max = max.max(n.parse().unwrap_or(u64::MAX - 1));
            }
        }
    }
    max.saturating_add(1)
}

/// The connections to reopen at launch: the selected one first, then the
/// others that were open (sessions from before that was kept: just the
/// selected one).
pub fn connections_to_restore(session: &Session) -> Vec<String> {
    let first = session.active.as_ref().map(|a| a.connection_id.as_str());
    let mut out: Vec<String> = first.map(str::to_owned).into_iter().collect();
    out.extend(
        session
            .wanted
            .iter()
            .flatten()
            .filter(|id| Some(id.as_str()) != first)
            .cloned(),
    );
    out
}

// ── session.json ──────────────────────────────────────────────────────────

/// Reads \`<dir>/session.json\`, pruned to \`connection_ids\`. Missing, unreadable
/// or corrupt: an empty session (never an error).
pub fn load<'a>(dir: &Path, connection_ids: impl IntoIterator<Item = &'a str>) -> Session {
    let raw = super::local_storage::read_config_file(dir, FILE);
    parse_session(raw.as_deref(), connection_ids).unwrap_or_default()
}

/// Writes \`<dir>/session.json\` (temp file + rename, owner-only).
pub fn save(dir: &Path, session: &Session) -> std::io::Result<()> {
    super::local_storage::write_config_file(dir, FILE, &session.to_json())
}

// ── Scope (store.ts: what the explorer shows and introspects) ─────────────

/// A picker choice of the \`scopes\` pref (\`string[] | '*'\`), borrowed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScopeSel<'a> {
    /// \`'*'\`: everything.
    All,
    Names(&'a [String]),
}

/// The \`scopes\` key of a database's schema pick: \`connectionId/database\`
/// (a connection's database pick is keyed by its id alone).
pub fn schema_scope_key(connection_id: &str, database: &str) -> String {
    format!("{connection_id}/{database}")
}

/// The connection's picked databases, in server order (default: its own
/// database). \`pick\` is \`scopes[connection id]\`; \`names\` the server's databases.
pub fn shown_databases(
    pick: Option<ScopeSel<'_>>,
    own_database: &str,
    names: &[String],
) -> Vec<String> {
    let want: HashSet<&str> = match pick {
        Some(ScopeSel::All) => return names.to_vec(),
        Some(ScopeSel::Names(sel)) => sel.iter().map(String::as_str).collect(),
        None => HashSet::from([own_database]),
    };
    names
        .iter()
        .filter(|n| want.contains(n.as_str()))
        .cloned()
        .collect()
}

/// The database's picked schemas, in server order (default: \`public\`, else
/// the first). \`pick\` is \`scopes[schema_scope_key(…)]\`.
pub fn database_schemas(pick: Option<ScopeSel<'_>>, names: &[String]) -> Vec<String> {
    match pick {
        Some(ScopeSel::All) => names.to_vec(),
        Some(ScopeSel::Names(sel)) => {
            let want: HashSet<&str> = sel.iter().map(String::as_str).collect();
            names
                .iter()
                .filter(|n| want.contains(n.as_str()))
                .cloned()
                .collect()
        }
        None if names.iter().any(|n| n == "public") => vec!["public".into()],
        None => names.iter().take(1).cloned().collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    fn saved() -> Value {
        json!({
            "active": { "connectionId": "c1", "database": "app" },
            "workspaces": {
                db_key("c1", "app"): {
                    "tabs": [
                        { "id": "q3", "kind": "query", "title": "console_1", "sql": "select 1" },
                        { "id": "t12", "kind": "table", "schema": "public", "table": "users", "preview": true }
                    ],
                    "activeTab": "t12"
                },
                db_key("gone", "app"): {
                    "tabs": [{ "id": "q40", "kind": "query", "title": "console_1", "sql": "" }],
                    "activeTab": "q40"
                }
            },
            "mru": [table_key("c1", "app", "public", "users"), table_key("gone", "app", "public", "x")],
            "seen": {
                table_key("c1", "app", "public", "users"): 5,
                table_key("gone", "app", "public", "x"): 6
            },
            "sidebarOpen": false,
            "bottomOpen": true
        })
    }

    fn parse(v: &Value, ids: &[&str]) -> Option<Session> {
        parse_session(Some(&v.to_string()), ids.iter().copied())
    }

    fn as_json<T: Serialize>(v: &T) -> Value {
        serde_json::to_value(v).unwrap()
    }

    fn keys<V>(m: &IndexMap<String, V>) -> Vec<String> {
        m.keys().cloned().collect()
    }

    // describe('parseSession')

    #[test]
    fn restores_tabs_their_sql_and_the_active_tab() {
        let saved = saved();
        let s = parse(&saved, &["c1"]).unwrap();
        assert_eq!(as_json(&s.active), saved["active"]);
        let key = db_key("c1", "app");
        assert_eq!(as_json(&s.workspaces[&key]), saved["workspaces"][&key]);
        assert_eq!(s.sidebar_open, Some(false));
        assert_eq!(s.right_panel_open, None);
    }

    #[test]
    fn drops_everything_of_connections_that_no_longer_exist() {
        let s = parse(&saved(), &["c1"]).unwrap();
        assert_eq!(keys(&s.workspaces), [db_key("c1", "app")]);
        assert_eq!(s.mru, [table_key("c1", "app", "public", "users")]);
        assert_eq!(keys(&s.seen), [table_key("c1", "app", "public", "users")]);
        assert_eq!(parse(&saved(), &["other"]).unwrap().active, None);
    }

    #[test]
    fn skips_malformed_and_duplicate_tabs_and_repairs_the_active_tab() {
        let raw = json!({
            "workspaces": {
                db_key("c1", "app"): {
                    "tabs": [
                        { "id": "q1", "kind": "query", "title": "console_1" },
                        { "id": "q2", "kind": "query", "title": "console_2", "sql": "a" },
                        { "id": "q2", "kind": "query", "title": "console_3", "sql": "b" },
                        "junk"
                    ],
                    "activeTab": "q1"
                },
                db_key("c1", "empty"): { "tabs": [], "activeTab": null }
            }
        });
        let s = parse(&raw, &["c1"]).unwrap();
        assert_eq!(
            as_json(&s.workspaces),
            json!({
                db_key("c1", "app"): {
                    "tabs": [{ "id": "q2", "kind": "query", "title": "console_2", "sql": "a" }],
                    "activeTab": "q2"
                }
            })
        );
    }

    #[test]
    fn keeps_a_table_tabs_filter_and_sort() {
        let tab = || json!({ "id": "t1", "kind": "table", "schema": "public", "table": "users", "preview": false });
        let with = |extra: Value| {
            let mut t = tab();
            for (k, v) in extra.as_object().unwrap() {
                t[k] = v.clone();
            }
            t
        };
        let raw = json!({
            "workspaces": {
                db_key("c1", "app"): {
                    "tabs": [
                        with(json!({ "where": "role = 'admin'", "orderBy": "id desc" })),
                        with(json!({ "id": "t2", "where": "  ", "orderBy": 3 }))
                    ],
                    "activeTab": "t1"
                }
            }
        });
        let s = parse(&raw, &["c1"]).unwrap();
        assert_eq!(
            as_json(&s.workspaces[&db_key("c1", "app")].tabs),
            json!([
                with(json!({ "where": "role = 'admin'", "orderBy": "id desc" })),
                with(json!({ "id": "t2" }))
            ])
        );
    }

    #[test]
    fn ignores_missing_or_corrupt_data() {
        assert_eq!(parse_session(None, ["c1"]), None);
        assert_eq!(parse_session(Some("{nope"), ["c1"]), None);
        assert_eq!(parse_session(Some("[]"), ["c1"]), None);
    }

    // describe('nextTabNumber')

    #[test]
    fn next_tab_number_is_past_every_restored_tab_id() {
        assert_eq!(next_tab_number(&parse(&saved(), &["c1"]).unwrap()), 13);
        assert_eq!(next_tab_number(&parse_session(Some("{}"), []).unwrap()), 1);
    }

    // describe('connections to restore')

    #[test]
    fn reopens_the_selected_connection_first_then_the_others_that_were_open() {
        // Built as text: serde_json's json! would sort the keys.
        let base = saved().to_string();
        let raw = format!(
            r#"{},"wanted":{{"c2":true,"c1":true,"gone":true,"c3":"yes"}}}}"#,
            &base[..base.len() - 1]
        );
        let s = parse_session(Some(&raw), ["c1", "c2", "c3"]).unwrap();
        assert_eq!(
            s.wanted
                .as_ref()
                .map(|w| w.iter().cloned().collect::<Vec<_>>()),
            Some(vec!["c2".to_owned(), "c1".to_owned()])
        );
        assert_eq!(as_json(&s)["wanted"], json!({ "c2": true, "c1": true }));
        assert_eq!(connections_to_restore(&s), ["c1", "c2"]);
    }

    #[test]
    fn reopens_just_the_selected_one_for_sessions_saved_before_open_connections_were_kept() {
        let s = parse(&saved(), &["c1"]).unwrap();
        assert_eq!(s.wanted, None);
        assert_eq!(connections_to_restore(&s), ["c1"]);
    }

    #[test]
    fn reopens_the_open_ones_when_nothing_was_selected() {
        let mut raw = saved();
        raw["active"] = Value::Null;
        raw["wanted"] = json!({ "c1": true });
        assert_eq!(
            connections_to_restore(&parse(&raw, &["c1"]).unwrap()),
            ["c1"]
        );
    }

    // scope.test.ts: describe('shownDatabases')

    fn names(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn shown_databases_defaults_to_the_connections_own_database() {
        let dbs = names(&["postgres", "app", "analytics"]);
        assert_eq!(shown_databases(None, "app", &dbs), ["app"]);
    }

    #[test]
    fn shown_databases_follows_the_pick_in_server_order_star_is_everything() {
        let dbs = names(&["postgres", "app", "analytics"]);
        let pick = names(&["analytics", "postgres", "gone"]);
        assert_eq!(
            shown_databases(Some(ScopeSel::Names(&pick)), "app", &dbs),
            ["postgres", "analytics"]
        );
        assert_eq!(shown_databases(Some(ScopeSel::All), "app", &dbs), dbs);
        assert!(shown_databases(Some(ScopeSel::Names(&[])), "app", &dbs).is_empty());
    }

    // scope.test.ts: describe('databaseSchemas')

    #[test]
    fn database_schemas_defaults_to_public_else_the_first_schema() {
        let all = names(&["audit", "public", "CleanClaims"]);
        assert_eq!(database_schemas(None, &all), ["public"]);
        assert_eq!(database_schemas(None, &names(&["x", "y"])), ["x"]);
    }

    #[test]
    fn database_schemas_follows_the_pick_per_database() {
        let all = names(&["audit", "public", "CleanClaims"]);
        let app_pick = names(&["CleanClaims", "audit"]);
        let scopes: IndexMap<String, ScopeSel<'_>> = IndexMap::from([
            ("c1/app".to_owned(), ScopeSel::Names(&app_pick)),
            ("c1/other".to_owned(), ScopeSel::All),
        ]);
        let pick = |db: &str| scopes.get(&schema_scope_key("c1", db)).copied();
        assert_eq!(
            database_schemas(pick("app"), &all),
            ["audit", "CleanClaims"]
        );
        assert_eq!(database_schemas(pick("other"), &all), all);
    }

    // Beyond the TypeScript cases.

    #[test]
    fn round_trips_through_its_own_json_with_the_typescript_key_order() {
        let mut raw = saved();
        raw["wanted"] = json!({ "c1": true });
        raw["rightPanelOpen"] = json!(true);
        let s = parse(&raw, &["c1"]).unwrap();
        let text = s.to_json();
        assert!(text.starts_with(r#"{"active":{"connectionId":"c1","database":"app"},"workspaces":{"c1\u0000app":{"tabs":[{"id":"q3","kind":"query","title":"console_1","sql":"select 1"},{"id":"t12","kind":"table","schema":"public","table":"users","preview":true}],"activeTab":"t12"}},"mru":"#), "{text}");
        assert!(text.ends_with(r#""wanted":{"c1":true},"sidebarOpen":false,"rightPanelOpen":true,"bottomOpen":true}"#), "{text}");
        assert_eq!(parse_session(Some(&text), ["c1"]), Some(s));
    }

    #[test]
    fn keeps_saved_key_order_and_odd_values() {
        let raw = format!(
            r#"{{"workspaces":{{"{b}":{{"tabs":[{{"id":"q9","kind":"query","title":"x","sql":""}}]}},"{a}":{{"tabs":[{{"id":"tab","kind":"query","title":"y","sql":""}}],"activeTab":"nope"}}}},"seen":{{"{b}\u0000s\u0000t":1.5}},"sidebarOpen":"yes"}}"#,
            a = r"c1\u0000a",
            b = r"c1\u0000b"
        );
        let s = parse_session(Some(&raw), ["c1"]).unwrap();
        assert_eq!(keys(&s.workspaces), [db_key("c1", "b"), db_key("c1", "a")]);
        assert_eq!(
            s.workspaces[&db_key("c1", "b")].active_tab.as_deref(),
            Some("q9")
        );
        assert_eq!(
            s.workspaces[&db_key("c1", "a")].active_tab.as_deref(),
            Some("tab")
        );
        assert_eq!(s.seen.values().copied().collect::<Vec<_>>(), [1]);
        assert_eq!(s.sidebar_open, None);
        // "tab" has no number: only q9 counts.
        assert_eq!(next_tab_number(&s), 10);
    }

    #[test]
    fn keys_split_and_join() {
        let k = table_key("c", "db", "public", "t");
        assert_eq!(connection_of(&k), "c");
        assert_eq!(connection_of(&db_key("c", "db")), "c");
        assert_eq!(
            parse_table_key(&k),
            TableRef {
                connection_id: "c".into(),
                database: "db".into(),
                schema: "public".into(),
                table: "t".into()
            }
        );
    }

    #[test]
    fn load_and_save_session_json() {
        let dir = super::super::local_storage::tests::temp_dir("session");
        // Missing: empty.
        assert_eq!(load(&dir, ["c1"]), Session::default());
        // Corrupt: empty.
        std::fs::write(dir.join(FILE), "{nope").unwrap();
        assert_eq!(load(&dir, ["c1"]), Session::default());
        let s = parse(&saved(), &["c1"]).unwrap();
        save(&dir, &s).unwrap();
        assert_eq!(load(&dir, ["c1"]), s);
        assert_eq!(load(&dir, ["other"]).workspaces.len(), 0);
        let leftovers: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(leftovers, [FILE]);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
