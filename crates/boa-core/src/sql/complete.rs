//! Context-aware completion for the SQL console. No parser: a small lexer
//! with offsets, then a look backwards from the caret within the current
//! statement decides what fits there:
//!
//! ```text
//!   FROM / JOIN / UPDATE / INTO …   tables, views, CTEs, schemas
//!   after a table reference         JOIN / WHERE / AS … keywords
//!   SELECT / WHERE / ON / SET / BY  columns of the tables in the statement,
//!                                   their aliases, functions, keywords
//!   JOIN t ON                       first, conditions from foreign keys
//!                                   between t and the tables before it
//!   after an expression             AND / FROM / ORDER BY … keywords
//!   alias. / table. / schema.       that table's columns, that schema's tables
//!   INSERT INTO t (                 t's columns
//!   :: / CAST(x AS                  type names
//! ```
//!
//! Tables in scope are read from the whole statement, so `SELECT | FROM t`
//! already knows t's columns. Kept free of the editor so it can be tested
//! and reused. All offsets are bytes.

use std::borrow::Cow;
use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};

use super::chars::{char_at, char_before, floor_boundary, is_ident, is_ident_start, is_space};
use super::tokenize::{TYPES, dollar_tag, is_keyword, qualified, quote_ident};
use crate::model::{ConstraintKind, Schema, Table, TableKind};

/// What a completion item is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CompletionKind {
    Keyword,
    Schema,
    Table,
    View,
    Cte,
    Column,
    Alias,
    Function,
    Type,
    /// A JOIN … ON condition from a foreign key.
    Join,
}

impl CompletionKind {
    /// Lower-case name (`keyword`, `table`, …), as in the TypeScript original.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Keyword => "keyword",
            Self::Schema => "schema",
            Self::Table => "table",
            Self::View => "view",
            Self::Cte => "cte",
            Self::Column => "column",
            Self::Alias => "alias",
            Self::Function => "function",
            Self::Type => "type",
            Self::Join => "join",
        }
    }
}

/// A completion item.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Completion {
    pub label: String,
    pub kind: CompletionKind,
    /// Text that replaces the word at the caret.
    pub insert: String,
    /// Caret byte offset into `insert` after inserting (None: its end).
    pub caret: Option<usize>,
    /// Short trailing hint: a column's type and table, a table's schema.
    pub detail: Option<String>,
    /// Primary-key column.
    pub pk: bool,
}

/// Completions at a caret and the word they replace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompletionResult {
    /// Start of the word being completed (an opening `"` included).
    pub from: usize,
    /// End of the word at the caret, for replacing the whole word.
    pub to: usize,
    /// What was typed of the word, up to the caret.
    pub prefix: String,
    /// Best first.
    pub items: Vec<Completion>,
}

/// Options for [`complete`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompleteOptions {
    /// Ranked first among schemas (the console's default schema).
    pub default_schema: Option<String>,
    /// Schemas whose tables need no qualifier (Postgres search_path).
    pub search_path: Vec<String>,
    /// Invoked by the user (Ctrl+Space): show items for an empty prefix too.
    pub explicit: bool,
    /// At most this many items.
    pub limit: usize,
}

impl Default for CompleteOptions {
    fn default() -> Self {
        Self {
            default_schema: None,
            search_path: vec!["public".to_owned()],
            explicit: false,
            limit: 200,
        }
    }
}

// --- lexer ------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TokType {
    Word,
    Ident,
    String,
    Comment,
    Number,
    Punct,
    Other,
}

#[derive(Debug, Clone)]
struct Tok<'a> {
    ty: TokType,
    start: usize,
    end: usize,
    /// Lower-cased word, unquoted identifier, or the punctuation character.
    name: Cow<'a, str>,
    /// Unterminated, or a line comment: a caret at `end` is still inside.
    open: bool,
}

fn lex(sql: &str) -> Vec<Tok<'_>> {
    let b = sql.as_bytes();
    let n = b.len();
    let at = |k: usize| b.get(k).copied().unwrap_or(0);
    let mut out = Vec::new();
    let add = |out: &mut Vec<Tok<'_>>, ty, start, end, open| {
        out.push(Tok {
            ty,
            start,
            end,
            name: Cow::Borrowed(""),
            open,
        });
    };
    let mut i = 0;
    while i < n {
        let c = b[i];
        let next = at(i + 1);
        let ch = char_at(sql, i).unwrap_or('\0');
        if is_space(ch) {
            i += ch.len_utf8();
            continue;
        }
        if c == b'-' && next == b'-' {
            let j = b[i..].iter().position(|&x| x == b'\n').map_or(n, |p| i + p);
            add(&mut out, TokType::Comment, i, j, true);
            i = j;
            continue;
        }
        if c == b'/' && next == b'*' {
            let e = sql[i + 2..].find("*/");
            let j = e.map_or(n, |e| i + 2 + e + 2);
            add(&mut out, TokType::Comment, i, j, e.is_none());
            i = j;
            continue;
        }
        if c == b'\'' || (matches!(c, b'e' | b'E' | b'n' | b'N') && next == b'\'') {
            let mut j = if c == b'\'' { i + 1 } else { i + 2 };
            let mut closed = false;
            while j < n {
                if b[j] == b'\'' {
                    if at(j + 1) == b'\'' {
                        j += 2;
                    } else {
                        j += 1;
                        closed = true;
                        break;
                    }
                } else {
                    j += 1;
                }
            }
            add(&mut out, TokType::String, i, j, !closed);
            i = j;
            continue;
        }
        if c == b'$'
            && let Some(tag_end) = dollar_tag(b, i).filter(|&e| e - i <= 64)
        {
            let tag = &sql[i..tag_end];
            let e = sql[tag_end..].find(tag);
            let j = e.map_or(n, |e| tag_end + e + tag.len());
            add(&mut out, TokType::String, i, j, e.is_none());
            i = j;
            continue;
        }
        if c == b'"' || c == b'`' {
            let mut j = i + 1;
            let mut closed = false;
            while j < n {
                if b[j] == c {
                    if at(j + 1) == c {
                        j += 2;
                    } else {
                        j += 1;
                        closed = true;
                        break;
                    }
                } else {
                    j += 1;
                }
            }
            let body = &sql[i + 1..if closed { j - 1 } else { j }];
            let q = c as char;
            let name = body
                .replace(&format!("{q}{q}"), &q.to_string())
                .to_lowercase();
            out.push(Tok {
                ty: TokType::Ident,
                start: i,
                end: j,
                name: Cow::Owned(name),
                open: !closed,
            });
            i = j;
            continue;
        }
        if c.is_ascii_digit() {
            let mut j = i + 1;
            while at(j).is_ascii_alphanumeric() || at(j) == b'_' || at(j) == b'.' {
                j += 1;
            }
            add(&mut out, TokType::Number, i, j, false);
            i = j;
            continue;
        }
        if is_ident_start(ch) {
            let mut j = i + ch.len_utf8();
            while let Some(d) = char_at(sql, j).filter(|d| is_ident(*d)) {
                j += d.len_utf8();
            }
            let word = &sql[i..j];
            let name = if word.bytes().any(|x| !x.is_ascii_lowercase()) {
                Cow::Owned(word.to_lowercase())
            } else {
                Cow::Borrowed(word)
            };
            out.push(Tok {
                ty: TokType::Word,
                start: i,
                end: j,
                name,
                open: false,
            });
            i = j;
            continue;
        }
        if matches!(c, b'$' | b':' | b'@') {
            // $1, :name, @name parameters (at most 64 bytes); `::` stays punctuation.
            let mut j = i + 1;
            if c == b'$' {
                while at(j).is_ascii_digit() {
                    j += 1;
                }
            } else if at(j).is_ascii_alphabetic() || at(j) == b'_' {
                j += 1;
                while at(j).is_ascii_alphanumeric() || at(j) == b'_' {
                    j += 1;
                }
            }
            let j = j.min(i + 64);
            let cast = c == b':' && (next == b':' || (i > 0 && b[i - 1] == b':'));
            if j > i + 1 && !cast {
                add(&mut out, TokType::Other, i, j, false);
                i = j;
                continue;
            }
        }
        let j = i + ch.len_utf8();
        out.push(Tok {
            ty: TokType::Punct,
            start: i,
            end: j,
            name: Cow::Borrowed(&sql[i..j]),
            open: false,
        });
        i = j;
    }
    out
}

