//! Query history, the UI half: ported from `ui/src/lib/history.ts` (+
//! `history.test.ts`) and the store in `ui/src/state/history.ts` (+
//! `state/history.test.ts`).
//!
//! The on-disk side (`history-<id>.jsonl`, one `Entry` per line) is
//! `crate::history`; this module adds what the UI did on top of it:
//!
//! - `query_key`: what makes two runs "the same query" (values, comments,
//!   spacing and case dropped; an `IN (…)` list counts as one value);
//! - `dedupe_queries`, `merge_newest_first`, `merge_loaded`: one record
//!   per query, newest first, capped at `HISTORY_CAP` and cut at the
//!   retention period (`history_cutoff`, `HISTORY_DAYS` by default);
//! - search (`filter_history`: every word, any order, errors only);
//! - display helpers (`sql_preview`, `outcome_label`, `duration`,
//!   `insertion_text`);
//! - `QueryHistory`: the per-connection lists of the Query History view
//!   (`useQueryHistory` + `recordHistory` / `loadHistory` / …) as a plain
//!   state machine. It does no I/O: its methods say what to write or delete
//!   through `Backend::history_*`, and take the results back.
//!
//! The browser mock's `localHistoryStore` is not ported (the Rust file store
//! replaces it); its test cases run against `crate::history::History`.

use std::borrow::Cow;
use std::collections::{HashMap, HashSet};

use crate::error::CmdError;
pub use crate::history::Entry;
use crate::sql::tokenize::{TokenKind, tokenize};

use super::reconnect::{Toast, ToastKind};

/// Distinct queries kept per connection; the same as [`crate::history::CAP`].
pub const HISTORY_CAP: usize = crate::history::CAP;

/// Days of history kept by default (Settings → Queries; 0 keeps it all).
pub const HISTORY_DAYS: u32 = 30;

const DAY_MS: f64 = 86_400_000.0;

/// Runs before this time (ms since the epoch) are past `days` of retention;
/// `None` when `days` is 0 (keep everything).
pub fn history_cutoff(days: u32, now_ms: f64) -> Option<f64> {
    (days > 0).then(|| now_ms - f64::from(days) * DAY_MS)
}

// ── Same query ────────────────────────────────────────────────────────────

/// JavaScript's `\s`.
fn is_js_space(c: char) -> bool {
    c.is_whitespace() || c == '\u{feff}'
}

/// `String.prototype.trim`.
fn js_trim(s: &str) -> &str {
    s.trim_matches(is_js_space)
}

fn is_word(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

const MARK: char = '\u{1}';

/// Lower-cases `text` except quoted names (`"…"` / `` `…` ``), which are set
/// aside in `quoted` and replaced by a marker (`\u{1}<n>\u{1}`).
fn push_plain(out: &mut String, text: &str, quoted: &mut Vec<String>) {
    let mut rest = text;
    while !rest.is_empty() {
        let first = rest.chars().next().unwrap_or(' ');
        if first == '"' || first == '`' {
            if let Some(close) = rest[1..].find(first) {
                let end = close + 2;
                quoted.push(rest[..end].to_owned());
                out.push(MARK);
                out.push_str(&(quoted.len() - 1).to_string());
                out.push(MARK);
                rest = &rest[end..];
            } else {
                // An unclosed quote matches nothing: kept as is.
                out.push(first);
                rest = &rest[1..];
            }
            continue;
        }
        let end = rest.find(['"', '`']).unwrap_or(rest.len());
        out.push_str(&rest[..end].to_lowercase());
        rest = &rest[end..];
    }
}

fn is_op(c: char) -> bool {
    matches!(
        c,
        '=' | '<' | '>' | '!' | ',' | '(' | ')' | '+' | '-' | '*' | '/' | '%' | '|' | ';'
    )
}

/// `/ ?([=<>!,()+\-*/%|;]) ?/g` → `$1`.
fn tighten_ops(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        let op = if c == ' ' && chars.get(i + 1).copied().is_some_and(is_op) {
            i += 1;
            Some(chars[i])
        } else if is_op(c) {
            Some(c)
        } else {
            None
        };
        match op {
            Some(op) => {
                out.push(op);
                i += 1;
                if chars.get(i) == Some(&' ') {
                    i += 1;
                }
            }
            None => {
                out.push(c);
                i += 1;
            }
        }
    }
    out
}

/// `/\bin\(\?(,\?)+\)/g` → `in(?)`.
fn collapse_in_lists(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    let mut copied = 0;
    while i < b.len() {
        let boundary = i == 0 || !is_word(char::from(b[i - 1]));
        if boundary && b[i..].starts_with(b"in(?,?") {
            let mut j = i + 4; // after "in(?"
            while b[j..].starts_with(b",?") {
                j += 2;
            }
            if b.get(j) == Some(&b')') {
                out.push_str(&s[copied..i]);
                out.push_str("in(?)");
                i = j + 1;
                copied = i;
                continue;
            }
        }
        i += 1;
    }
    out.push_str(&s[copied..]);
    out
}

