//! Port of state/prefs.ts: the client-side preferences (Settings → General
//! plus the remembered layout), with the same defaults, clamps, `applyOrder`
//! and "reset to defaults keeps the layout".
//!
//! The Tauri app kept them in WebView localStorage under `boa.prefs.v2`; the
//! GPUI app keeps the same JSON object (same field names, camelCase) in
//! `<config dir>/prefs.json` ([`PREFS_FILE`], [`load`], [`Prefs::save`]).
//!
//! Reading is as forgiving as the TS `load()`: a missing or unreadable file, or
//! one that is not a JSON object, gives the defaults; a field with the wrong
//! type keeps its default; `rightTab` outside the known tabs and a negative or
//! non-finite `historyDays` fall back to the default. Fields this version does
//! not know are kept and written back (the TS `{ ...DEFAULT_PREFS, ...parsed }`
//! kept them too), so a newer build's prefs survive a downgrade.
//!
//! One addition: `rowLimit` is read as a whole number in 1..=5,000
//! ([`ROW_LIMIT_MAX`], the largest value Settings offers), so a hand-edited
//! file can't ask the backend for huge pages.

use std::collections::{BTreeMap, HashMap};
use std::io::Write as _;
use std::path::{Path, PathBuf};

use serde::de::DeserializeOwned;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::{Map, Value};

/// File name of the prefs in the config dir.
pub const PREFS_FILE: &str = "prefs.json";

/// The localStorage key the Tauri app used (for the one-time import).
pub const LOCAL_STORAGE_KEY: &str = "boa.prefs.v2";

/// Sidebar width limits for the drag handle (px).
pub const SIDEBAR_MIN_WIDTH: u32 = 200;
pub const SIDEBAR_MAX_WIDTH: u32 = 480;
/// Right panel width limits for the drag handle (px).
pub const RIGHT_PANEL_MIN_WIDTH: u32 = 280;
pub const RIGHT_PANEL_MAX_WIDTH: u32 = 800;
/// Bottom panel height limits for the drag handle (px). The maximum is also
/// at most the window height minus [`BOTTOM_WINDOW_MARGIN`].
pub const BOTTOM_MIN_HEIGHT: u32 = 120;
pub const BOTTOM_MAX_HEIGHT: u32 = 900;
/// `window.innerHeight - 200`: room the bottom panel leaves above it.
pub const BOTTOM_WINDOW_MARGIN: f64 = 200.;

/// The largest `rowLimit` Settings offers.
pub const ROW_LIMIT_MAX: u32 = 5_000;
/// What Settings offers for `rowLimit`.
pub const ROW_LIMIT_CHOICES: [u32; 4] = [100, 500, 1_000, 5_000];
/// What Settings offers for `recentCount`.
pub const RECENT_COUNT_CHOICES: [u32; 4] = [3, 5, 8, 10];
/// What Settings offers for `historyDays` (0: forever).
pub const HISTORY_DAYS_CHOICES: [u32; 5] = [7, 30, 90, 365, 0];

/// The right panel's tabs.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RightTab {
    #[default]
    Structure,
    Info,
    History,
}

impl RightTab {
    /// All tabs, in the order the panel shows them (`RIGHT_TABS`).
    pub const ALL: [RightTab; 3] = [RightTab::Structure, RightTab::Info, RightTab::History];

    /// The JSON value (`'structure' | 'info' | 'history'`).
    pub fn as_str(self) -> &'static str {
        match self {
            RightTab::Structure => "structure",
            RightTab::Info => "info",
            RightTab::History => "history",
        }
    }

    /// The tab's label in the panel header.
    pub fn label(self) -> &'static str {
        match self {
            RightTab::Structure => "Structure",
            RightTab::Info => "Info",
            RightTab::History => "History",
        }
    }
}

/// What the explorer shows of a connection (its databases) or a database
/// (its schemas): DataGrip's "N of M". `'*'` is all, including ones created
/// later; otherwise the listed names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Scope {
    All,
    Some(Vec<String>),
}

impl Serialize for Scope {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Scope::All => serializer.serialize_str("*"),
            Scope::Some(names) => names.serialize(serializer),
        }
    }
}

impl<'de> Deserialize<'de> for Scope {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Raw {
            Str(String),
            List(Vec<String>),
        }
        match Raw::deserialize(deserializer)? {
            Raw::Str(s) if s == "*" => Ok(Scope::All),
            Raw::Str(s) => Err(serde::de::Error::custom(format!(
                "scope must be \"*\" or a list, not {s:?}"
            ))),
            Raw::List(names) => Ok(Scope::Some(names)),
        }
    }
}