fn is_name(t: Option<&Tok<'_>>) -> bool {
    t.is_some_and(|t| t.ty == TokType::Ident || (t.ty == TokType::Word && !is_keyword(&t.name)))
}
fn is_punct(t: Option<&Tok<'_>>, ch: &str) -> bool {
    t.is_some_and(|t| t.ty == TokType::Punct && t.name == ch)
}
fn is_word(t: Option<&Tok<'_>>, ws: &[&str]) -> bool {
    t.is_some_and(|t| t.ty == TokType::Word && ws.contains(&t.name.as_ref()))
}
fn is_name_or_word(t: Option<&Tok<'_>>) -> bool {
    t.is_some_and(|t| matches!(t.ty, TokType::Ident | TokType::Word))
}
/// `toks[k]` for a possibly negative `k`.
fn get<'t, 'a>(toks: &'t [Tok<'a>], k: isize) -> Option<&'t Tok<'a>> {
    usize::try_from(k).ok().and_then(|k| toks.get(k))
}

// --- vocabulary -------------------------------------------------------------

const START: &[&str] = &[
    "select",
    "insert into",
    "update",
    "delete from",
    "with",
    "explain",
    "explain analyze",
    "create table",
    "create index",
    "create view",
    "alter table",
    "drop table",
    "truncate",
    "begin",
    "commit",
    "rollback",
    "values",
    "show",
    "set",
    "vacuum",
    "analyze",
    "table",
];

const AFTER_TABLE: &[&str] = &[
    "as",
    "join",
    "inner join",
    "left join",
    "right join",
    "full join",
    "cross join",
    "natural join",
    "on",
    "using",
    "where",
    "group by",
    "order by",
    "having",
    "limit",
    "offset",
    "union",
    "union all",
    "set",
    "values",
    "returning",
    "for update",
];

const AFTER_EXPR: &[&str] = &[
    "and",
    "or",
    "not",
    "as",
    "from",
    "where",
    "join",
    "inner join",
    "left join",
    "on",
    "group by",
    "order by",
    "having",
    "limit",
    "offset",
    "asc",
    "desc",
    "nulls first",
    "nulls last",
    "is null",
    "is not null",
    "in",
    "not in",
    "like",
    "ilike",
    "between",
    "union",
    "union all",
    "except",
    "intersect",
    "returning",
    "then",
    "else",
    "end",
    "when",
    "over",
    "filter",
];

const EXPR: &[&str] = &[
    "distinct", "case", "when", "then", "else", "end", "not", "exists", "null", "true", "false",
    "select", "interval", "and", "or",
];

const FUNCTIONS: &[&str] = &[
    "count",
    "sum",
    "avg",
    "min",
    "max",
    "coalesce",
    "nullif",
    "greatest",
    "least",
    "now",
    "lower",
    "upper",
    "length",
    "trim",
    "substring",
    "replace",
    "concat",
    "date_trunc",
    "extract",
    "to_char",
    "to_date",
    "to_timestamp",
    "round",
    "floor",
    "ceil",
    "abs",
    "string_agg",
    "array_agg",
    "json_agg",
    "jsonb_agg",
    "json_build_object",
    "jsonb_build_object",
    "row_number",
    "rank",
    "dense_rank",
    "lag",
    "lead",
    "first_value",
    "last_value",
    "generate_series",
    "gen_random_uuid",
    "age",
    "date_part",
];

/// Keywords that open a clause of table references.
const TABLE_CLAUSE: &[&str] = &["from", "join", "update", "into", "table", "truncate"];
/// Keywords that open a clause of expressions.
const COLUMN_CLAUSE: &[&str] = &[
    "select",
    "where",
    "on",
    "and",
    "or",
    "by",
    "having",
    "set",
    "returning",
    "when",
    "then",
    "else",
    "case",
    "not",
    "between",
    "like",
    "ilike",
    "distinct",
    "in",
    "is",
    "using",
    "over",
    "partition",
];
const NO_COMPLETION: &[&str] = &["as", "limit", "offset", "with", "values", "fetch"];
/// Keywords that end an expression, after which keywords (not columns) follow.
const EXPR_END: &[&str] = &[
    "null", "true", "false", "asc", "desc", "end", "first", "last",
];

// --- scope ------------------------------------------------------------------

#[derive(Debug, Clone)]
struct Ref<'s> {
    /// Table name as written (lower-cased).
    name: String,
    alias: Option<String>,
    schema: Option<&'s str>,
    table: Option<&'s Table>,
    /// INSERT INTO target.
    insert: bool,
    /// Index (in the statement's tokens) of the last token of the reference; -1: none.
    end_tok: isize,
}