/// Puts the quoted names set aside by [`push_plain`] back.
fn restore_quoted(s: &str, quoted: &[String]) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(start) = rest.find(MARK) {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        let digits = after.bytes().take_while(u8::is_ascii_digit).count();
        let restored = (digits > 0 && after[digits..].starts_with(MARK))
            .then(|| after[..digits].parse::<usize>().ok())
            .flatten()
            .and_then(|n| quoted.get(n));
        match restored {
            Some(q) => {
                out.push_str(q);
                rest = &after[digits + 1..];
            }
            None => {
                out.push(MARK);
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

/// What makes two statements the same query: the SQL with its values
/// (strings, numbers, `$1` / `:name` parameters) replaced by `?`, comments
/// and spacing dropped, and unquoted text lower-cased. The values of an
/// `in (1, 2, 3)` list count as one, whatever their number. Quoted
/// identifiers are kept as written. (`queryKey`)
pub fn query_key(sql: &str) -> String {
    let mut quoted = Vec::new();
    let mut out = String::with_capacity(sql.len());
    for t in tokenize(sql, None) {
        match t.kind {
            TokenKind::Comment => out.push(' '),
            TokenKind::String | TokenKind::Number | TokenKind::Param => out.push('?'),
            _ => push_plain(&mut out, t.text(sql), &mut quoted),
        }
    }
    // \s+ → ' '
    let mut spaced = String::with_capacity(out.len());
    let mut in_space = false;
    for c in out.chars() {
        if is_js_space(c) {
            if !in_space {
                spaced.push(' ');
            }
            in_space = true;
        } else {
            spaced.push(c);
            in_space = false;
        }
    }
    let tight = collapse_in_lists(&tighten_ops(&spaced));
    let trimmed = js_trim(tight.trim_end_matches([';', ' ']));
    restore_quoted(trimmed, &quoted)
}

/// A record's key (computed for records saved before keys were). (`recordKey`)
pub fn record_key(r: &Entry) -> Cow<'_, str> {
    match &r.key {
        Some(k) => Cow::Borrowed(k),
        None => Cow::Owned(query_key(&r.sql)),
    }
}

/// The newest record of each query, from a newest-first list, capped;
/// `cutoff` drops runs older than it. (`dedupeQueries`)
pub fn dedupe_queries<T: AsRef<Entry> + Clone>(
    items: &[T],
    cap: usize,
    cutoff: Option<f64>,
) -> Vec<T> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for item in items {
        if out.len() >= cap {
            break;
        }
        let r = item.as_ref();
        if cutoff.is_some_and(|c| r.at < c) {
            continue;
        }
        if seen.insert(record_key(r).into_owned()) {
            out.push(item.clone());
        }
    }
    out
}

impl AsRef<Entry> for Entry {
    fn as_ref(&self) -> &Entry {
        self
    }
}

/// A new record id: a random UUID (v4 format), as `crypto.randomUUID()`. (`newHistoryId`)
pub fn new_history_id() -> String {
    use std::hash::{BuildHasher, Hasher};
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let random = || {
        let mut h = std::collections::hash_map::RandomState::new().build_hasher();
        h.write_u64(COUNTER.fetch_add(1, Ordering::Relaxed));
        h.write_u128(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or_default(),
        );
        h.finish()
    };
    let (a, b) = (random(), random());
    format!(
        "{:08x}-{:04x}-4{:03x}-{:x}{:03x}-{:012x}",
        a >> 32,
        (a >> 16) & 0xffff,
        a & 0x0fff,
        8 + ((b >> 62) & 0x3),
        (b >> 48) & 0x0fff,
        b & 0xffff_ffff_ffff
    )
}

/// One line of the session-only Output log (`HistoryEntry` in
/// `ui/src/state/types.ts`): every fetch, including page re-fetches and
/// notes. [`to_record`] turns a statement run into its stored [`Entry`].
#[derive(Debug, Clone, Default, PartialEq)]
pub struct OutputEntry {
    pub id: u64,
    pub sql: String,
    pub database: String,
    /// When it ran (ms since the epoch).
    pub at: f64,
    /// Duration in ms.
    pub ms: f64,
    /// Rows fetched; `None` for statements that return none (and errors).
    pub rows: Option<f64>,
    /// More rows existed than were fetched.
    pub truncated: bool,
    /// Rows skipped before the fetched page.
    pub offset: Option<f64>,
    /// Server detail / hint lines for errors.
    pub detail: Option<String>,
    /// Affected rows, for statements that return none.
    pub affected: Option<f64>,
    /// Editor tab title it ran from (`console_1`).
    pub source: Option<String>,
    pub error: Option<String>,
    /// Outcome line for entries that aren't statement runs (`26 rows exported to …`).
    pub note: Option<String>,
}

/// The stored form of an Output log entry (no session-only fields, no
/// empty ones). (`toRecord`)
pub fn to_record(e: &OutputEntry, id: String) -> Entry {
    let some = |s: &Option<String>| s.clone().filter(|s| !s.is_empty());
    Entry {
        id,
        sql: e.sql.clone(),
        key: Some(query_key(&e.sql)),
        database: e.database.clone(),
        at: e.at,
        ms: e.ms,
        rows: e.rows,
        truncated: e.truncated.then_some(true),
        affected: e.affected,
        source: some(&e.source),
        error: some(&e.error),
        detail: some(&e.detail),
    }
}

// ── Search ────────────────────────────────────────────────────────────────

/// The lower-cased, whitespace-separated words of a search. (`searchTerms`)
pub fn search_terms(query: &str) -> Vec<String> {
    query
        .to_lowercase()
        .split(is_js_space)
        .filter(|t| !t.is_empty())
        .map(str::to_owned)
        .collect()
}

/// What the history view's search box and "Errors" pill ask for. (`HistoryFilter`)
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HistoryFilter {
    pub query: String,
    pub errors_only: bool,
}

/// Records whose SQL contains every whitespace-separated term of the query
/// (case-insensitive, any order); `errors_only` keeps failed ones. Order is
/// kept. (`filterHistory`)
pub fn filter_history<'a, T: AsRef<Entry>>(items: &'a [T], f: &HistoryFilter) -> Vec<&'a T> {
    let terms = search_terms(&f.query);
    if terms.is_empty() && !f.errors_only {
        return items.iter().collect();
    }
    items
        .iter()
        .filter(|item| {
            let r = item.as_ref();
            if f.errors_only && r.error.is_none() {
                return false;
            }
            if terms.is_empty() {
                return true;
            }
            let s = r.sql.to_lowercase();
            terms.iter().all(|t| s.contains(t.as_str()))
        })
        .collect()
}

/// Lists of several connections (each newest first), merged newest first
/// (a stable sort: equal times keep their order). (`mergeNewestFirst`)
pub fn merge_newest_first<T: AsRef<Entry> + Clone>(lists: &[&[T]]) -> Vec<T> {
    let mut all: Vec<T> = lists.iter().flat_map(|l| l.iter().cloned()).collect();
    all.sort_by(|a, b| b.as_ref().at.total_cmp(&a.as_ref().at));
    all
}

/// Prepends records made while a load was in flight to the loaded list
/// (newest first), skipping ones the load already returned, and keeps the
/// newest run of each query, capped and no older than `cutoff`. (`mergeLoaded`)
pub fn merge_loaded(
    loaded: &[Entry],
    pending: &[Entry],
    cap: usize,
    cutoff: Option<f64>,
) -> Vec<Entry> {
    let seen: HashSet<&str> = loaded.iter().map(|r| r.id.as_str()).collect();
    let extra: Vec<Entry> = pending
        .iter()
        .filter(|r| !seen.contains(r.id.as_str()))
        .cloned()
        .collect();
    dedupe_queries(
        &merge_newest_first(&[extra.as_slice(), loaded]),
        cap,
        cutoff,
    )
}

// ── Display ───────────────────────────────────────────────────────────────

/// The first lines of a statement, from [`sql_preview`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SqlPreview {
    pub text: String,
    /// More (non-blank) lines follow.
    pub more: bool,
}