/// `boa.prefs.v2`: every field of the TS `Prefs`, in its order, with its JSON name.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Prefs {
    /// Rows fetched when a table tab opens.
    pub row_limit: u32,
    /// Ask before running UPDATE / DELETE / DROP without a WHERE.
    pub confirm_destructive: bool,
    /// Save every statement run to the connection's query history (on disk).
    pub record_history: bool,
    /// Days a run stays in the query history; 0 keeps it until deleted.
    pub history_days: u32,
    /// Approximate row counts next to tables in the sidebar.
    pub sidebar_row_counts: bool,
    /// Show views alongside tables in the sidebar.
    pub sidebar_views: bool,
    /// Number of "Recent" entries in the ⌘K palette.
    pub recent_count: u32,
    /// Sidebar layout (drag to reorder): connection ids; those not listed follow.
    pub connection_order: Vec<String>,
    /// Expanded Database Explorer nodes, by tree path (Sidebar `nodeId`).
    /// `Record<string, true>`: only `true` values are kept.
    pub tree_expanded: BTreeMap<String, bool>,
    /// By connection id the databases, by `connectionId/database` the
    /// schemas the explorer shows; unset is the default (the connection's
    /// database; the `public` schema).
    pub scopes: BTreeMap<String, Scope>,
    /// Bottom panel (Database Output) height in px, as last dragged.
    pub bottom_height: u32,
    /// Sidebar width in px, as last dragged.
    pub sidebar_width: u32,
    /// Right panel width in px, as last dragged.
    pub right_panel_width: u32,
    /// Right panel tab last selected.
    pub right_tab: RightTab,
    /// Fields this version does not know, written back unchanged.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// The JSON names of the fields "Reset to defaults" keeps (layout, not settings).
pub const KEPT_ON_RESET: [&str; 7] = [
    "connectionOrder",
    "treeExpanded",
    "scopes",
    "bottomHeight",
    "sidebarWidth",
    "rightPanelWidth",
    "rightTab",
];

/// The JSON names of every known field, in order.
pub const FIELDS: [&str; 14] = [
    "rowLimit",
    "confirmDestructive",
    "recordHistory",
    "historyDays",
    "sidebarRowCounts",
    "sidebarViews",
    "recentCount",
    "connectionOrder",
    "treeExpanded",
    "scopes",
    "bottomHeight",
    "sidebarWidth",
    "rightPanelWidth",
    "rightTab",
];

impl Default for Prefs {
    /// `DEFAULT_PREFS`.
    fn default() -> Self {
        Self {
            row_limit: 500,
            confirm_destructive: true,
            record_history: true,
            history_days: 30,
            sidebar_row_counts: true,
            sidebar_views: true,
            recent_count: 5,
            connection_order: Vec::new(),
            tree_expanded: BTreeMap::new(),
            scopes: BTreeMap::new(),
            bottom_height: 280,
            sidebar_width: 260,
            right_panel_width: 340,
            right_tab: RightTab::Structure,
            extra: Map::new(),
        }
    }
}

impl<'de> Deserialize<'de> for Prefs {
    /// Lenient, like the TS `load()`: see [`Prefs::from_value`].
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Ok(Self::from_value(Value::deserialize(deserializer)?))
    }
}

/// A JSON number as a non-negative whole number (rounded, like the TS
/// clamps). None for anything else, negative numbers included.
fn whole(value: &Value) -> Option<u32> {
    let n = value.as_f64()?;
    (n.is_finite() && n >= 0.).then(|| n.round().min(f64::from(u32::MAX)) as u32)
}

/// Overwrites `slot` when `value` has the right shape.
fn set<T: DeserializeOwned>(slot: &mut T, value: &Value) {
    if let Ok(v) = serde_json::from_value(value.clone()) {
        *slot = v;
    }
}

/// Overwrites `slot` when `value` is a non-negative number.
fn set_whole(slot: &mut u32, value: &Value) {
    if let Some(n) = whole(value) {
        *slot = n;
    }
}