struct Catalog<'s> {
    schemas: &'s [Schema],
    search_path: &'s [String],
    default_schema: Option<&'s str>,
}

fn find_table<'s>(cat: &Catalog<'s>, parts: &[String]) -> Option<(&'s str, &'s Table)> {
    let name = parts.last()?;
    let look = |s: &'s Schema| {
        s.tables
            .iter()
            .find(|t| t.name.to_lowercase() == *name)
            .map(|t| (s.name.as_str(), t))
    };
    if parts.len() >= 2 {
        let want = &parts[parts.len() - 2];
        return cat
            .schemas
            .iter()
            .find(|x| x.name.to_lowercase() == *want)
            .and_then(look);
    }
    let order = cat
        .search_path
        .iter()
        .map(String::as_str)
        .chain([cat.default_schema.unwrap_or("")]);
    for name in order {
        if let Some(hit) = cat.schemas.iter().find(|x| x.name == name).and_then(look) {
            return Some(hit);
        }
    }
    cat.schemas.iter().find_map(look)
}

/// Tables (with aliases) and CTE names the statement refers to, anywhere in it.
fn scope<'s>(stmt: &[Tok<'_>], cat: &Catalog<'s>) -> (Vec<Ref<'s>>, Vec<String>) {
    let mut refs = Vec::new();
    let mut ctes: Vec<String> = Vec::new();
    for k in 0..stmt.len() {
        let t = &stmt[k];
        let ki = k as isize;
        // name AS ( … ) after WITH / RECURSIVE / a comma
        if is_name(Some(t))
            && is_word(stmt.get(k + 1), &["as"])
            && is_punct(stmt.get(k + 2), "(")
            && (is_word(get(stmt, ki - 1), &["with", "recursive"])
                || is_punct(get(stmt, ki - 1), ","))
        {
            ctes.push(t.name.to_string());
            continue;
        }
        if t.ty != TokType::Word || !TABLE_CLAUSE.contains(&t.name.as_ref()) {
            continue;
        }
        let mut j = k + 1;
        loop {
            if is_word(stmt.get(j), &["only"]) {
                j += 1;
            }
            if !is_name(stmt.get(j)) {
                break;
            }
            let mut parts = vec![stmt[j].name.to_string()];
            while is_punct(stmt.get(j + 1), ".") && is_name(stmt.get(j + 2)) {
                parts.push(stmt[j + 2].name.to_string());
                j += 2;
            }
            let mut end_tok = j as isize;
            j += 1;
            let mut alias = None;
            if is_word(stmt.get(j), &["as"]) {
                j += 1;
            }
            if is_name(stmt.get(j))
                && !is_punct(stmt.get(j + 1), ".")
                && !is_punct(stmt.get(j + 1), "(")
            {
                alias = Some(stmt[j].name.to_string());
                end_tok = j as isize;
                j += 1;
            }
            let hit = if parts.len() == 1 && ctes.contains(&parts[0]) {
                None
            } else {
                find_table(cat, &parts)
            };
            refs.push(Ref {
                name: parts.pop().unwrap_or_default(),
                alias,
                schema: hit.map(|h| h.0),
                table: hit.map(|h| h.1),
                insert: t.name == "into",
                end_tok,
            });
            if t.name == "from" && is_punct(stmt.get(j), ",") {
                j += 1;
            } else {
                break;
            }
        }
    }
    (refs, ctes)
}

// --- matching ---------------------------------------------------------------

/// How well `label` matches what was typed: 0 none, 1 nothing typed, 2
/// substring (3+ characters), 3 word starts (`crat` → `created_at`), 4
/// prefix, 5 exact. Case-insensitive; `u.created_at` also matches as `created_at`.
pub fn match_tier(label: &str, typed: &str) -> u8 {
    if typed.is_empty() {
        return 1;
    }
    match label.rfind('.') {
        Some(dot) if dot > 0 => tier(label, typed).max(tier(&label[dot + 1..], typed)),
        _ => tier(label, typed),
    }
}

fn tier(label: &str, typed: &str) -> u8 {
    let l = label.to_lowercase();
    let q = typed.to_lowercase();
    if l == q {
        return 5;
    }
    if l.starts_with(&q) {
        return 4;
    }
    let segs: Vec<Vec<char>> = l
        .split(|c: char| c == '_' || c == '.' || is_space(c))
        .filter(|s| !s.is_empty())
        .map(|s| s.chars().collect())
        .collect();
    let qc: Vec<char> = q.chars().collect();
    if segments_match(&segs, &qc) {
        return 3;
    }
    // A substring anywhere is noise for one or two letters.
    if qc.len() >= 3 && l.contains(&q) {
        return 2;
    }
    0
}

/// `uid` → user_id, `crat` → created_at: each piece is a prefix of a later segment.
fn segments_match(segs: &[Vec<char>], q: &[char]) -> bool {
    fn go(segs: &[Vec<char>], q: &[char], si: usize, qi: usize) -> bool {
        if qi == q.len() {
            return true;
        }
        if si == segs.len() {
            return false;
        }
        let seg = &segs[si];
        for len in (1..=seg.len().min(q.len() - qi)).rev() {
            if seg.starts_with(&q[qi..qi + len]) && go(segs, q, si + 1, qi + len) {
                return true;
            }
        }
        si > 0 && go(segs, q, si + 1, qi)
    }
    q.len() >= 2 && segs.len() >= 2 && go(segs, q, 0, 0)
}

// --- complete ---------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Start,
    Table,
    AfterTable,
    Column,
    AfterExpr,
    /// INSERT INTO t ( …: the index of t's ref.
    InsertColumns(usize),
    Type,
}

struct Items {
    items: Vec<(Completion, f64)>,
}

impl Items {
    fn add(&mut self, c: Completion, priority: f64) {
        self.items.push((c, priority));
    }

    fn simple(
        &mut self,
        label: &str,
        kind: CompletionKind,
        insert: String,
        detail: Option<String>,
        priority: f64,
    ) {
        self.add(
            Completion {
                label: label.to_owned(),
                kind,
                insert,
                caret: None,
                detail,
                pk: false,
            },
            priority,
        );
    }

    fn keyword(&mut self, k: &str, upper: bool, priority: f64) {
        let insert = if upper {
            k.to_uppercase()
        } else {
            k.to_owned()
        };
        self.simple(k, CompletionKind::Keyword, insert, None, priority);
    }