/// The first `lines` non-blank lines of a statement (2 in the history
/// list), and whether there is more. (`sqlPreview`)
pub fn sql_preview(sql: &str, lines: usize) -> SqlPreview {
    let normalized = sql.replace("\r\n", "\n").replace('\r', "\n");
    let all: Vec<&str> = normalized.split('\n').collect();
    let blank = |l: &&str| js_trim(l).is_empty();
    let Some(start) = all.iter().position(|l| !blank(l)) else {
        return SqlPreview {
            text: String::new(),
            more: false,
        };
    };
    let mut end = all.len();
    while end > start && blank(&all[end - 1]) {
        end -= 1;
    }
    SqlPreview {
        text: all[start..end.min(start + lines)].join("\n"),
        more: end - start > lines,
    }
}

/// `Number.prototype.toLocaleString()` in en-US: thousands separated by
/// commas, at most three decimals.
pub(crate) fn locale_number(n: f64) -> String {
    if !n.is_finite() {
        return n.to_string();
    }
    let fixed = format!("{:.3}", n.abs());
    let (int, frac) = fixed.split_once('.').unwrap_or((&fixed, ""));
    let frac = frac.trim_end_matches('0');
    let mut out = String::new();
    if n < 0.0 && (int != "0" || !frac.is_empty()) {
        out.push('-');
    }
    for (i, c) in int.chars().enumerate() {
        if i > 0 && (int.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    if !frac.is_empty() {
        out.push('.');
        out.push_str(frac);
    }
    out
}

/// "12 rows", "12+ rows", "3 affected", "error", "done". (`outcomeLabel`)
pub fn outcome_label(r: &Entry) -> String {
    if r.error.as_deref().is_some_and(|e| !e.is_empty()) {
        return "error".into();
    }
    let truncated = r.truncated == Some(true);
    if let Some(rows) = r.rows {
        let plural = if rows == 1.0 && !truncated { "" } else { "s" };
        let plus = if truncated { "+" } else { "" };
        return format!("{}{plus} row{plural}", locale_number(rows));
    }
    match r.affected {
        Some(a) if a != 0.0 && !a.is_nan() => format!("{} affected", locale_number(a)),
        _ => "done".into(),
    }
}

/// `Math.round`: halves round towards +∞.
fn js_round(x: f64) -> f64 {
    let f = x.floor();
    if x - f >= 0.5 { f + 1.0 } else { f }
}

/// `Number.prototype.toFixed` for 0 or 1 digits (rounds the decimal value half up).
fn to_fixed(x: f64, digits: usize) -> String {
    // Near-exact decimal expansion, rounded half up by hand.
    let s = format!("{:.*}", digits + 30, x.abs());
    let (int, frac) = s.split_once('.').unwrap_or((&s, ""));
    let mut ds: Vec<u8> = int.bytes().chain(frac.bytes().take(digits)).collect();
    if frac.as_bytes().get(digits).is_some_and(|&d| d >= b'5') {
        let mut k = ds.len();
        loop {
            if k == 0 {
                ds.insert(0, b'1');
                break;
            }
            k -= 1;
            if ds[k] == b'9' {
                ds[k] = b'0';
            } else {
                ds[k] += 1;
                break;
            }
        }
    }
    let split = ds.len() - digits;
    let mut out = String::new();
    if x < 0.0 {
        out.push('-');
    }
    out.push_str(std::str::from_utf8(&ds[..split]).unwrap_or("0"));
    if digits > 0 {
        out.push('.');
        out.push_str(std::str::from_utf8(&ds[split..]).unwrap_or(""));
    }
    out
}

/// "850 ms", "1.2 s", "2m 5s". (`duration`)
pub fn duration(ms: f64) -> String {
    if ms < 1000.0 {
        return format!("{} ms", js_round(ms));
    }
    if ms < 60_000.0 {
        return format!(
            "{} s",
            to_fixed(ms / 1000.0, if ms < 10_000.0 { 1 } else { 0 })
        );
    }
    let s = js_round(ms / 1000.0) as u64;
    format!("{}m {}s", s / 60, s % 60)
}

/// Text to insert at the caret of a console: the statement on a line of its
/// own when the caret is at the end (or start) of a line holding other text;
/// as is in the middle of a line (e.g. as a subquery). `before` / `after`:
/// the console's text around the caret. (`insertionText`)
pub fn insertion_text(before: &str, after: &str, sql: &str) -> String {
    let line_before = &before[before.rfind('\n').map_or(0, |i| i + 1)..];
    let line_after = &after[..after.find('\n').unwrap_or(after.len())];
    let text_before = !js_trim(line_before).is_empty();
    let text_after = !js_trim(line_after).is_empty();
    match (text_before, text_after) {
        (true, false) => format!("\n{sql}"),
        (false, true) => format!("{sql}\n"),
        _ => sql.to_owned(),
    }
}

// ── The Query History view's state (state/history.ts) ─────────────────────

/// A record with the connection it belongs to, as the history view lists it
/// (all connections merged). (`HistoryItem`)
#[derive(Debug, Clone, PartialEq)]
pub struct HistoryItem {
    pub connection_id: String,
    pub entry: Entry,
}

impl AsRef<Entry> for HistoryItem {
    fn as_ref(&self) -> &Entry {
        &self.entry
    }
}

/// The two Settings that drive history (`recordHistory`, `historyDays` in prefs).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HistoryPrefs {
    /// Settings → Queries → "Record query history".
    pub record: bool,
    /// Retention in days; 0 keeps everything.
    pub days: u32,
}

impl Default for HistoryPrefs {
    fn default() -> Self {
        Self {
            record: true,
            days: HISTORY_DAYS,
        }
    }
}

/// What [`QueryHistory::record`] asks the caller to do.
#[derive(Debug, Clone, PartialEq)]
pub struct RecordPlan {
    /// `Backend::history_append(connection_id, entry, Some(days))`, then
    /// [`QueryHistory::append_done`] with its result.
    pub append: Entry,
    pub days: u32,
    /// Earlier runs of this query saved without a key (before runs were
    /// deduplicated): `Backend::history_delete` them (errors ignored), or
    /// they would come back once this run is deleted.
    pub delete_legacy: Vec<String>,
}

/// What [`QueryHistory::finish_load`] asks the caller to do.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct LoadOutcome {
    /// Earlier runs of listed queries (saved before runs were deduplicated):
    /// `Backend::history_delete` them (errors ignored), or they would
    /// resurface when the listed one is deleted.
    pub stale: Vec<String>,
    /// "Could not read query history" when the read failed.
    pub toast: Option<Toast>,
}

/// Toast titles of the history operations (`reportError` titles in state/history.ts).
pub const SAVE_FAILED: &str = "Could not save query history";
pub const READ_FAILED: &str = "Could not read query history";
pub const DELETE_FAILED: &str = "Could not delete history entry";
pub const CLEAR_FAILED: &str = "Could not clear query history";