impl Prefs {
    /// Reads a parsed `boa.prefs.v2` object over the defaults. Not an object:
    /// the defaults. A field of the wrong type keeps its default; unknown
    /// fields go to [`Prefs::extra`].
    pub fn from_value(value: Value) -> Self {
        let mut prefs = Self::default();
        let Value::Object(map) = value else {
            return prefs;
        };
        for (key, v) in map {
            match key.as_str() {
                "rowLimit" => {
                    if let Some(n) = whole(&v).filter(|n| *n > 0) {
                        prefs.row_limit = n.min(ROW_LIMIT_MAX);
                    }
                }
                "confirmDestructive" => set(&mut prefs.confirm_destructive, &v),
                "recordHistory" => set(&mut prefs.record_history, &v),
                // Not finite or < 0: the default (TS load()).
                "historyDays" => set_whole(&mut prefs.history_days, &v),
                "sidebarRowCounts" => set(&mut prefs.sidebar_row_counts, &v),
                "sidebarViews" => set(&mut prefs.sidebar_views, &v),
                "recentCount" => set_whole(&mut prefs.recent_count, &v),
                "connectionOrder" => set(&mut prefs.connection_order, &v),
                "treeExpanded" => {
                    if let Value::Object(nodes) = &v {
                        prefs.tree_expanded = nodes
                            .iter()
                            .filter(|(_, open)| open.as_bool() == Some(true))
                            .map(|(id, _)| (id.clone(), true))
                            .collect();
                    }
                }
                "scopes" => {
                    if let Value::Object(scopes) = &v {
                        prefs.scopes = scopes
                            .iter()
                            .filter_map(|(k, s)| {
                                Some((k.clone(), serde_json::from_value(s.clone()).ok()?))
                            })
                            .collect();
                    }
                }
                "bottomHeight" => set_whole(&mut prefs.bottom_height, &v),
                "sidebarWidth" => set_whole(&mut prefs.sidebar_width, &v),
                "rightPanelWidth" => set_whole(&mut prefs.right_panel_width, &v),
                // Outside RIGHT_TABS: the default (TS load()).
                "rightTab" => set(&mut prefs.right_tab, &v),
                _ => {
                    prefs.extra.insert(key, v);
                }
            }
        }
        prefs
    }

    /// Parses `boa.prefs.v2` JSON text (prefs.json, or the localStorage value).
    /// Anything unparsable gives the defaults.
    pub fn from_json(raw: &str) -> Self {
        serde_json::from_str::<Value>(raw).map_or_else(|_| Self::default(), Self::from_value)
    }

    /// The JSON text written to prefs.json (what `JSON.stringify(prefs)` wrote).
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_else(|_| "{}".to_owned())
    }

    /// `resetPrefs()`: the defaults, keeping the layout fields of
    /// [`KEPT_ON_RESET`] (and fields this version does not know).
    pub fn reset(&self) -> Self {
        Self {
            connection_order: self.connection_order.clone(),
            tree_expanded: self.tree_expanded.clone(),
            scopes: self.scopes.clone(),
            bottom_height: self.bottom_height,
            sidebar_width: self.sidebar_width,
            right_panel_width: self.right_panel_width,
            right_tab: self.right_tab,
            extra: self.extra.clone(),
            ..Self::default()
        }
    }

    /// `rowLimit`, at least 1 and at most [`ROW_LIMIT_MAX`]: the page size to use.
    pub fn page_size(&self) -> u32 {
        self.row_limit.clamp(1, ROW_LIMIT_MAX)
    }

    /// `historyDays` as the history calls take it (None: keep everything).
    pub fn history_retention(&self) -> Option<u32> {
        (self.history_days > 0).then_some(self.history_days)
    }

    /// The saved sidebar width, clamped (what the sidebar shows).
    pub fn sidebar_width_px(&self) -> u32 {
        clamp_sidebar_width(f64::from(self.sidebar_width))
    }

    /// The saved right panel width, clamped.
    pub fn right_panel_width_px(&self) -> u32 {
        clamp_right_panel_width(f64::from(self.right_panel_width))
    }

    /// The saved bottom panel height, clamped for a window this tall.
    pub fn bottom_height_px(&self, window_height: Option<f64>) -> u32 {
        clamp_bottom_height(f64::from(self.bottom_height), window_height)
    }

    /// Whether explorer node `id` is expanded.
    pub fn is_expanded(&self, id: &str) -> bool {
        self.tree_expanded.get(id).copied().unwrap_or(false)
    }

    /// Expands or collapses explorer node `id` (collapsing removes the key,
    /// as Sidebar's `toggleNode` does).
    pub fn set_expanded(&mut self, id: &str, open: bool) {
        if open {
            self.tree_expanded.insert(id.to_owned(), true);
        } else {
            self.tree_expanded.remove(id);
        }
    }

    /// Writes `<dir>/prefs.json` (temp file + rename, so a crash never leaves
    /// half a file), owner-only like every other file in the config dir.
    /// Creates `dir` if needed.
    pub fn save(&self, dir: &Path) -> std::io::Result<()> {
        std::fs::create_dir_all(dir)?;
        let tmp = dir.join(format!(".{PREFS_FILE}.{}.tmp", std::process::id()));
        let result = (|| {
            let mut file = std::fs::File::create(&tmp)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt as _;
                file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
            }
            file.write_all(self.to_json().as_bytes())?;
            file.sync_all()?;
            std::fs::rename(&tmp, prefs_path(dir))
        })();
        if result.is_err() {
            let _ = std::fs::remove_file(&tmp);
        }
        result
    }
}