    /// A table's columns, detailed with `shown` (its alias or name).
    fn columns(&mut self, table: Option<&Table>, shown: &str, priority: f64) {
        for c in table.map_or(&[][..], |t| &t.columns) {
            self.add(
                Completion {
                    label: c.name.clone(),
                    kind: CompletionKind::Column,
                    insert: quote_ident(&c.name),
                    caret: None,
                    detail: Some(format!("{} · {shown}", c.ty)),
                    pk: c.pk,
                },
                priority,
            );
        }
    }

    fn table(&mut self, schema: &str, t: &Table, priority: f64, qualify: bool) {
        let kind = if t.kind == TableKind::View {
            CompletionKind::View
        } else {
            CompletionKind::Table
        };
        let insert = if qualify {
            qualified(schema, &t.name)
        } else {
            quote_ident(&t.name)
        };
        self.simple(&t.name, kind, insert, Some(schema.to_owned()), priority);
    }

    fn tables(&mut self, cat: &Catalog<'_>, priority: f64) {
        for s in cat.schemas {
            let on_path = cat.search_path.contains(&s.name);
            let bonus = if Some(s.name.as_str()) == cat.default_schema {
                0.5
            } else if on_path {
                0.25
            } else {
                0.0
            };
            for t in &s.tables {
                self.table(&s.name, t, priority + bonus, !on_path);
            }
        }
    }
}