/// `reportError(title, err)`: an error toast with the error's message.
pub fn error_toast(title: &str, err: &CmdError) -> Toast {
    Toast::new(ToastKind::Error, title).body(err.message.clone())
}

/// The Query History view's state (`useQueryHistory` and the functions of
/// `ui/src/state/history.ts`): connections' lists, newest first, loaded
/// lazily the first time the view needs one and then following new runs in
/// memory. Pure: the caller does the `Backend::history_*` calls the methods
/// ask for and reports back.
///
/// Typical flow in a GPUI model:
/// - a statement ran: [`record`](Self::record) → append (+ legacy deletes)
///   → [`append_done`](Self::append_done);
/// - the view shows connection `id`: [`begin_load`](Self::begin_load) →
///   `history_list(id, Some(days))` → [`finish_load`](Self::finish_load) →
///   delete `stale`;
/// - delete / clear: [`delete_entries`](Self::delete_entries) /
///   [`clear`](Self::clear) at once, then the backend call; on error show
///   [`error_toast`] and force a reload;
/// - retention changed: reload [`days_changed`](Self::days_changed)'s ids with `force`.
#[derive(Debug, Default, Clone)]
pub struct QueryHistory {
    /// The Query History view is open.
    pub open: bool,
    /// Loaded histories by connection id, newest first.
    lists: HashMap<String, Vec<Entry>>,
    /// Connections whose history is being read.
    loading: HashSet<String>,
    /// Records made while a connection's history was loading, merged in afterwards.
    pending: HashMap<String, Vec<Entry>>,
    /// The last append failed (one toast per failure streak).
    append_failed: bool,
}

impl QueryHistory {
    pub fn new() -> Self {
        Self::default()
    }

    /// Opens the view (`openHistory`).
    pub fn open_view(&mut self) {
        self.open = true;
    }

    /// `toggleHistory`.
    pub fn toggle(&mut self) {
        self.open = !self.open;
    }

    /// A connection's loaded list, newest first (`None`: not loaded).
    pub fn list(&self, connection_id: &str) -> Option<&[Entry]> {
        self.lists.get(connection_id).map(Vec::as_slice)
    }

    /// Its history is being read.
    pub fn is_loading(&self, connection_id: &str) -> bool {
        self.loading.contains(connection_id)
    }

    /// The loaded lists of `ids` (the active connection, or all of them in
    /// the view's order) merged newest first, with their connection.
    pub fn items(&self, ids: &[String]) -> Vec<HistoryItem> {
        let lists: Vec<Vec<HistoryItem>> = ids
            .iter()
            .map(|id| {
                self.lists
                    .get(id)
                    .map(|l| {
                        l.iter()
                            .map(|e| HistoryItem {
                                connection_id: id.clone(),
                                entry: e.clone(),
                            })
                            .collect()
                    })
                    .unwrap_or_default()
            })
            .collect();
        let slices: Vec<&[HistoryItem]> = lists.iter().map(Vec::as_slice).collect();
        merge_newest_first(&slices)
    }

    /// Saves a statement the user ran (not page re-fetches) to its
    /// connection's history, unless recording is off (`None`). The record
    /// gets a new id. (`recordHistory`)
    pub fn record(
        &mut self,
        connection_id: &str,
        entry: &OutputEntry,
        prefs: HistoryPrefs,
        now_ms: f64,
    ) -> Option<RecordPlan> {
        self.record_with_id(connection_id, entry, prefs, now_ms, new_history_id())
    }

    /// [`record`](Self::record) with a given record id (tests).
    pub fn record_with_id(
        &mut self,
        connection_id: &str,
        entry: &OutputEntry,
        prefs: HistoryPrefs,
        now_ms: f64,
        id: String,
    ) -> Option<RecordPlan> {
        if !prefs.record {
            return None;
        }
        let rec = to_record(entry, id);
        let key = rec.key.clone().unwrap_or_default();
        let delete_legacy = self
            .lists
            .get(connection_id)
            .map(|l| {
                l.iter()
                    .filter(|r| r.key.is_none() && record_key(r) == key.as_str())
                    .map(|r| r.id.clone())
                    .collect()
            })
            .unwrap_or_default();
        if let Some(list) = self.lists.get_mut(connection_id) {
            let mut with = Vec::with_capacity(list.len() + 1);
            with.push(rec.clone());
            with.append(list);
            *list = dedupe_queries(&with, HISTORY_CAP, history_cutoff(prefs.days, now_ms));
        }
        // A (re)load in flight may read the file before this append lands.
        if self.loading.contains(connection_id) {
            self.pending
                .entry(connection_id.to_owned())
                .or_default()
                .insert(0, rec.clone());
        }
        Some(RecordPlan {
            append: rec,
            days: prefs.days,
            delete_legacy,
        })
    }

    /// The result of a [`RecordPlan::append`]: a toast for the first failure
    /// of a streak, not one per statement.
    pub fn append_done(&mut self, result: Result<(), &CmdError>) -> Option<Toast> {
        match result {
            Ok(()) => {
                self.append_failed = false;
                None
            }
            Err(e) => {
                let first = !self.append_failed;
                self.append_failed = true;
                first.then(|| error_toast(SAVE_FAILED, e))
            }
        }
    }

    /// Starts reading a connection's history: true when the caller should
    /// call `history_list` (not when one is in flight, or it is loaded and
    /// not `force`d). (`loadHistory`, first half)
    pub fn begin_load(&mut self, connection_id: &str, force: bool) -> bool {
        if self.loading.contains(connection_id)
            || (self.lists.contains_key(connection_id) && !force)
        {
            return false;
        }
        self.loading.insert(connection_id.to_owned());
        self.pending.insert(connection_id.to_owned(), Vec::new());
        true
    }

    /// The read's result (`history_list` order: oldest first), with the
    /// retention it was read with. (`loadHistory`, second half)
    pub fn finish_load(
        &mut self,
        connection_id: &str,
        result: Result<Vec<Entry>, &CmdError>,
        days: u32,
        now_ms: f64,
    ) -> LoadOutcome {
        let pending = self.pending.remove(connection_id).unwrap_or_default();
        self.loading.remove(connection_id);
        match result {
            Ok(list) => {
                let newest_first: Vec<Entry> = list.iter().rev().cloned().collect();
                let merged = merge_loaded(
                    &newest_first,
                    &pending,
                    HISTORY_CAP,
                    history_cutoff(days, now_ms),
                );
                let kept: HashSet<&str> = merged.iter().map(|r| r.id.as_str()).collect();
                let stale = list
                    .iter()
                    .filter(|r| !kept.contains(r.id.as_str()))
                    .map(|r| r.id.clone())
                    .collect();
                self.lists.insert(connection_id.to_owned(), merged);
                LoadOutcome { stale, toast: None }
            }
            Err(e) => LoadOutcome {
                stale: Vec::new(),
                toast: Some(error_toast(READ_FAILED, e)),
            },
        }
    }