/// `<dir>/prefs.json`.
pub fn prefs_path(dir: &Path) -> PathBuf {
    dir.join(PREFS_FILE)
}

/// Whether `<dir>/prefs.json` exists (the localStorage import runs only when it doesn't).
pub fn exists(dir: &Path) -> bool {
    prefs_path(dir).exists()
}

/// Reads `<dir>/prefs.json`. Never fails: a missing, unreadable or corrupt
/// file gives the defaults (an unreadable or corrupt one is logged).
pub fn load(dir: &Path) -> Prefs {
    let path = prefs_path(dir);
    match std::fs::read_to_string(&path) {
        Ok(raw) => match serde_json::from_str::<Value>(&raw) {
            Ok(value) => Prefs::from_value(value),
            Err(err) => {
                tracing::warn!(path = %path.display(), error = %err, "prefs.json is not valid JSON; using the defaults");
                Prefs::default()
            }
        },
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Prefs::default(),
        Err(err) => {
            tracing::warn!(path = %path.display(), error = %err, "reading prefs.json; using the defaults");
            Prefs::default()
        }
    }
}

/// `Math.round(Math.min(max, Math.max(min, px)))`; not finite: `default`.
fn clamp_px(px: f64, min: u32, max: u32, default: u32) -> u32 {
    if !px.is_finite() {
        return default;
    }
    px.max(f64::from(min)).min(f64::from(max)).round() as u32
}

/// `clampSidebarWidth`: 200–480 px, rounded; not finite: the default.
pub fn clamp_sidebar_width(px: f64) -> u32 {
    clamp_px(
        px,
        SIDEBAR_MIN_WIDTH,
        SIDEBAR_MAX_WIDTH,
        Prefs::default().sidebar_width,
    )
}

/// `clampRightPanelWidth`: 280–800 px, rounded; not finite: the default.
pub fn clamp_right_panel_width(px: f64) -> u32 {
    clamp_px(
        px,
        RIGHT_PANEL_MIN_WIDTH,
        RIGHT_PANEL_MAX_WIDTH,
        Prefs::default().right_panel_width,
    )
}

/// `clampBottomHeight`: at least 120 px and at most min(900, window height −
/// 200), rounded; not finite: the default. Without a window (`None`) the
/// maximum is 900. As in the TS (`Math.min(max, Math.max(min, px))`) the
/// maximum wins in a window too short for both; it never goes below 0.
pub fn clamp_bottom_height(px: f64, window_height: Option<f64>) -> u32 {
    if !px.is_finite() {
        return Prefs::default().bottom_height;
    }
    let max = window_height.map_or(f64::from(BOTTOM_MAX_HEIGHT), |h| h - BOTTOM_WINDOW_MARGIN);
    let max = f64::from(BOTTOM_MAX_HEIGHT).min(max);
    let v = max.min(f64::from(BOTTOM_MIN_HEIGHT).max(px)).round();
    if v > 0. { v as u32 } else { 0 }
}