/// Completions at byte `pos` of `sql` against the catalog `schemas`, or None
/// when nothing fits (inside a string, a comment, a number, an alias name, or
/// an empty prefix without [`CompleteOptions::explicit`]).
pub fn complete(
    sql: &str,
    pos: usize,
    schemas: &[Schema],
    opts: &CompleteOptions,
) -> Option<CompletionResult> {
    let pos = floor_boundary(sql, pos);
    let cat = Catalog {
        schemas,
        search_path: &opts.search_path,
        default_schema: opts.default_schema.as_deref(),
    };
    let toks = lex(sql);

    // The word at the caret, possibly an open "quoted identifier.
    let mut from = pos;
    let mut to = pos;
    let prefix: &str;
    let inside = toks
        .iter()
        .find(|t| pos > t.start && (pos < t.end || (pos == t.end && t.open)));
    match inside.map(|t| t.ty) {
        Some(TokType::String | TokType::Comment | TokType::Other | TokType::Number) => return None,
        Some(TokType::Ident) => {
            let t = inside?;
            from = t.start;
            to = t.end;
            prefix = &sql[t.start + 1..pos];
        }
        _ => {
            while let Some(c) = char_before(sql, from).filter(|c| is_ident(*c)) {
                from -= c.len_utf8();
            }
            while let Some(c) = char_at(sql, to).filter(|c| is_ident(*c)) {
                to += c.len_utf8();
            }
            prefix = &sql[from..pos];
            if prefix.starts_with(|c: char| c.is_ascii_digit()) {
                return None;
            }
        }
    }
    let quoted = inside.is_some_and(|t| t.ty == TokType::Ident);

    // Current statement, comments dropped.
    let mut lo = 0;
    let mut hi = toks.len();
    for (i, t) in toks.iter().enumerate() {
        if !is_punct(Some(t), ";") {
            continue;
        }
        if t.end <= from {
            lo = i + 1;
        } else if t.start >= pos && hi == toks.len() {
            hi = i;
        }
    }
    let stmt: Vec<Tok<'_>> = toks[lo..hi.max(lo)]
        .iter()
        .filter(|t| t.ty != TokType::Comment)
        .cloned()
        .collect();
    let pre = &stmt[..stmt.iter().take_while(|t| t.end <= from).count()];
    let (refs, ctes) = scope(&stmt, &cat);
    let last = pre.last();

    let mut items = Items { items: Vec::new() };

    // --- qualified: alias. / table. / schema. / schema.table.
    if !quoted && is_punct(last, ".") && last.is_some_and(|l| l.end == from) {
        let mut parts: Vec<String> = Vec::new();
        let mut k = pre.len() as isize - 2;
        while is_name_or_word(get(pre, k)) {
            let t = &pre[k as usize];
            parts.insert(0, t.name.to_string());
            if is_punct(get(pre, k - 1), ".") && get(pre, k - 1).is_some_and(|d| d.end == t.start) {
                k -= 2;
            } else {
                break;
            }
        }
        if parts.is_empty() {
            return None;
        }
        if let [p] = parts.as_slice() {
            let r = refs
                .iter()
                .find(|r| r.alias.as_ref() == Some(p))
                .or_else(|| refs.iter().find(|r| r.alias.is_none() && r.name == *p));
            let direct = match r {
                Some(r) if r.table.is_some() => r.table,
                _ => find_table(&cat, &parts).map(|h| h.1),
            };
            if direct.is_some() {
                items.columns(direct, p, 5.0);
            } else if let Some(s) = schemas.iter().find(|s| s.name.to_lowercase() == *p) {
                for t in &s.tables {
                    items.table(&s.name, t, 4.0, false);
                }
            }
        } else if let Some((_, t)) = find_table(&cat, &parts[parts.len() - 2..]) {
            items.columns(Some(t), &t.name, 5.0);
        }
        return finish(items, from, to, prefix, opts, true);
    }

    let mode = mode_at(pre, &refs, opts.explicit)?;
    let upper = upper_keywords(sql, pre, prefix);

    match mode {
        Mode::Start => START.iter().for_each(|k| items.keyword(k, upper, 1.0)),
        Mode::Table => {
            for c in &ctes {
                items.simple(
                    c,
                    CompletionKind::Cte,
                    quote_ident(c),
                    Some("cte".into()),
                    5.0,
                );
            }
            items.tables(&cat, 4.0);
            for s in schemas {
                items.simple(
                    &s.name,
                    CompletionKind::Schema,
                    format!("{}.", quote_ident(&s.name)),
                    None,
                    2.0,
                );
            }
        }
        Mode::AfterTable => AFTER_TABLE
            .iter()
            .for_each(|k| items.keyword(k, upper, 1.0)),
        Mode::AfterExpr => AFTER_EXPR.iter().for_each(|k| items.keyword(k, upper, 1.0)),
        Mode::InsertColumns(r) => {
            let r = &refs[r];
            items.columns(r.table, r.alias.as_ref().unwrap_or(&r.name), 5.0);
        }
        Mode::Type => {
            for t in TYPES {
                items.simple(t, CompletionKind::Type, (*t).to_owned(), None, 3.0);
            }
        }
        Mode::Column => {
            // JOIN t ON |: the foreign keys between t and the tables before it, first.
            if is_word(last, &["on"]) {
                for cond in join_conditions(&refs, pre.len() as isize - 1, upper) {
                    items.simple(
                        &cond,
                        CompletionKind::Join,
                        cond.clone(),
                        Some("foreign key".into()),
                        6.0,
                    );
                }
            }
            // Columns of every table in the statement. A name two tables share
            // is inserted qualified, so the query stays unambiguous.
            let resolved: Vec<(&Ref<'_>, &Table)> =
                refs.iter().filter_map(|r| Some((r, r.table?))).collect();
            let mut count: HashMap<&str, usize> = HashMap::new();
            for (_, t) in &resolved {
                for c in &t.columns {
                    *count.entry(c.name.as_str()).or_default() += 1;
                }
            }
            for (r, t) in &resolved {
                let shown = r.alias.as_ref().unwrap_or(&r.name);
                for c in &t.columns {
                    let shared = count.get(c.name.as_str()).copied().unwrap_or(0) > 1;
                    items.add(
                        Completion {
                            label: if shared {
                                format!("{shown}.{}", c.name)
                            } else {
                                c.name.clone()
                            },
                            kind: CompletionKind::Column,
                            insert: if shared {
                                format!("{}.{}", quote_ident(shown), quote_ident(&c.name))
                            } else {
                                quote_ident(&c.name)
                            },
                            caret: None,
                            detail: Some(format!("{} · {shown}", c.ty)),
                            pk: c.pk,
                        },
                        5.0,
                    );
                }
            }
            for r in &refs {
                if let Some(a) = &r.alias {
                    items.simple(
                        a,
                        CompletionKind::Alias,
                        a.clone(),
                        Some(r.name.clone()),
                        4.0,
                    );
                }
            }
            // Nothing in scope yet (SELECT typed before FROM): offer every
            // column on the search path once something is typed.
            if resolved.is_empty() && !prefix.is_empty() {
                for s in schemas {
                    if !cat.search_path.contains(&s.name)
                        && Some(s.name.as_str()) != cat.default_schema
                    {
                        continue;
                    }
                    for t in &s.tables {
                        items.columns(Some(t), &t.name, 3.5);
                    }
                }
            }
            for f in FUNCTIONS {
                items.add(
                    Completion {
                        label: (*f).to_owned(),
                        kind: CompletionKind::Function,
                        insert: format!("{f}()"),
                        caret: Some(f.len() + 1),
                        detail: None,
                        pk: false,
                    },
                    3.0,
                );
            }
            EXPR.iter().for_each(|k| items.keyword(k, upper, 2.0));
            if !prefix.is_empty() {
                items.tables(&cat, 1.0);
            }
        }
    }
    finish(items, from, to, prefix, opts, false)
}

/// `a.x = b.y` conditions for `JOIN t ON`, from the foreign keys either way
/// between t (the reference ending just before ON, token `on_tok`) and each
/// table referenced before it; the joined table's side comes first.
fn join_conditions(refs: &[Ref<'_>], on_tok: isize, upper: bool) -> Vec<String> {
    let Some(ji) = refs.iter().position(|r| r.end_tok == on_tok - 1) else {
        return Vec::new();
    };
    let joined = &refs[ji];
    if joined.table.is_none() {
        return Vec::new();
    }
    let q = |r: &Ref<'_>| quote_ident(r.alias.as_ref().unwrap_or(&r.name));
    let and = if upper { " AND " } else { " and " };
    let fks = |from: &Ref<'_>, to: &Ref<'_>| -> Vec<(Vec<String>, Vec<String>)> {
        from.table
            .map_or(&[][..], |t| &t.constraints)
            .iter()
            .filter_map(|c| {
                let r = c.references.as_ref()?;
                (c.kind == ConstraintKind::ForeignKey
                    && Some(r.table.as_str()) == to.table.map(|t| t.name.as_str())
                    && Some(r.schema.as_str()) == to.schema
                    && r.columns.len() == c.columns.len())
                .then(|| (c.columns.clone(), r.columns.clone()))
            })
            .collect()
    };
    let cond = |a: &Ref<'_>, a_cols: &[String], b: &Ref<'_>, b_cols: &[String]| {
        a_cols
            .iter()
            .zip(b_cols)
            .map(|(x, y)| format!("{}.{} = {}.{}", q(a), quote_ident(x), q(b), quote_ident(y)))
            .collect::<Vec<_>>()
            .join(and)
    };
    let mut out = Vec::new();
    for (oi, other) in refs.iter().enumerate() {
        if oi == ji || other.table.is_none() || other.insert || other.end_tok >= joined.end_tok {
            continue;
        }
        for (cols, ref_cols) in fks(joined, other) {
            out.push(cond(joined, &cols, other, &ref_cols));
        }
        for (cols, ref_cols) in fks(other, joined) {
            out.push(cond(joined, &ref_cols, other, &cols));
        }
    }
    out
}

/// What belongs at the caret, from the clause it is in.
fn mode_at(pre: &[Tok<'_>], refs: &[Ref<'_>], explicit: bool) -> Option<Mode> {
    let n = pre.len() as isize;
    let last = pre.last();
    if is_punct(last, ":") && is_punct(get(pre, n - 2), ":") {
        return Some(Mode::Type);
    }

    // Walk back to the keyword opening this clause, skipping (…) groups; an
    // unmatched `(` means the caret is inside parentheses.
    let mut depth = 0;
    let mut clause: Option<usize> = None;
    let mut paren: isize = -1;
    for k in (0..pre.len()).rev() {
        let t = &pre[k];
        if is_punct(Some(t), ")") {
            depth += 1;
        } else if is_punct(Some(t), "(") {
            if depth == 0 {
                paren = k as isize;
                break;
            }
            depth -= 1;
        } else if depth == 0 && t.ty == TokType::Word {
            let w = t.name.as_ref();
            if TABLE_CLAUSE.contains(&w) || COLUMN_CLAUSE.contains(&w) || NO_COMPLETION.contains(&w)
            {
                clause = Some(k);
                break;
            }
            if w == "explain" {
                break;
            }
        }
    }

    let Some(ci) = clause else {
        if paren < 0 {
            return Some(Mode::Start);
        }
        let r = refs.iter().position(|r| r.insert && r.end_tok == paren - 1);
        if let Some(r) = r
            && (refs[r].table.is_some() || is_punct(last, ","))
        {
            return Some(Mode::InsertColumns(r));
        }
        return Some(if after_expr(pre) {
            Mode::AfterExpr
        } else {
            Mode::Column
        });
    };
    let clause_name = pre[ci].name.as_ref();
    let last_is_clause = ci + 1 == pre.len();
    if clause_name == "as" {
        // CAST(x AS |): a type. Otherwise an alias is being named.
        let open = enclosing_paren(pre, ci);
        if open > 0 && is_word(get(pre, open - 1), &["cast"]) {
            return Some(Mode::Type);
        }
        return (explicit && !last_is_clause).then_some(Mode::AfterExpr);
    }
    if NO_COMPLETION.contains(&clause_name) {
        return None;
    }
    if TABLE_CLAUSE.contains(&clause_name) {
        return Some(
            if last_is_clause || is_punct(last, ",") || is_word(last, &["only"]) {
                Mode::Table
            } else {
                Mode::AfterTable
            },
        );
    }
    if !last_is_clause && after_expr(pre) {
        return Some(Mode::AfterExpr);
    }
    Some(Mode::Column)
}

/// Index of the unmatched `(` before index `k`, or -1.
fn enclosing_paren(pre: &[Tok<'_>], k: usize) -> isize {
    let mut depth = 0;
    for i in (0..k).rev() {
        if is_punct(pre.get(i), ")") {
            depth += 1;
        } else if is_punct(pre.get(i), "(") {
            if depth == 0 {
                return i as isize;
            }
            depth -= 1;
        }
    }
    -1
}

/// The previous token ends an expression: `x |`, `'a' |`, `f(…) |`, `select * |`.
fn after_expr(pre: &[Tok<'_>]) -> bool {
    let Some(last) = pre.last() else { return false };
    match last.ty {
        TokType::Ident | TokType::Number | TokType::String | TokType::Other => true,
        TokType::Word => !is_keyword(&last.name) || EXPR_END.contains(&last.name.as_ref()),
        _ if is_punct(Some(last), ")") => true,
        _ if is_punct(Some(last), "*") => {
            let before = get(pre, pre.len() as isize - 2);
            is_word(before, &["select", "distinct"])
                || is_punct(before, ",")
                || is_punct(before, ".")
        }
        _ => false,
    }
}

/// Keywords take the case being typed, else that of the statement's last keyword.
fn upper_keywords(sql: &str, pre: &[Tok<'_>], prefix: &str) -> bool {
    if prefix.bytes().any(|c| c.is_ascii_alphabetic()) {
        return prefix == prefix.to_uppercase();
    }
    let Some(kw) = pre
        .iter()
        .rev()
        .find(|t| t.ty == TokType::Word && is_keyword(&t.name))
    else {
        return false;
    };
    let text = &sql[kw.start..kw.end];
    text == text.to_uppercase()
}

/// Approximates JS `localeCompare` (ICU root collation) for identifiers:
/// punctuation before digits before letters, letters case-insensitively,
/// then lower case before upper case.
fn locale_cmp(a: &str, b: &str) -> Ordering {
    fn key(c: char) -> (u8, char) {
        if c.is_alphabetic() {
            (2, c.to_lowercase().next().unwrap_or(c))
        } else if c.is_numeric() {
            (1, c)
        } else {
            (0, c)
        }
    }
    a.chars()
        .map(key)
        .cmp(b.chars().map(key))
        .then_with(|| {
            a.chars()
                .map(|c| !c.is_lowercase())
                .cmp(b.chars().map(|c| !c.is_lowercase()))
        })
        .then_with(|| a.cmp(b))
}

fn finish(
    items: Items,
    from: usize,
    to: usize,
    prefix: &str,
    opts: &CompleteOptions,
    qualified: bool,
) -> Option<CompletionResult> {
    if prefix.is_empty() && !opts.explicit && !qualified {
        return None;
    }
    let mut seen = HashSet::new();
    let mut ranked: Vec<(usize, u8, Completion, f64)> = items
        .items
        .into_iter()
        .enumerate()
        .filter_map(|(i, (c, priority))| {
            let tier = match_tier(&c.label, prefix);
            if tier == 0 {
                return None;
            }
            let id = (c.kind, c.label.clone(), c.detail.clone());
            seen.insert(id).then_some((i, tier, c, priority))
        })
        .collect();
    // Nothing typed: keep the given order (table definition order, curated
    // keyword lists). Otherwise shorter, then alphabetical.
    ranked.sort_by(|a, b| {
        b.1.cmp(&a.1)
            .then_with(|| b.3.total_cmp(&a.3))
            .then_with(|| {
                if prefix.is_empty() {
                    a.0.cmp(&b.0)
                } else {
                    let len = |c: &Completion| c.label.chars().count();
                    len(&a.2)
                        .cmp(&len(&b.2))
                        .then_with(|| locale_cmp(&a.2.label, &b.2.label))
                }
            })
    });
    let items: Vec<Completion> = ranked
        .into_iter()
        .take(opts.limit)
        .map(|(_, _, c, _)| c)
        .collect();
    let first = items.first()?;
    // Typing a word that is already complete should not pop a list up that
    // swallows the next Enter.
    if !opts.explicit && items.len() == 1 && first.label.to_lowercase() == prefix.to_lowercase() {
        return None;
    }
    Some(CompletionResult {
        from,
        to,
        prefix: prefix.to_owned(),
        items,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Column, Constraint, ForeignRef};

    fn col(name: &str, ty: &str, pk: bool) -> Column {
        Column {
            name: name.into(),
            ty: ty.into(),
            pk,
            nullable: true,
            default: None,
        }
    }
    fn c(name: &str) -> Column {
        col(name, "text", false)
    }
    fn table(name: &str, columns: Vec<Column>) -> Table {
        Table {
            name: name.into(),
            columns,
            ..Table::default()
        }
    }
    fn view(name: &str, columns: Vec<Column>) -> Table {
        Table {
            kind: TableKind::View,
            ..table(name, columns)
        }
    }
    fn schema(name: &str, tables: Vec<Table>) -> Schema {
        Schema {
            name: name.into(),
            tables,
        }
    }

    fn schemas() -> Vec<Schema> {
        vec![
            schema(
                "public",
                vec![
                    table(
                        "users",
                        vec![
                            col("id", "integer", true),
                            c("email"),
                            col("created_at", "timestamptz", false),
                        ],
                    ),
                    table(
                        "orders",
                        vec![
                            col("id", "integer", true),
                            col("user_id", "integer", false),
                            col("total", "numeric", false),
                        ],
                    ),
                    view(
                        "active_users",
                        vec![col("id", "integer", false), c("email")],
                    ),
                ],
            ),
            schema(
                "billing",
                vec![table(
                    "invoices",
                    vec![col("id", "integer", true), c("amount")],
                )],
            ),
        ]
    }

    /// Completes at the `|` in `sql` (a byte offset).
    fn at_in(
        sql: &str,
        explicit: bool,
        cat: &[Schema],
        default_schema: Option<&str>,
    ) -> Option<CompletionResult> {
        let pos = sql.find('|').unwrap();
        let opts = CompleteOptions {
            default_schema: default_schema.map(Into::into),
            explicit,
            ..Default::default()
        };
        complete(&sql.replacen('|', "", 1), pos, cat, &opts)
    }
    fn at(sql: &str, explicit: bool) -> Option<CompletionResult> {
        at_in(sql, explicit, &schemas(), Some("public"))
    }
    fn labels_ex(sql: &str, explicit: bool) -> Vec<String> {
        at(sql, explicit)
            .map(|r| r.items.into_iter().map(|i| i.label).collect())
            .unwrap_or_default()
    }
    fn labels(sql: &str) -> Vec<String> {
        labels_ex(sql, true)
    }
    fn inserts(sql: &str) -> Vec<String> {
        at(sql, true)
            .map(|r| r.items.into_iter().map(|i| i.insert).collect())
            .unwrap_or_default()
    }
    fn has(v: &[String], want: &[&str]) -> bool {
        want.iter().all(|w| v.iter().any(|x| x == w))
    }
    const NONE: [&str; 0] = [];

    #[test]
    fn offers_statement_keywords_at_the_start() {
        assert_eq!(labels("sel|"), ["select"]);
        assert!(has(&labels("|"), &["insert into"]));
    }

    #[test]
    fn offers_tables_after_from_join_qualifying_ones_off_the_search_path() {
        assert_eq!(labels("select * from u|"), ["users"]);
        assert_eq!(labels("select * from users u join o|"), ["orders"]);
        assert_eq!(inserts("select * from inv|"), ["billing.invoices"]);
        assert!(has(
            &labels("select * from |"),
            &[
                "users",
                "orders",
                "active_users",
                "invoices",
                "public",
                "billing"
            ]
        ));
    }

    #[test]
    fn offers_join_where_after_a_table_reference_not_more_tables() {
        assert_eq!(labels("select * from users u w|"), ["where"]);
        assert!(has(&labels("select * from users l|"), &["left join"]));
    }

    #[test]
    fn offers_columns_of_the_tables_in_the_statement() {
        let l = labels("select | from users");
        assert_eq!(l[..3], ["id", "email", "created_at"]);
        assert_eq!(labels("select * from users where em|"), ["email"]);
    }

    #[test]
    fn qualifies_a_column_two_tables_share() {
        let items = at("select * from users u join orders o on |", true)
            .unwrap()
            .items;
        let mut ids: Vec<&str> = items
            .iter()
            .filter(|i| i.kind == CompletionKind::Column && i.label.ends_with(".id"))
            .map(|i| i.insert.as_str())
            .collect();
        ids.sort();
        assert_eq!(ids, ["o.id", "u.id"]);
        assert_eq!(
            items
                .iter()
                .find(|i| i.label == "user_id")
                .map(|i| i.insert.as_str()),
            Some("user_id")
        );
        assert!(has(
            &labels("select * from users u join orders o on u.id = o.user_id where i|"),
            &["u.id", "o.id"]
        ));
    }

    #[test]
    fn offers_aliases_in_expressions() {
        assert!(has(&labels("select * from users usr where us|"), &["usr"]));
    }

    #[test]
    fn completes_alias_table_and_schema_dot() {
        assert_eq!(
            labels_ex("select u.| from users u", false),
            ["id", "email", "created_at"]
        );
        assert_eq!(labels("select users.e| from users")[0], "email");
        assert_eq!(labels_ex("select * from billing.|", false), ["invoices"]);
        assert_eq!(
            labels("select billing.invoices.a| from billing.invoices"),
            ["amount"]
        );
        assert_eq!(labels_ex("select x.| from users u", false), NONE);
    }

    #[test]
    fn offers_keywords_after_an_expression() {
        assert!(has(
            &labels("select * from users where id = 1 o|"),
            &["or", "order by", "offset"]
        ));
        assert_eq!(labels("select * f|")[0], "from");
        assert!(has(&labels("select id f|"), &["from"]));
    }

    #[test]
    fn offers_the_target_columns_inside_insert_into() {
        assert_eq!(labels("insert into orders (|)"), ["id", "user_id", "total"]);
        assert!(has(&labels("insert into orders (id, |)"), &["user_id"]));
    }

    #[test]
    fn offers_columns_in_update_set() {
        assert_eq!(labels("update users set e|")[0], "email");
    }

    #[test]
    fn reads_tables_from_subqueries_and_ctes() {
        assert_eq!(
            labels("with recent as (select * from orders) select * from r|"),
            ["recent"]
        );
        assert_eq!(
            labels("select * from users where id in (select user_id from orders where to|)")[0],
            "total"
        );
    }

    #[test]
    fn offers_types_after_cast() {
        assert!(has(&labels("select id::int|"), &["integer"]));
        assert!(has(
            &labels("select cast(id as tim|)"),
            &["timestamp", "timestamptz"]
        ));
    }

    #[test]
    fn stays_quiet_in_strings_comments_numbers_and_alias_names() {
        assert_eq!(at("select 'fr|", true), None);
        assert_eq!(at("-- fr|", true), None);
        assert_eq!(at("select 12|", true), None);
        assert_eq!(at("select id as e|", false), None);
    }

    #[test]
    fn only_scopes_the_statement_at_the_caret() {
        assert!(!labels("select * from orders; select | from users").contains(&"total".to_owned()));
    }

    #[test]
    fn matches_word_starts_and_keeps_keyword_case() {
        assert!(has(
            &labels("select * from orders where uid|"),
            &["user_id"]
        ));
        assert_eq!(inserts("SELECT * FROM users WH|"), ["WHERE"]);
        assert!(
            inserts("SELECT * FROM users |")
                .iter()
                .all(|s| *s == s.to_uppercase())
        );
    }

    #[test]
    fn needs_a_typed_prefix_unless_invoked_and_hides_an_already_complete_word() {
        assert_eq!(at("select * from |", false), None);
        assert_eq!(
            at("select * from users wher|", false)
                .map(|r| r.items[0].label.clone())
                .as_deref(),
            Some("where")
        );
        assert_eq!(at("select * from users where|", false), None);
    }

    #[test]
    fn replaces_the_whole_word_on_demand() {
        let r = at("select * from us|ers", true).unwrap();
        assert_eq!((r.from, r.to, r.prefix.as_str()), (14, 19, "us"));
    }

    fn fk(cols: &[&str], table: &str, ref_cols: &[&str], schema: &str) -> Constraint {
        let v = |s: &[&str]| s.iter().map(|x| (*x).to_owned()).collect::<Vec<_>>();
        Constraint {
            name: format!("{table}_fkey"),
            kind: ConstraintKind::ForeignKey,
            columns: v(cols),
            definition: String::new(),
            references: Some(ForeignRef {
                schema: schema.into(),
                table: table.into(),
                columns: v(ref_cols),
            }),
        }
    }

    #[test]
    fn offers_join_on_conditions_from_foreign_keys_either_way() {
        let users = table("users", vec![col("id", "integer", true), c("email")]);
        let mut orders = table(
            "orders",
            vec![col("id", "integer", true), col("user_id", "integer", false)],
        );
        orders.constraints = vec![fk(&["user_id"], "users", &["id"], "public")];
        let mut lines = table(
            "lines",
            vec![
                col("order_id", "integer", false),
                col("n", "integer", false),
            ],
        );
        lines.constraints = vec![
            fk(&["order_id"], "orders", &["id"], "public"),
            fk(&["n"], "users", &["id"], "other"),
        ];
        let cat = vec![schema("public", vec![users, orders, lines])];
        let on = |sql: &str| {
            at_in(sql, true, &cat, None)
                .map(|r| r.items)
                .unwrap_or_default()
        };

        let first = &on("select * from users u join orders o on |")[0];
        assert_eq!(
            (first.kind, first.insert.as_str()),
            (CompletionKind::Join, "o.user_id = u.id")
        );
        assert_eq!(
            on("select * from orders join users on |")[0].insert,
            "users.id = orders.user_id"
        );
        // Every earlier table; a key into another schema's `users` doesn't count.
        let joins: Vec<String> =
            on("SELECT * FROM users u JOIN orders o ON o.user_id = u.id JOIN lines l ON |")
                .into_iter()
                .filter(|i| i.kind == CompletionKind::Join)
                .map(|i| i.insert)
                .collect();
        assert_eq!(joins, ["l.order_id = o.id"]);
        // Typing narrows it like any other item (the alias itself matches
        // better); not after ON's first word.
        let narrowed: Vec<String> = on("select * from users u join orders o on o|")
            .into_iter()
            .map(|i| i.insert)
            .collect();
        assert!(has(&narrowed, &["o", "o.user_id = u.id"]));
        assert_eq!(
            on("select * from users u join orders o on user_i|")[0].kind,
            CompletionKind::Column
        );
        assert!(
            !on("select * from users u join orders o on o.id = |")
                .iter()
                .any(|i| i.kind == CompletionKind::Join)
        );
    }

    #[test]
    fn joins_a_composite_foreign_key_with_and() {
        let a = table("a", vec![c("x"), c("y")]);
        let mut b = table("b", vec![c("ax"), c("ay")]);
        b.constraints = vec![fk(&["ax", "ay"], "a", &["x", "y"], "public")];
        let sql = "SELECT * FROM a JOIN b ON ";
        let opts = CompleteOptions {
            explicit: true,
            ..Default::default()
        };
        let items = complete(sql, sql.len(), &[schema("public", vec![a, b])], &opts)
            .unwrap()
            .items;
        assert_eq!(items[0].insert, "b.ax = a.x AND b.ay = a.y");
    }

    #[test]
    fn completes_inside_an_open_quoted_identifier() {
        let r = at("select * from \"us|", true).unwrap();
        assert_eq!(r.from, 14);
        assert_eq!(r.items[0].label, "users");
    }

    #[test]
    fn match_tier_ranks_exact_prefix_word_starts_substring() {
        assert_eq!(match_tier("users", "users"), 5);
        assert_eq!(match_tier("users", "us"), 4);
        assert_eq!(match_tier("created_at", "crat"), 3);
        assert_eq!(match_tier("created_at", "ated"), 2);
        assert_eq!(match_tier("created_at", "zz"), 0);
    }

    // --- Rust-port specific ---------------------------------------------------

    #[test]
    fn uses_byte_offsets_on_non_ascii_text() {
        // `ü` is 2 bytes (1 UTF-16 unit): offsets are bytes.
        let sql = "select 'ü', em| from users";
        let r = at(sql, false).unwrap();
        let em = sql.find("em").unwrap();
        assert_eq!((r.from, r.to, r.prefix.as_str()), (em, em + 2, "em"));
        assert_eq!(r.items[0].label, "email");

        let cat = vec![schema(
            "public",
            vec![table("bücher", vec![c("titel"), c("größe")])],
        )];
        let sql = "select * from bü|";
        let r = at_in(sql, false, &cat, None).unwrap();
        assert_eq!((r.from, r.to, r.prefix.as_str()), (14, 17, "bü"));
        assert_eq!(r.items[0].insert, "\"bücher\"");
        let r = at_in("select grö| from bücher", false, &cat, None).unwrap();
        assert_eq!(r.items[0].label, "größe");
        let r = at_in("select b.| from bücher b", false, &cat, None).unwrap();
        assert_eq!(
            r.items.iter().map(|i| i.label.as_str()).collect::<Vec<_>>(),
            ["titel", "größe"]
        );
        // A caret inside a multibyte char is moved back to its start.
        let sql = "select * from bü";
        let opts = CompleteOptions {
            explicit: true,
            ..Default::default()
        };
        let r = complete(sql, sql.len() - 1, &cat, &opts).unwrap();
        assert_eq!(r.prefix, "b");
    }

    #[test]
    fn function_items_put_the_caret_inside_the_parens() {
        let items = at("select coal|", true).unwrap().items;
        let f = items
            .iter()
            .find(|i| i.kind == CompletionKind::Function)
            .unwrap();
        assert_eq!((f.insert.as_str(), f.caret), ("coalesce()", Some(9)));
    }
}