    /// Drops entries from the loaded list at once; then call
    /// `history_delete` (on error: [`error_toast`]`(DELETE_FAILED, e)` and a
    /// forced reload). (`deleteHistoryEntries`)
    pub fn delete_entries(&mut self, connection_id: &str, ids: &[String]) {
        let drop: HashSet<&str> = ids.iter().map(String::as_str).collect();
        if let Some(list) = self.lists.get_mut(connection_id) {
            list.retain(|r| !drop.contains(r.id.as_str()));
        }
    }

    /// Empties the connection's list at once; then call `history_clear` (on
    /// error: [`error_toast`]`(CLEAR_FAILED, e)` and a forced reload). (`clearHistory`)
    pub fn clear(&mut self, connection_id: &str) {
        self.lists.insert(connection_id.to_owned(), Vec::new());
    }

    /// A new retention period: the loaded connections to re-read with
    /// `force` (the files keep runs past a shorter period until the next
    /// statement is saved). Empty when it did not change.
    pub fn days_changed(&self, prev: u32, next: u32) -> Vec<String> {
        if prev == next {
            return Vec::new();
        }
        let mut ids: Vec<String> = self.lists.keys().cloned().collect();
        ids.sort();
        ids
    }

    /// Drops a deleted connection's list (the backend deletes its file). (`forgetHistory`)
    pub fn forget(&mut self, connection_id: &str) {
        self.pending.remove(connection_id);
        self.lists.remove(connection_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::history::History;
    use std::time::Instant;

    fn rec(id: &str, sql: &str, at: f64) -> Entry {
        Entry {
            id: id.into(),
            sql: sql.into(),
            key: None,
            database: "app".into(),
            at,
            ms: 5.0,
            rows: None,
            truncated: None,
            affected: None,
            source: None,
            error: None,
            detail: None,
        }
    }

    fn ids<T: AsRef<Entry>>(items: &[T]) -> Vec<&str> {
        items.iter().map(|r| r.as_ref().id.as_str()).collect()
    }

    // describe('toRecord')

    #[test]
    fn keeps_what_matters_and_drops_nulls_and_session_only_fields() {
        let r = to_record(
            &OutputEntry {
                id: 7,
                sql: "select 1".into(),
                database: "app".into(),
                at: 10.0,
                ms: 2.0,
                rows: Some(1.0),
                offset: Some(0.0),
                source: Some("console_1".into()),
                ..OutputEntry::default()
            },
            "r1".into(),
        );
        assert_eq!(
            serde_json::to_string(&r).unwrap(),
            r#"{"id":"r1","sql":"select 1","key":"select ?","database":"app","at":10.0,"ms":2.0,"rows":1.0,"source":"console_1"}"#
        );
        let r = to_record(
            &OutputEntry {
                id: 8,
                sql: "select x".into(),
                database: "app".into(),
                at: 11.0,
                ms: 3.0,
                rows: None,
                error: Some("[42703] no x".into()),
                detail: Some("Hint: y".into()),
                ..OutputEntry::default()
            },
            "r2".into(),
        );
        assert_eq!(
            r,
            Entry {
                key: Some("select x".into()),
                ms: 3.0,
                error: Some("[42703] no x".into()),
                detail: Some("Hint: y".into()),
                ..rec("r2", "select x", 11.0)
            }
        );
        let r = to_record(
            &OutputEntry {
                id: 9,
                sql: "delete from t".into(),
                database: "app".into(),
                at: 1.0,
                ms: 1.0,
                affected: Some(0.0),
                ..OutputEntry::default()
            },
            "r3".into(),
        );
        assert_eq!(r.affected, Some(0.0));
    }

    #[test]
    fn makes_unique_ids() {
        let e = OutputEntry {
            id: 1,
            sql: "s".into(),
            database: "d".into(),
            at: 1.0,
            ms: 1.0,
            ..OutputEntry::default()
        };
        let (a, b) = (new_history_id(), new_history_id());
        assert_ne!(to_record(&e, a.clone()).id, to_record(&e, b).id);
        assert_eq!(a.len(), 36);
        assert_eq!(a.as_bytes()[14], b'4');
    }

    // describe('filterHistory')

    fn items() -> Vec<Entry> {
        vec![
            rec("1", "SELECT * FROM users WHERE id = 1", 3.0),
            Entry {
                error: Some("boom".into()),
                ..rec("2", "select name from Orders", 2.0)
            },
            rec("3", "update users set name = 1", 1.0),
        ]
    }

    fn filter<'a>(items: &'a [Entry], query: &str, errors_only: bool) -> Vec<&'a str> {
        filter_history(
            items,
            &HistoryFilter {
                query: query.into(),
                errors_only,
            },
        )
        .into_iter()
        .map(|r| r.id.as_str())
        .collect()
    }

    #[test]
    fn returns_everything_for_an_empty_query() {
        let items = items();
        let all = filter_history(
            &items,
            &HistoryFilter {
                query: "  ".into(),
                errors_only: false,
            },
        );
        assert_eq!(all.len(), 3);
        assert!(all.iter().zip(&items).all(|(a, b)| std::ptr::eq(*a, b)));
    }

    #[test]
    fn matches_every_word_case_insensitively_in_any_order() {
        let items = items();
        assert_eq!(filter(&items, "USERS", false), ["1", "3"]);
        assert_eq!(filter(&items, "name users", false), ["3"]);
        assert_eq!(filter(&items, "orders", false), ["2"]);
        assert!(filter(&items, "nothing", false).is_empty());
    }

    #[test]
    fn filters_errors_only() {
        let items = items();
        assert_eq!(filter(&items, "", true), ["2"]);
        assert!(filter(&items, "users", true).is_empty());
    }

    #[test]
    fn stays_fast_with_thousands_of_entries() {
        let many: Vec<Entry> = (0..5000)
            .map(|i| {
                rec(
                    &i.to_string(),
                    &format!("select col_{i} from table_{} where x = {i}", i % 50),
                    f64::from(i),
                )
            })
            .collect();
        let t = Instant::now();
        for q in [
            "s",
            "se",
            "sel",
            "select",
            "select table_7",
            "select table_7 col_4",
        ] {
            let _ = filter(&many, q, false);
        }
        assert_eq!(filter(&many, "table_7 col_4007", false), ["4007"]);
        // 250 ms in the TS test; generous for unoptimised builds on a busy machine.
        assert!(t.elapsed().as_millis() < 1000, "{:?}", t.elapsed());
    }

    // describe('queryKey')