/// `applyOrder`: `ids` in the saved `order` first (in that order), then the
/// rest in their original order. Ids of `order` missing from `ids` are
/// ignored; an id listed twice in `order` takes its last place (a JS `Map`).
pub fn apply_order<S: AsRef<str>>(ids: &[S], order: &[String]) -> Vec<String> {
    if order.is_empty() {
        return ids.iter().map(|s| s.as_ref().to_owned()).collect();
    }
    let rank: HashMap<&str, usize> = order
        .iter()
        .enumerate()
        .map(|(i, id)| (id.as_str(), i))
        .collect();
    let mut known: Vec<&str> = ids
        .iter()
        .map(AsRef::as_ref)
        .filter(|id| rank.contains_key(id))
        .collect();
    // Array.prototype.sort is stable; so is sort_by_key.
    known.sort_by_key(|id| rank[id]);
    known
        .into_iter()
        .chain(
            ids.iter()
                .map(AsRef::as_ref)
                .filter(|id| !rank.contains_key(id)),
        )
        .map(str::to_owned)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// A fresh scratch dir under the system temp dir (removed on drop).
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(name: &str) -> Self {
            use std::sync::atomic::{AtomicUsize, Ordering};
            static N: AtomicUsize = AtomicUsize::new(0);
            let dir = std::env::temp_dir().join(format!(
                "boa-prefs-{name}-{}-{}",
                std::process::id(),
                N.fetch_add(1, Ordering::Relaxed)
            ));
            let _ = std::fs::remove_dir_all(&dir);
            Self(dir)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    const DEFAULT_JSON: &str = r#"{"rowLimit":500,"confirmDestructive":true,"recordHistory":true,"historyDays":30,"sidebarRowCounts":true,"sidebarViews":true,"recentCount":5,"connectionOrder":[],"treeExpanded":{},"scopes":{},"bottomHeight":280,"sidebarWidth":260,"rightPanelWidth":340,"rightTab":"structure"}"#;

    #[test]
    fn defaults_serialize_like_json_stringify_of_default_prefs() {
        assert_eq!(Prefs::default().to_json(), DEFAULT_JSON);
    }

    #[test]
    fn fields_lists_every_serialized_key_in_order() {
        let value = serde_json::to_value(Prefs::default()).unwrap();
        // Compare as sets: serde_json's Map order depends on its features
        // (sorted, or insertion order with preserve_order). The order of the
        // text follows the struct (and the TS object): checked below.
        let mut keys: Vec<&str> = value
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        keys.sort_unstable();
        let mut sorted_fields = FIELDS.to_vec();
        sorted_fields.sort_unstable();
        assert_eq!(keys, sorted_fields);
        let text = Prefs::default().to_json();
        let positions: Vec<usize> = FIELDS
            .iter()
            .map(|f| text.find(&format!("\"{f}\"")).unwrap())
            .collect();
        assert!(positions.windows(2).all(|w| w[0] < w[1]), "{positions:?}");
        for kept in KEPT_ON_RESET {
            assert!(FIELDS.contains(&kept), "{kept}");
        }
    }

    #[test]
    fn missing_or_garbage_text_gives_the_defaults() {
        assert_eq!(Prefs::from_json(""), Prefs::default());
        assert_eq!(Prefs::from_json("{not json"), Prefs::default());
        assert_eq!(Prefs::from_json("null"), Prefs::default());
        assert_eq!(Prefs::from_json("[1,2]"), Prefs::default());
        assert_eq!(Prefs::from_json("42"), Prefs::default());
        assert_eq!(Prefs::from_json("\"x\""), Prefs::default());
    }

    #[test]
    fn saved_fields_override_the_defaults() {
        let p = Prefs::from_json(r#"{"rowLimit":1000,"sidebarViews":false,"recentCount":8}"#);
        assert_eq!(p.row_limit, 1000);
        assert!(!p.sidebar_views);
        assert_eq!(p.recent_count, 8);
        assert!(p.confirm_destructive);
        assert_eq!(p.history_days, 30);
    }

    #[test]
    fn round_trips_every_field() {
        let mut p = Prefs {
            row_limit: 100,
            confirm_destructive: false,
            record_history: false,
            history_days: 0,
            sidebar_row_counts: false,
            sidebar_views: false,
            recent_count: 10,
            connection_order: vec!["b".into(), "a".into()],
            bottom_height: 400,
            sidebar_width: 300,
            right_panel_width: 500,
            right_tab: RightTab::History,
            ..Prefs::default()
        };
        p.set_expanded("conn:a", true);
        p.scopes.insert("a".into(), Scope::All);
        p.scopes.insert(
            "a/db".into(),
            Scope::Some(vec!["public".into(), "x".into()]),
        );
        let back = Prefs::from_json(&p.to_json());
        assert_eq!(back, p);
        let v: Value = serde_json::from_str(&p.to_json()).unwrap();
        assert_eq!(v["scopes"]["a"], json!("*"));
        assert_eq!(v["scopes"]["a/db"], json!(["public", "x"]));
        assert_eq!(v["treeExpanded"], json!({"conn:a": true}));
        assert_eq!(v["rightTab"], json!("history"));
    }

    #[test]
    fn a_field_of_the_wrong_type_keeps_its_default() {
        let p = Prefs::from_json(
            r#"{"rowLimit":"many","confirmDestructive":"yes","connectionOrder":{"a":1},"sidebarWidth":"wide","treeExpanded":[],"scopes":3,"recentCount":3}"#,
        );
        let d = Prefs::default();
        assert_eq!(p.row_limit, d.row_limit);
        assert_eq!(p.confirm_destructive, d.confirm_destructive);
        assert_eq!(p.connection_order, d.connection_order);
        assert_eq!(p.sidebar_width, d.sidebar_width);
        assert_eq!(p.tree_expanded, d.tree_expanded);
        assert_eq!(p.scopes, d.scopes);
        assert_eq!(p.recent_count, 3);
    }

    #[test]
    fn an_unknown_right_tab_falls_back_to_structure() {
        assert_eq!(
            Prefs::from_json(r#"{"rightTab":"bogus"}"#).right_tab,
            RightTab::Structure
        );
        assert_eq!(
            Prefs::from_json(r#"{"rightTab":"info"}"#).right_tab,
            RightTab::Info
        );
        assert_eq!(
            Prefs::from_json(r#"{"rightTab":"history"}"#).right_tab,
            RightTab::History
        );
        assert_eq!(
            Prefs::from_json(r#"{"rightTab":7}"#).right_tab,
            RightTab::Structure
        );
    }

    #[test]
    fn negative_history_days_fall_back_to_the_default() {
        assert_eq!(Prefs::from_json(r#"{"historyDays":-1}"#).history_days, 30);
        assert_eq!(Prefs::from_json(r#"{"historyDays":"7"}"#).history_days, 30);
        assert_eq!(Prefs::from_json(r#"{"historyDays":0}"#).history_days, 0);
        assert_eq!(Prefs::from_json(r#"{"historyDays":365}"#).history_days, 365);
        assert_eq!(Prefs::from_json(r#"{"historyDays":6.6}"#).history_days, 7);
    }

    #[test]
    fn row_limit_is_capped_at_5000() {
        assert_eq!(
            Prefs::from_json(r#"{"rowLimit":100000}"#).row_limit,
            ROW_LIMIT_MAX
        );
        assert_eq!(Prefs::from_json(r#"{"rowLimit":5000}"#).row_limit, 5000);
        assert_eq!(Prefs::from_json(r#"{"rowLimit":0}"#).row_limit, 500);
        assert_eq!(Prefs::from_json(r#"{"rowLimit":-5}"#).row_limit, 500);
        let p = Prefs {
            row_limit: 9_999,
            ..Prefs::default()
        };
        assert_eq!(p.page_size(), ROW_LIMIT_MAX);
        let p = Prefs {
            row_limit: 0,
            ..Prefs::default()
        };
        assert_eq!(p.page_size(), 1);
        assert_eq!(Prefs::default().page_size(), 500);
    }

    #[test]
    fn layout_numbers_are_read_as_whole_px() {
        let p =
            Prefs::from_json(r#"{"sidebarWidth":301.4,"bottomHeight":250.6,"rightPanelWidth":-3}"#);
        assert_eq!(p.sidebar_width, 301);
        assert_eq!(p.bottom_height, 251);
        assert_eq!(p.right_panel_width, 340);
    }

    #[test]
    fn tree_expanded_keeps_only_true_nodes() {
        let p = Prefs::from_json(r#"{"treeExpanded":{"a":true,"b":false,"c":1,"d":true}}"#);
        assert_eq!(p.tree_expanded.keys().collect::<Vec<_>>(), ["a", "d"]);
        assert!(p.is_expanded("a"));
        assert!(!p.is_expanded("b"));
    }

    #[test]
    fn set_expanded_adds_and_removes_keys() {
        let mut p = Prefs::default();
        p.set_expanded("x", true);
        assert!(p.is_expanded("x"));
        p.set_expanded("x", false);
        assert!(p.tree_expanded.is_empty());
    }

    #[test]
    fn scopes_accept_star_or_lists_and_drop_anything_else() {
        let p = Prefs::from_json(
            r#"{"scopes":{"c1":"*","c1/db":["public","sales"],"bad":"all","worse":[1]}}"#,
        );
        assert_eq!(p.scopes.len(), 2);
        assert_eq!(p.scopes["c1"], Scope::All);
        assert_eq!(
            p.scopes["c1/db"],
            Scope::Some(vec!["public".into(), "sales".into()])
        );
    }

    #[test]
    fn unknown_fields_are_kept_and_written_back() {
        let p = Prefs::from_json(r#"{"rowLimit":100,"futureThing":{"x":[1,2]},"theme":"dark"}"#);
        assert_eq!(p.extra["futureThing"], json!({"x": [1, 2]}));
        let v: Value = serde_json::from_str(&p.to_json()).unwrap();
        assert_eq!(v["futureThing"], json!({"x": [1, 2]}));
        assert_eq!(v["theme"], json!("dark"));
        assert_eq!(v["rowLimit"], json!(100));
    }

    #[test]
    fn reset_restores_settings_and_keeps_the_layout() {
        let mut p = Prefs {
            row_limit: 5000,
            confirm_destructive: false,
            record_history: false,
            history_days: 7,
            sidebar_row_counts: false,
            sidebar_views: false,
            recent_count: 10,
            connection_order: vec!["z".into()],
            bottom_height: 500,
            sidebar_width: 400,
            right_panel_width: 700,
            right_tab: RightTab::Info,
            ..Prefs::default()
        };
        p.set_expanded("n", true);
        p.scopes.insert("z".into(), Scope::All);
        p.extra.insert("future".into(), json!(1));
        let r = p.reset();
        let d = Prefs::default();
        assert_eq!(r.row_limit, d.row_limit);
        assert_eq!(r.confirm_destructive, d.confirm_destructive);
        assert_eq!(r.record_history, d.record_history);
        assert_eq!(r.history_days, d.history_days);
        assert_eq!(r.sidebar_row_counts, d.sidebar_row_counts);
        assert_eq!(r.sidebar_views, d.sidebar_views);
        assert_eq!(r.recent_count, d.recent_count);
        assert_eq!(r.connection_order, p.connection_order);
        assert_eq!(r.tree_expanded, p.tree_expanded);
        assert_eq!(r.scopes, p.scopes);
        assert_eq!(r.bottom_height, 500);
        assert_eq!(r.sidebar_width, 400);
        assert_eq!(r.right_panel_width, 700);
        assert_eq!(r.right_tab, RightTab::Info);
        assert_eq!(r.extra, p.extra);
    }

    #[test]
    fn clamp_sidebar_width_cases() {
        assert_eq!(clamp_sidebar_width(100.), 200);
        assert_eq!(clamp_sidebar_width(200.), 200);
        assert_eq!(clamp_sidebar_width(333.4), 333);
        assert_eq!(clamp_sidebar_width(333.5), 334);
        assert_eq!(clamp_sidebar_width(480.), 480);
        assert_eq!(clamp_sidebar_width(10_000.), 480);
        assert_eq!(clamp_sidebar_width(-50.), 200);
        assert_eq!(clamp_sidebar_width(f64::NAN), 260);
        assert_eq!(clamp_sidebar_width(f64::INFINITY), 260);
    }

    #[test]
    fn clamp_right_panel_width_cases() {
        assert_eq!(clamp_right_panel_width(0.), 280);
        assert_eq!(clamp_right_panel_width(280.), 280);
        assert_eq!(clamp_right_panel_width(512.7), 513);
        assert_eq!(clamp_right_panel_width(800.), 800);
        assert_eq!(clamp_right_panel_width(1200.), 800);
        assert_eq!(clamp_right_panel_width(f64::NAN), 340);
        assert_eq!(clamp_right_panel_width(f64::NEG_INFINITY), 340);
    }

    #[test]
    fn clamp_bottom_height_cases() {
        // No window: 120..=900.
        assert_eq!(clamp_bottom_height(50., None), 120);
        assert_eq!(clamp_bottom_height(280., None), 280);
        assert_eq!(clamp_bottom_height(2000., None), 900);
        // A 820 px window: at most 620.
        assert_eq!(clamp_bottom_height(700., Some(820.)), 620);
        assert_eq!(clamp_bottom_height(600.4, Some(820.)), 600);
        assert_eq!(clamp_bottom_height(10., Some(820.)), 120);
        // A tall window: still at most 900.
        assert_eq!(clamp_bottom_height(1500., Some(2000.)), 900);
        // A window too short for both: the maximum wins, as in the TS.
        assert_eq!(clamp_bottom_height(280., Some(300.)), 100);
        assert_eq!(clamp_bottom_height(280., Some(150.)), 0);
        assert_eq!(clamp_bottom_height(f64::NAN, Some(820.)), 280);
    }

    #[test]
    fn clamped_getters_use_the_saved_values() {
        let p = Prefs {
            sidebar_width: 1000,
            right_panel_width: 10,
            bottom_height: 880,
            ..Prefs::default()
        };
        assert_eq!(p.sidebar_width_px(), 480);
        assert_eq!(p.right_panel_width_px(), 280);
        assert_eq!(p.bottom_height_px(Some(820.)), 620);
        assert_eq!(p.bottom_height_px(None), 880);
    }

    #[test]
    fn history_retention_is_none_for_forever() {
        assert_eq!(Prefs::default().history_retention(), Some(30));
        let p = Prefs {
            history_days: 0,
            ..Prefs::default()
        };
        assert_eq!(p.history_retention(), None);
    }

    fn ids(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn apply_order_without_an_order_keeps_the_ids() {
        assert_eq!(apply_order(&["a", "b", "c"], &[]), ids(&["a", "b", "c"]));
    }

    #[test]
    fn apply_order_puts_ordered_ids_first_then_the_rest() {
        assert_eq!(
            apply_order(&["a", "b", "c", "d"], &ids(&["c", "a"])),
            ids(&["c", "a", "b", "d"])
        );
    }

    #[test]
    fn apply_order_ignores_ids_that_no_longer_exist() {
        assert_eq!(
            apply_order(&["a", "b"], &ids(&["gone", "b", "also-gone"])),
            ids(&["b", "a"])
        );
        assert_eq!(apply_order::<&str>(&[], &ids(&["a"])), Vec::<String>::new());
    }

    #[test]
    fn apply_order_uses_the_last_place_of_a_repeated_id() {
        // new Map([["a",0],["b",1],["a",2]]) ranks a after b.
        assert_eq!(
            apply_order(&["a", "b"], &ids(&["a", "b", "a"])),
            ids(&["b", "a"])
        );
    }

    #[test]
    fn load_without_a_file_gives_the_defaults() {
        let dir = TempDir::new("missing");
        assert!(!exists(&dir.0));
        assert_eq!(load(&dir.0), Prefs::default());
    }

    #[test]
    fn load_of_a_corrupt_file_gives_the_defaults() {
        let dir = TempDir::new("corrupt");
        std::fs::create_dir_all(&dir.0).unwrap();
        std::fs::write(prefs_path(&dir.0), b"{\"rowLimit\": 1000,").unwrap();
        assert_eq!(load(&dir.0), Prefs::default());
        std::fs::write(prefs_path(&dir.0), [0xff, 0xfe, 0x00]).unwrap();
        assert_eq!(load(&dir.0), Prefs::default());
    }

    #[test]
    fn load_of_a_directory_in_place_of_the_file_gives_the_defaults() {
        let dir = TempDir::new("isdir");
        std::fs::create_dir_all(prefs_path(&dir.0)).unwrap();
        assert_eq!(load(&dir.0), Prefs::default());
    }

    #[test]
    fn save_creates_the_dir_and_load_reads_it_back() {
        let dir = TempDir::new("save");
        let nested = dir.0.join("a").join("b");
        let p = Prefs {
            row_limit: 1000,
            sidebar_width: 333,
            right_tab: RightTab::History,
            ..Prefs::default()
        };
        p.save(&nested).unwrap();
        assert!(exists(&nested));
        assert_eq!(load(&nested), p);
        let raw = std::fs::read_to_string(prefs_path(&nested)).unwrap();
        assert_eq!(raw, p.to_json());
        // No temp file left behind.
        let names: Vec<String> = std::fs::read_dir(&nested)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, [PREFS_FILE]);
    }

    #[test]
    #[cfg(unix)]
    fn save_writes_an_owner_only_file() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = TempDir::new("owner-only");
        Prefs::default().save(&dir.0).unwrap();
        let mode = std::fs::metadata(prefs_path(&dir.0))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    #[test]
    fn save_replaces_an_existing_file() {
        let dir = TempDir::new("replace");
        Prefs::default().save(&dir.0).unwrap();
        let p = Prefs {
            recent_count: 3,
            ..Prefs::default()
        };
        p.save(&dir.0).unwrap();
        assert_eq!(load(&dir.0).recent_count, 3);
    }

    #[test]
    fn deserialize_is_lenient_too() {
        let p: Prefs = serde_json::from_str(r#"{"rightTab":"nope","rowLimit":100}"#).unwrap();
        assert_eq!(p.right_tab, RightTab::Structure);
        assert_eq!(p.row_limit, 100);
        let p: Prefs = serde_json::from_str("[]").unwrap();
        assert_eq!(p, Prefs::default());
    }

    #[test]
    fn right_tab_names() {
        assert_eq!(
            RightTab::ALL.map(RightTab::as_str),
            ["structure", "info", "history"]
        );
        assert_eq!(
            RightTab::ALL.map(RightTab::label),
            ["Structure", "Info", "History"]
        );
    }
}