    #[track_caller]
    fn same(a: &str, b: &str) {
        assert_eq!(query_key(a), query_key(b), "{a:?} vs {b:?}");
    }

    #[track_caller]
    fn differ(a: &str, b: &str) {
        assert_ne!(query_key(a), query_key(b), "{a:?} vs {b:?}");
    }

    #[test]
    fn ignores_values() {
        same(
            "select * from users where id = 1",
            "select * from users where id = 42",
        );
        same(
            "select * from t where name = 'ann'",
            "select * from t where name = 'bob''s'",
        );
        same(
            "select * from t where a = $1 and b = :b",
            "select * from t where a = 7 and b = 'x'",
        );
        same(
            "select * from t where id in (1, 2, 3)",
            "select * from t where id in (9)",
        );
        same(
            "select * from t limit 10 offset 20",
            "select * from t limit 50 offset 0",
        );
    }

    #[test]
    fn ignores_case_spacing_comments_and_a_trailing_semicolon() {
        same(
            "SELECT *\n  FROM Users -- all\n WHERE id=1;",
            "select * from users where id = 2",
        );
        same("/* x */ select 1", "select 2 ;");
    }

    #[test]
    fn tells_different_queries_apart() {
        differ(
            "select * from users where id = 1",
            "select * from orders where id = 1",
        );
        differ("select a from t", "select b from t");
        differ("select * from t1", "select * from t2");
        differ(r#"select * from "Users""#, r#"select * from "users""#);
        differ("select * from t where a = 1", "select * from t where a > 1");
    }

    #[test]
    fn counts_values_as_one_only_in_an_in_list() {
        same(
            "select * from t where id not in (1,2)",
            "select * from t where id NOT IN (3, 4, 5)",
        );
        differ("select 1", "select 1, 2");
        differ("select round(1.25)", "select round(1.25, 1)");
        differ(
            "insert into t values (1, 2)",
            "insert into t values (1, 2, 3)",
        );
        differ("select join(1, 2)", "select join(1)");
    }

    #[test]
    fn keeps_quoted_identifiers_as_written() {
        differ(r#"select * from "a  b""#, r#"select * from "a b""#);
        differ(r#"select "a + b" from t"#, r#"select "a+b" from t"#);
        differ("select * from `Users`", "select * from `users`");
        same(
            r#"SELECT * FROM "My Table"  WHERE id = 1"#,
            r#"select * from "My Table" where id=2"#,
        );
        assert_eq!(
            query_key(r#"SELECT "A  b", c FROM "T 1" WHERE x = 5"#),
            r#"select "A  b",c from "T 1" where x=?"#
        );
    }

    // describe('dedupeQueries')

    #[test]
    fn keeps_the_newest_run_of_each_query_capped() {
        let items = vec![
            rec("4", "select * from t where id = 4", 4.0),
            rec("3", "select * from u", 3.0),
            Entry {
                key: Some(query_key("select * from t where id = 2")),
                ..rec("2", "select * from t where id = 2", 2.0)
            },
            rec("1", "select * from v", 1.0),
        ];
        assert_eq!(
            ids(&dedupe_queries(&items, HISTORY_CAP, None)),
            ["4", "3", "1"]
        );
        assert_eq!(ids(&dedupe_queries(&items, 2, None)), ["4", "3"]);
        assert_eq!(ids(&dedupe_queries(&items, 10, Some(3.0))), ["4", "3"]);
    }

    #[test]
    fn has_a_cutoff_only_for_a_retention_period() {
        assert_eq!(history_cutoff(30, 100.0 * DAY_MS), Some(70.0 * DAY_MS));
        assert_eq!(history_cutoff(0, 5.0), None);
    }

    // describe('merging')

    #[test]
    fn merges_connections_newest_first() {
        let a = vec![rec("a2", "x", 20.0), rec("a1", "x", 5.0)];
        let b = vec![rec("b1", "x", 10.0)];
        assert_eq!(
            ids(&merge_newest_first(&[a.as_slice(), b.as_slice()])),
            ["a2", "b1", "a1"]
        );
    }

    #[test]
    fn adds_records_made_during_a_load_without_duplicates_capped() {
        let loaded = vec![rec("2", "b", 2.0), rec("1", "a", 1.0)];
        let pending = vec![rec("3", "c", 3.0), rec("2", "b", 2.0)];
        assert_eq!(
            ids(&merge_loaded(&loaded, &pending, HISTORY_CAP, None)),
            ["3", "2", "1"]
        );
        assert_eq!(ids(&merge_loaded(&loaded, &pending, 2, None)), ["3", "2"]);
    }

    // describe('display')

    fn preview(text: &str, more: bool) -> SqlPreview {
        SqlPreview {
            text: text.into(),
            more,
        }
    }

    #[test]
    fn previews_the_first_non_blank_lines() {
        assert_eq!(
            sql_preview("\n\n  select a,\n  b\nfrom t\n", 2),
            preview("  select a,\n  b", true)
        );
        assert_eq!(
            sql_preview("select 1\r\n\r\n", 2),
            preview("select 1", false)
        );
        assert_eq!(sql_preview("a\nb\n\n", 2), preview("a\nb", false));
        assert_eq!(sql_preview("   ", 2), preview("", false));
    }

    #[test]
    fn labels_outcomes() {
        let with = |f: fn(&mut Entry)| {
            let mut r = rec("1", "x", 1.0);
            f(&mut r);
            outcome_label(&r)
        };
        assert_eq!(with(|r| r.rows = Some(1.0)), "1 row");
        assert_eq!(
            with(|r| {
                r.rows = Some(500.0);
                r.truncated = Some(true);
            }),
            "500+ rows"
        );
        assert_eq!(with(|r| r.rows = Some(0.0)), "0 rows");
        assert_eq!(with(|r| r.affected = Some(3.0)), "3 affected");
        assert_eq!(with(|r| r.affected = Some(0.0)), "done");
        assert_eq!(
            with(|r| {
                r.error = Some("e".into());
                r.rows = Some(2.0);
            }),
            "error"
        );
        // toLocaleString groups thousands.
        assert_eq!(with(|r| r.rows = Some(12_345.0)), "12,345 rows");
        assert_eq!(
            with(|r| r.affected = Some(1_000_000.0)),
            "1,000,000 affected"
        );
    }

    #[test]
    fn formats_durations() {
        assert_eq!(duration(12.0), "12 ms");
        assert_eq!(duration(1234.0), "1.2 s");
        assert_eq!(duration(12_400.0), "12 s");
        assert_eq!(duration(125_000.0), "2m 5s");
        // toFixed rounds the decimal value half up.
        assert_eq!(duration(1250.0), "1.3 s");
    }

    // describe('localHistoryStore'): the browser mock's store is replaced by
    // the history files; the same sequence against `crate::history::History`
    // (its cap is fixed at 5,000, so the cap-of-3 step is left out).

    fn temp_dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "boa-state-history-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn keyed(id: &str, sql: &str, at: f64) -> Entry {
        Entry {
            key: Some(query_key(sql)),
            ..rec(id, sql, at)
        }
    }

    #[test]
    fn appends_oldest_first_deletes_and_clears_per_connection() {
        let dir = temp_dir("store");
        let h = History::default();
        let list = |id: &str, cutoff: Option<f64>| -> Vec<String> {
            h.list(&dir, id, cutoff)
                .unwrap()
                .into_iter()
                .map(|e| e.id)
                .collect()
        };
        for i in [2, 3, 4] {
            h.append(
                &dir,
                "c1",
                &keyed(
                    &i.to_string(),
                    &format!("select {i} from t{i}"),
                    f64::from(i),
                ),
                None,
            )
            .unwrap();
        }
        h.append(&dir, "c2", &keyed("z", "x", 9.0), None).unwrap();
        assert_eq!(list("c1", None), ["2", "3", "4"]);
        // The same query again, with another value: replaces its earlier entry.
        h.append(&dir, "c1", &keyed("3b", "select 30 from t3", 5.0), None)
            .unwrap();
        assert_eq!(list("c1", None), ["2", "4", "3b"]);
        h.delete(&dir, "c1", &["3b".into()]).unwrap();
        h.append(&dir, "c1", &keyed("3", "select 3 from t3", 6.0), None)
            .unwrap();
        assert_eq!(list("c1", None), ["2", "4", "3"]);
        // Runs before the cutoff are hidden by a list and dropped by an append.
        assert_eq!(list("c1", Some(5.0)), ["3"]);
        h.append(&dir, "c1", &keyed("5", "select 5 from t5", 7.0), Some(5.0))
            .unwrap();
        assert_eq!(list("c1", None), ["3", "5"]);
        h.delete(&dir, "c1", &["3".into()]).unwrap();
        assert_eq!(list("c1", None), ["5"]);
        h.clear(&dir, "c1").unwrap();
        assert!(list("c1", None).is_empty());
        assert_eq!(list("c2", None), ["z"]);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn survives_missing_or_corrupt_storage() {
        let dir = temp_dir("corrupt");
        let h = History::default();
        assert!(h.list(&dir.join("missing"), "c", None).unwrap().is_empty());
        std::fs::write(dir.join("history-c.jsonl"), "{nope").unwrap();
        assert!(h.list(&dir, "c", None).unwrap().is_empty());
        std::fs::remove_dir_all(dir).unwrap();
    }

    // describe('insertionText')

    #[test]
    fn puts_the_statement_on_its_own_line_at_a_line_end_or_start() {
        assert_eq!(insertion_text("select 1;", "", "select 2"), "\nselect 2");
        assert_eq!(
            insertion_text("select 1;", "\nselect 3", "select 2"),
            "\nselect 2"
        );
        assert_eq!(insertion_text("", "select 1;", "select 2"), "select 2\n");
        assert_eq!(insertion_text("a\n", "b", "x"), "x\n");
    }

    #[test]
    fn inserts_as_is_on_an_empty_line_or_mid_line() {
        assert_eq!(insertion_text("", "", "select 2"), "select 2");
        assert_eq!(insertion_text("a\n  ", "\nb", "x"), "x");
        assert_eq!(
            insertion_text("select * from (", ") t", "select 1"),
            "select 1"
        );
    }

    // describe('query history state') (state/history.test.ts), with the
    // same in-memory backend mock: it keeps every append as is.

    const T0: f64 = 1_800_000_000_000.0;

    #[derive(Default)]
    struct Disk(HashMap<String, Vec<Entry>>);

    impl Disk {
        fn list(&self, id: &str, days: u32, now: f64) -> Vec<Entry> {
            let cutoff = history_cutoff(days, now);
            self.0
                .get(id)
                .map(|l| {
                    l.iter()
                        .filter(|r| cutoff.is_none_or(|c| r.at >= c))
                        .cloned()
                        .collect()
                })
                .unwrap_or_default()
        }
        fn sqls(&self, id: &str) -> Vec<String> {
            self.0
                .get(id)
                .map(|l| l.iter().map(|r| r.sql.clone()).collect())
                .unwrap_or_default()
        }
    }

    /// The store plus its backend calls, run to completion like the TS awaits.
    struct Harness {
        h: QueryHistory,
        disk: Disk,
        prefs: HistoryPrefs,
        n: u32,
    }

    impl Harness {
        fn new() -> Self {
            Self {
                h: QueryHistory::new(),
                disk: Disk::default(),
                prefs: HistoryPrefs::default(),
                n: 0,
            }
        }

        fn entry(sql: &str, at: f64) -> OutputEntry {
            OutputEntry {
                id: at as u64,
                sql: sql.into(),
                database: "app".into(),
                at: T0 + at,
                ms: 1.0,
                rows: Some(1.0),
                ..OutputEntry::default()
            }
        }

        fn record(&mut self, id: &str, sql: &str, at: f64) {
            self.n += 1;
            let plan = self.h.record_with_id(
                id,
                &Self::entry(sql, at),
                self.prefs,
                T0,
                format!("r{}", self.n),
            );
            if let Some(plan) = plan {
                self.delete_on_disk(id, &plan.delete_legacy);
                self.disk.0.entry(id.into()).or_default().push(plan.append);
                assert_eq!(self.h.append_done(Ok(())), None);
            }
        }

        fn delete_on_disk(&mut self, id: &str, ids: &[String]) {
            if let Some(l) = self.disk.0.get_mut(id) {
                l.retain(|r| !ids.contains(&r.id));
            }
        }

        fn load(&mut self, id: &str, force: bool) {
            if self.h.begin_load(id, force) {
                let list = self.disk.list(id, self.prefs.days, T0);
                let out = self.h.finish_load(id, Ok(list), self.prefs.days, T0);
                assert_eq!(out.toast, None);
                self.delete_on_disk(id, &out.stale);
            }
        }

        fn set_days(&mut self, days: u32) {
            let prev = self.prefs.days;
            self.prefs.days = days;
            for id in self.h.days_changed(prev, days) {
                self.load(&id, true);
            }
        }

        fn sqls(&self, id: &str) -> Option<Vec<String>> {
            self.h
                .list(id)
                .map(|l| l.iter().map(|r| r.sql.clone()).collect())
        }
    }

    #[test]
    fn saves_runs_loads_newest_first_and_follows_new_runs() {
        let mut t = Harness::new();
        t.record("c1", "select a", 1.0);
        t.record("c1", "select b", 2.0);
        t.load("c1", false);
        assert_eq!(t.sqls("c1").unwrap(), ["select b", "select a"]);
        t.record("c1", "select c", 3.0);
        assert_eq!(t.sqls("c1").unwrap(), ["select c", "select b", "select a"]);
        assert_eq!(t.disk.0["c1"].len(), 3);
    }

    #[test]
    fn keeps_one_entry_per_query_with_the_latest_values() {
        let mut t = Harness::new();
        t.record("c1", "select * from users where id = 1", 1.0);
        t.record("c1", "select * from orders", 2.0);
        t.load("c1", false);
        t.record("c1", "SELECT * FROM users\n  WHERE id = 2", 3.0);
        assert_eq!(
            t.sqls("c1").unwrap(),
            [
                "SELECT * FROM users\n  WHERE id = 2",
                "select * from orders"
            ]
        );
        // A fresh read (the mocked disk keeps every append) dedupes the same way.
        t.load("c1", true);
        assert_eq!(
            t.sqls("c1").unwrap(),
            [
                "SELECT * FROM users\n  WHERE id = 2",
                "select * from orders"
            ]
        );
        // The earlier run is removed from disk too.
        let ats: Vec<f64> = t.disk.0["c1"].iter().map(|r| r.at - T0).collect();
        assert_eq!(ats, [2.0, 3.0]);
    }

    #[test]
    fn replaces_an_earlier_run_saved_without_a_key_on_disk_too() {
        let mut t = Harness::new();
        t.disk.0.insert(
            "c1".into(),
            vec![
                rec("old", "select * from users where id = 1", T0 - DAY_MS),
                rec("other", "select * from orders", T0 - DAY_MS + 1.0),
            ],
        );
        t.load("c1", false);
        t.record("c1", "select * from users where id = 2", 5.0);
        assert_eq!(
            t.sqls("c1").unwrap(),
            ["select * from users where id = 2", "select * from orders"]
        );
        assert_eq!(
            t.disk.sqls("c1"),
            ["select * from orders", "select * from users where id = 2"]
        );
    }

    #[test]
    fn keeps_the_last_30_days_by_default_or_the_period_set() {
        let mut t = Harness::new();
        t.record("c1", "select a", -40.0 * DAY_MS);
        t.record("c1", "select b", -20.0 * DAY_MS);
        t.record("c1", "select c", -2.0 * DAY_MS);
        t.load("c1", false);
        assert_eq!(t.sqls("c1").unwrap(), ["select c", "select b"]);
        t.set_days(7);
        assert_eq!(t.sqls("c1").unwrap(), ["select c"]);
        t.set_days(0);
        assert_eq!(t.sqls("c1").unwrap(), ["select c", "select b", "select a"]);
    }

    #[test]
    fn keeps_runs_made_while_loading() {
        let mut t = Harness::new();
        t.record("c1", "old", 1.0);
        assert!(t.h.begin_load("c1", false));
        // The read happens when the load starts …
        let list = t.disk.list("c1", t.prefs.days, T0);
        assert!(t.h.is_loading("c1"));
        assert!(!t.h.begin_load("c1", true), "one read at a time");
        // … and a run lands before it answers.
        t.record("c1", "during", 2.0);
        let out = t.h.finish_load("c1", Ok(list), t.prefs.days, T0);
        assert_eq!(out, LoadOutcome::default());
        assert!(!t.h.is_loading("c1"));
        assert_eq!(t.sqls("c1").unwrap(), ["during", "old"]);
    }

    #[test]
    fn records_nothing_when_turned_off() {
        let mut t = Harness::new();
        t.prefs.record = false;
        t.record("c1", "select 1", 1.0);
        t.load("c1", false);
        assert_eq!(t.sqls("c1").unwrap(), Vec::<String>::new());
        assert!(!t.disk.0.contains_key("c1"));
    }

    #[test]
    fn deletes_entries_and_clears_a_connection() {
        let mut t = Harness::new();
        t.record("c1", "a", 1.0);
        t.record("c1", "b", 2.0);
        t.record("c2", "c", 3.0);
        t.load("c1", false);
        let first = t.h.list("c1").unwrap()[1].id.clone();
        t.h.delete_entries("c1", std::slice::from_ref(&first));
        t.delete_on_disk("c1", &[first]);
        assert_eq!(t.sqls("c1").unwrap(), ["b"]);
        assert_eq!(t.disk.sqls("c1"), ["b"]);
        t.h.clear("c1");
        t.disk.0.remove("c1");
        assert_eq!(t.sqls("c1").unwrap(), Vec::<String>::new());
        assert!(!t.disk.0.contains_key("c1"));
        assert_eq!(t.disk.0["c2"].len(), 1);
    }

    // Beyond the TS cases.

    #[test]
    fn one_toast_per_failure_streak_and_read_errors() {
        let mut h = QueryHistory::new();
        let err = CmdError::msg("disk full");
        let t = h.append_done(Err(&err)).unwrap();
        assert_eq!(
            (t.title.as_str(), t.body.as_deref()),
            (SAVE_FAILED, Some("disk full"))
        );
        assert_eq!(h.append_done(Err(&err)), None);
        assert_eq!(h.append_done(Ok(())), None);
        assert!(h.append_done(Err(&err)).is_some());
        assert!(h.begin_load("c1", false));
        let out = h.finish_load("c1", Err(&err), 30, T0);
        assert_eq!(out.toast.unwrap().title, READ_FAILED);
        assert_eq!(h.list("c1"), None);
        assert!(h.begin_load("c1", false), "a failed read can be retried");
    }

    #[test]
    fn merges_all_connections_and_forgets_one() {
        let mut t = Harness::new();
        t.record("c1", "a", 1.0);
        t.record("c2", "b", 2.0);
        t.record("c1", "c", 3.0);
        t.load("c1", false);
        t.load("c2", false);
        let all = t.h.items(&["c1".into(), "c2".into(), "c3".into()]);
        let got: Vec<(&str, &str)> = all
            .iter()
            .map(|i| (i.connection_id.as_str(), i.entry.sql.as_str()))
            .collect();
        assert_eq!(got, [("c1", "c"), ("c2", "b"), ("c1", "a")]);
        let hits = filter_history(
            &all,
            &HistoryFilter {
                query: "B".into(),
                errors_only: false,
            },
        );
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].connection_id, "c2");
        assert_eq!(t.h.days_changed(30, 30), Vec::<String>::new());
        assert_eq!(t.h.days_changed(30, 7), ["c1", "c2"]);
        t.h.forget("c1");
        assert_eq!(t.h.list("c1"), None);
        t.h.toggle();
        assert!(t.h.open);
    }
}
