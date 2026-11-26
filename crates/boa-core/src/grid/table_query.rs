//! Sort and filter of a table data tab (`tableQuery.ts`, DataGrip's WHERE /
//! ORDER BY bar): the SELECT a tab pages through, built from its free-form
//! clauses, and the ORDER BY text that a column-header click (asc → desc →
//! off) rewrites. Clauses are checked only for what would let them escape
//! their place in the statement (a top-level `;`, unbalanced parentheses,
//! an unterminated string or comment); everything else is left to the
//! server, which reports a bad condition like any other query error.

use super::js::{is_js_space, is_word, js_trim};
use crate::sql::{qualified, quote_ident};

/// A table tab's clauses, as typed (without the WHERE / ORDER BY keyword;
/// empty when unset).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TableQuery {
    pub filter: String,
    pub order_by: String,
}

/// Sort direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SortDir {
    Asc,
    Desc,
}

/// `NULLS FIRST` / `NULLS LAST`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Nulls {
    First,
    Last,
}

impl Nulls {
    fn word(self) -> &'static str {
        match self {
            Self::First => "first",
            Self::Last => "last",
        }
    }
}

struct Scan {
    /// Per char: its parenthesis depth when it is code, -1 inside strings,
    /// quoted names and comments.
    depth: Vec<i32>,
    error: Option<String>,
}

fn is_ident_char(c: Option<&char>) -> bool {
    // `[\w$À-￿]`: astral characters count too (their surrogates are in range).
    c.is_some_and(|&c| is_word(c) || c == '$' || c >= '\u{c0}')
}

fn find_seq(hay: &[char], needle: &[char], from: usize) -> Option<usize> {
    if needle.is_empty() || hay.len() < needle.len() {
        return None;
    }
    (from..=hay.len() - needle.len()).find(|&k| hay[k..k + needle.len()] == *needle)
}

/// `^\$(?:[A-Za-z_]\w*)?\$` at `i`.
fn dollar_tag(s: &[char], i: usize) -> Option<usize> {
    let mut j = i + 1;
    if s.get(j)
        .is_some_and(|c| c.is_ascii_alphabetic() || *c == '_')
    {
        j += 1;
        while s.get(j).is_some_and(|&c| is_word(c)) {
            j += 1;
        }
    }
    (s.get(j) == Some(&'$')).then_some(j + 1 - i)
}

/// Walks `text` the way Postgres's lexer splits it: 'strings' (E'…' with
/// backslash escapes), "quoted names", $tag$ dollar quotes $tag$, -- line
/// and nested block comments, and parentheses.
fn scan(text: &[char]) -> Scan {
    let n = text.len();
    let mut depth = vec![-1; n];
    let fail = |depth, error: &str| Scan {
        depth,
        error: Some(error.to_owned()),
    };
    let mut d: i32 = 0;
    let mut i = 0;
    while i < n {
        let c = text[i];
        let next = text.get(i + 1).copied();
        if c == '-' && next == Some('-') {
            i = (i..n).find(|&k| text[k] == '\n').unwrap_or(n);
            continue;
        }
        if c == '/' && next == Some('*') {
            let mut level = 0;
            let mut j = i;
            while j < n {
                if text[j] == '/' && text.get(j + 1) == Some(&'*') {
                    level += 1;
                    j += 2;
                } else if text[j] == '*' && text.get(j + 1) == Some(&'/') {
                    level -= 1;
                    j += 2;
                    if level == 0 {
                        break;
                    }
                } else {
                    j += 1;
                }
            }
            if level > 0 {
                return fail(depth, "Unterminated /* comment");
            }
            i = j;
            continue;
        }
        let prev = i.checked_sub(1).and_then(|p| text.get(p));
        let e_string = (c == 'E' || c == 'e') && next == Some('\'') && !is_ident_char(prev);
        if c == '\'' || e_string {
            let mut j = if e_string { i + 2 } else { i + 1 };
            let mut closed = false;
            while j < n {
                let ch = text[j];
                if e_string && ch == '\\' {
                    j += 2;
                } else if ch == '\'' {
                    if text.get(j + 1) == Some(&'\'') {
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
            if !closed {
                return fail(depth, "Unterminated string literal");
            }
            i = j;
            continue;
        }
        if c == '"' {
            let mut j = i + 1;
            let mut closed = false;
            while j < n {
                if text[j] == '"' {
                    if text.get(j + 1) == Some(&'"') {
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
            if !closed {
                return fail(depth, "Unterminated quoted identifier");
            }
            i = j;
            continue;
        }
        if c == '$'
            && !is_ident_char(prev)
            && let Some(len) = dollar_tag(text, i)
        {
            let tag = &text[i..i + len];
            match find_seq(text, tag, i + len) {
                Some(end) => {
                    i = end + len;
                    continue;
                }
                None => {
                    let tag: String = tag.iter().collect();
                    return fail(depth, &format!("Unterminated {tag} string"));
                }
            }
        }
        if c == '(' {
            depth[i] = d;
            d += 1;
        } else if c == ')' {
            if d == 0 {
                return fail(depth, "Unbalanced parentheses");
            }
            d -= 1;
            depth[i] = d;
        } else {
            depth[i] = d;
        }
        i += 1;
    }
    Scan {
        depth,
        error: (d != 0).then(|| "Unbalanced parentheses".to_owned()),
    }
}

/// Why `clause` can't go into the generated SELECT as one clause of one
/// statement, or `None` when it can (whether it is valid SQL is the server's call).
pub fn clause_error(clause: &str) -> Option<String> {
    let chars: Vec<char> = clause.chars().collect();
    let Scan { depth, error } = scan(&chars);
    if error.is_some() {
        return error;
    }
    chars
        .iter()
        .zip(&depth)
        .any(|(&c, &d)| c == ';' && d >= 0)
        .then(|| "Only one condition: remove the \";\"".to_owned())
}

/// What [`split_top_level`] / [`top_level_matches`] look for (case-insensitive).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Separator<'a> {
    /// The text itself (`,`).
    Literal(&'a str),
    /// A whole word (`\band\b`).
    Word(&'a str),
}

/// A match of a separator in top-level code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TopLevelMatch {
    /// Byte offset in the text.
    pub index: usize,
    pub text: String,
}

fn match_at(s: &[char], i: usize, sep: Separator<'_>) -> Option<usize> {
    let (pat, word) = match sep {
        Separator::Literal(p) => (p, false),
        Separator::Word(p) => (p, true),
    };
    let pat: Vec<char> = pat.chars().collect();
    let end = i + pat.len();
    if pat.is_empty() || end > s.len() {
        return None;
    }
    let same = s[i..end]
        .iter()
        .zip(&pat)
        .all(|(a, b)| a.to_lowercase().eq(b.to_lowercase()));
    let bounded =
        !word || ((i == 0 || !is_word(s[i - 1])) && s.get(end).is_none_or(|&c| !is_word(c)));
    (same && bounded).then_some(pat.len())
}

/// Matches of `sep` in top-level code of `text` (outside parentheses,
/// strings, quoted names and comments); none when `text` doesn't scan.
pub fn top_level_matches(text: &str, sep: Separator<'_>) -> Vec<TopLevelMatch> {
    let chars: Vec<char> = text.chars().collect();
    let offsets: Vec<usize> = text.char_indices().map(|(b, _)| b).collect();
    let Scan { depth, error } = scan(&chars);
    if error.is_some() {
        return Vec::new();
    }
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        match match_at(&chars, i, sep) {
            Some(len) => {
                if depth[i] == 0 {
                    out.push(TopLevelMatch {
                        index: offsets[i],
                        text: chars[i..i + len].iter().collect(),
                    });
                }
                i += len;
            }
            None => i += 1,
        }
    }
    out
}

/// `text` split at top-level `sep`, parts trimmed.
pub fn split_top_level(text: &str, sep: Separator<'_>) -> Vec<String> {
    let cuts = top_level_matches(text, sep);
    if cuts.is_empty() {
        let t = js_trim(text);
        return if t.is_empty() {
            Vec::new()
        } else {
            vec![t.to_owned()]
        };
    }
    let mut parts = Vec::new();
    let mut from = 0;
    for c in &cuts {
        parts.push(js_trim(&text[from..c.index]).to_owned());
        from = c.index + c.text.len();
    }
    parts.push(js_trim(&text[from..]).to_owned());
    parts
}

/// The clause keyword someone may type out of habit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClauseKeyword {
    Where,
    OrderBy,
}

/// `s` starts with ASCII `word` (case-insensitive); the rest after it.
fn strip_word<'a>(s: &'a str, word: &str) -> Option<&'a str> {
    let head = s.get(..word.len())?;
    head.eq_ignore_ascii_case(word).then(|| &s[word.len()..])
}

/// The clause as typed, minus a leading `where` / `order by`.
pub fn normalize_clause(text: &str, keyword: ClauseKeyword) -> String {
    let t = js_trim(text);
    let rest = match keyword {
        ClauseKeyword::Where => strip_word(t, "where"),
        ClauseKeyword::OrderBy => strip_word(t, "order").and_then(|r| {
            let after = r.trim_start_matches(is_js_space);
            if after.len() == r.len() {
                None
            } else {
                strip_word(after, "by")
            }
        }),
    };
    match rest {
        Some(r) if !r.chars().next().is_some_and(is_word) => js_trim(r).to_owned(),
        _ => t.to_owned(),
    }
}

/// The SELECT a table tab pages through, and whether it may run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableSelect {
    pub sql: String,
    /// Set when a clause must not be run (`WHERE: …` / `ORDER BY: …`).
    pub error: Option<String>,
}

/// The statement a table tab pages through: `select *` with the tab's WHERE
/// and ORDER BY, one clause per line (so a trailing `--` comment can't
/// swallow the next).
pub fn table_select(schema: &str, table: &str, q: &TableQuery) -> TableSelect {
    let filter = normalize_clause(&q.filter, ClauseKeyword::Where);
    let order_by = normalize_clause(&q.order_by, ClauseKeyword::OrderBy);
    let mut lines = vec![format!("select * from {}", qualified(schema, table))];
    if !filter.is_empty() {
        lines.push(format!("where {filter}"));
    }
    if !order_by.is_empty() {
        lines.push(format!("order by {order_by}"));
    }
    let where_err = (!filter.is_empty())
        .then(|| clause_error(&filter))
        .flatten();
    let order_err = (!order_by.is_empty())
        .then(|| clause_error(&order_by))
        .flatten();
    TableSelect {
        sql: lines.join("\n"),
        error: where_err
            .map(|e| format!("WHERE: {e}"))
            .or_else(|| order_err.map(|e| format!("ORDER BY: {e}"))),
    }
}

// ── ORDER BY items ────────────────────────────────────────────────────────

/// One item of an ORDER BY clause.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OrderItem {
    /// The item as typed (or as rewritten by a header click).
    pub text: String,
    /// The sort expression, without direction / nulls.
    pub expr: String,
    pub dir: SortDir,
    pub nulls: Option<Nulls>,
    /// Column the expression names (`name`, `"Name"`), else `None`.
    pub column: Option<String>,
}

fn ident_start(c: char) -> bool {
    c.is_ascii_alphabetic() || c == '_' || c >= '\u{c0}'
}

/// The column a bare or "quoted" identifier names (bare ones fold to lower
/// case), else `None`.
pub fn ident_name(expr: &str) -> Option<String> {
    let e = js_trim(expr);
    let mut chars = e.chars();
    if chars.next().is_some_and(ident_start)
        && chars.all(|c| is_word(c) || c == '$' || c >= '\u{c0}')
    {
        return Some(e.to_lowercase());
    }
    let inner = e.strip_prefix('"')?.strip_suffix('"')?;
    if inner.is_empty() {
        return None;
    }
    let mut out = String::new();
    let mut it = inner.chars();
    while let Some(c) = it.next() {
        // A quote inside must be doubled.
        if c == '"' && it.next() != Some('"') {
            return None;
        }
        out.push(c);
    }
    Some(out)
}

/// Strips a trailing `\s+word` (one of `words`, case-insensitive): the text
/// before the white space, and the word.
fn strip_trailing<'a>(s: &'a str, words: &[&'static str]) -> Option<(&'a str, &'static str)> {
    words.iter().find_map(|&w| {
        let cut = s.len().checked_sub(w.len())?;
        let tail = s.get(cut..)?;
        if !tail.eq_ignore_ascii_case(w) {
            return None;
        }
        let before = &s[..cut];
        let trimmed = before.trim_end_matches(is_js_space);
        (trimmed.len() < before.len()).then_some((trimmed, w))
    })
}

fn parse_item(part: &str) -> OrderItem {
    // `^([\s\S]*?)(?:\s+(asc|desc))?(?:\s+nulls\s+(first|last))?$`
    let mut rest = part;
    let mut nulls = None;
    if let Some((before, w)) = strip_trailing(rest, &["first", "last"])
        && let Some((before, _)) = strip_trailing(before, &["nulls"])
    {
        rest = before;
        nulls = Some(if w == "first" {
            Nulls::First
        } else {
            Nulls::Last
        });
    }
    let mut dir = SortDir::Asc;
    if let Some((before, w)) = strip_trailing(rest, &["asc", "desc"]) {
        rest = before;
        dir = if w == "desc" {
            SortDir::Desc
        } else {
            SortDir::Asc
        };
    }
    let expr = match js_trim(rest) {
        "" => part,
        e => e,
    };
    OrderItem {
        text: part.to_owned(),
        expr: expr.to_owned(),
        dir,
        nulls,
        column: ident_name(expr),
    }
}

/// An ORDER BY clause's comma-separated items.
pub fn parse_order_by(order_by: &str) -> Vec<OrderItem> {
    let text = normalize_clause(order_by, ClauseKeyword::OrderBy);
    if text.is_empty() {
        return Vec::new();
    }
    let chars: Vec<char> = text.chars().collect();
    // Can't be split reliably: one opaque item (a header click replaces it).
    let parts = if scan(&chars).error.is_some() {
        vec![text]
    } else {
        split_top_level(&text, Separator::Literal(","))
            .into_iter()
            .filter(|p| !p.is_empty())
            .collect()
    };
    parts.iter().map(|p| parse_item(p)).collect()
}

/// A sorted column's header arrow.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ColumnSort {
    pub dir: SortDir,
    /// 1-based position in the ORDER BY.
    pub rank: usize,
}

/// Header arrows: each column the ORDER BY sorts by (first mention), in order.
pub fn sort_state(order_by: &str) -> Vec<(String, ColumnSort)> {
    let mut out: Vec<(String, ColumnSort)> = Vec::new();
    for (i, it) in parse_order_by(order_by).into_iter().enumerate() {
        if let Some(col) = it.column
            && !out.iter().any(|(c, _)| *c == col)
        {
            out.push((
                col,
                ColumnSort {
                    dir: it.dir,
                    rank: i + 1,
                },
            ));
        }
    }
    out
}

fn render(column: &str, dir: SortDir, nulls: Option<Nulls>) -> String {
    let mut s = quote_ident(column);
    if dir == SortDir::Desc {
        s.push_str(" desc");
    }
    if let Some(n) = nulls {
        s.push_str(" nulls ");
        s.push_str(n.word());
    }
    s
}

/// A header click on `column` (DataGrip): ascending → descending → off. A
/// column already in the ORDER BY changes in place; another one replaces
/// the sort, or with `add` (⇧-click) is appended to it.
pub fn cycle_sort(order_by: &str, column: &str, add: bool) -> String {
    let items = parse_order_by(order_by);
    let mut texts: Vec<String> = items.iter().map(|it| it.text.clone()).collect();
    match items
        .iter()
        .position(|it| it.column.as_deref() == Some(column))
    {
        Some(i) => {
            if items[i].dir == SortDir::Asc {
                texts[i] = render(column, SortDir::Desc, items[i].nulls);
            } else {
                texts.remove(i);
            }
        }
        None if add => texts.push(render(column, SortDir::Asc, None)),
        None => texts = vec![render(column, SortDir::Asc, None)],
    }
    texts.join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn q(filter: &str, order_by: &str) -> TableQuery {
        TableQuery {
            filter: filter.into(),
            order_by: order_by.into(),
        }
    }

    // ── tableSelect ──

    #[test]
    fn table_select_is_the_plain_select_without_clauses() {
        assert_eq!(
            table_select("public", "users", &TableQuery::default()),
            TableSelect {
                sql: "select * from public.users".into(),
                error: None
            }
        );
        assert_eq!(
            table_select("public", "users", &q("  ", "")).sql,
            "select * from public.users"
        );
    }

    #[test]
    fn table_select_adds_where_and_order_by_one_per_line() {
        assert_eq!(
            table_select("Sales", "order", &q("status = 'active' and id > 10", "id desc")),
            TableSelect {
                sql: "select * from \"Sales\".\"order\"\nwhere status = 'active' and id > 10\norder by id desc".into(),
                error: None
            }
        );
        assert_eq!(
            table_select("public", "users", &q("", "email")).sql,
            "select * from public.users\norder by email"
        );
    }

    #[test]
    fn table_select_drops_a_typed_out_keyword() {
        assert_eq!(
            table_select("public", "users", &q("WHERE id = 1", "order  by id")).sql,
            "select * from public.users\nwhere id = 1\norder by id"
        );
        // Only whole words.
        assert_eq!(
            normalize_clause("whereabouts = 1", ClauseKeyword::Where),
            "whereabouts = 1"
        );
        assert_eq!(
            normalize_clause("orderby", ClauseKeyword::OrderBy),
            "orderby"
        );
    }

    #[test]
    fn table_select_keeps_a_trailing_comment_from_swallowing_the_next_clause() {
        let s = table_select("public", "users", &q("id = 1 -- me", "id"));
        assert_eq!(s.error, None);
        assert_eq!(
            s.sql.split('\n').collect::<Vec<_>>(),
            [
                "select * from public.users",
                "where id = 1 -- me",
                "order by id"
            ]
        );
    }

    #[test]
    fn table_select_flags_clauses_that_must_not_run() {
        assert_eq!(
            table_select("public", "users", &q("true; drop table users", ""))
                .error
                .as_deref(),
            Some("WHERE: Only one condition: remove the \";\"")
        );
        assert!(
            table_select("public", "users", &q("", "id; delete from users"))
                .error
                .unwrap()
                .starts_with("ORDER BY: ")
        );
    }

    // ── clauseError ──

    #[test]
    fn clause_error_accepts_semicolons_inside_strings_names_comments_and_dollar_quotes() {
        for ok in [
            "name = 'a;b'",
            "name = 'it''s;'",
            "x = E'\\';'",
            "\"we;ird\" = 1",
            "id = 1 -- ; trailing",
            "id = 1 /* ; /* nested ; */ */",
            "body = $$;$$",
            "body = $t$ ; $t$",
            "id in (select id from t where a = (1))",
        ] {
            assert_eq!(clause_error(ok), None, "{ok}");
        }
    }

    #[test]
    fn clause_error_rejects_a_second_statement_and_broken_nesting() {
        assert!(
            clause_error("true; drop table users")
                .unwrap()
                .contains(';')
        );
        assert!(clause_error("true;").unwrap().contains(';'));
        let err = |s: &str| clause_error(s);
        assert_eq!(
            err("1=1) union select 1 where (1=1").as_deref(),
            Some("Unbalanced parentheses")
        );
        assert_eq!(err("(a = 1").as_deref(), Some("Unbalanced parentheses"));
        assert_eq!(
            err("name = 'open").as_deref(),
            Some("Unterminated string literal")
        );
        assert_eq!(
            err("\"open = 1").as_deref(),
            Some("Unterminated quoted identifier")
        );
        assert_eq!(
            err("a = 1 /* open").as_deref(),
            Some("Unterminated /* comment")
        );
        assert_eq!(
            err("a = $x$ open").as_deref(),
            Some("Unterminated $x$ string")
        );
    }

    #[test]
    fn clause_error_does_not_treat_identifiers_with_dollar_as_dollar_quotes() {
        assert_eq!(clause_error("a$b = 1"), None);
    }

    // ── splitTopLevel ──

    #[test]
    fn split_top_level_splits_only_outside_parentheses_and_strings() {
        assert_eq!(
            split_top_level("a, coalesce(b, c), 'x,y' desc", Separator::Literal(",")),
            ["a", "coalesce(b, c)", "'x,y' desc"]
        );
        assert_eq!(
            split_top_level("role = 'a and b' AND id > 1", Separator::Word("and")),
            ["role = 'a and b'", "id > 1"]
        );
    }

    // ── parseOrderBy ──

    #[test]
    fn parse_order_by_reads_direction_nulls_and_the_column_named() {
        assert_eq!(
            parse_order_by("id, \"Name\" DESC nulls last, lower(email) asc"),
            [
                OrderItem {
                    text: "id".into(),
                    expr: "id".into(),
                    dir: SortDir::Asc,
                    nulls: None,
                    column: Some("id".into())
                },
                OrderItem {
                    text: "\"Name\" DESC nulls last".into(),
                    expr: "\"Name\"".into(),
                    dir: SortDir::Desc,
                    nulls: Some(Nulls::Last),
                    column: Some("Name".into())
                },
                OrderItem {
                    text: "lower(email) asc".into(),
                    expr: "lower(email)".into(),
                    dir: SortDir::Asc,
                    nulls: None,
                    column: None
                },
            ]
        );
        assert_eq!(parse_order_by("  "), []);
    }

    #[test]
    fn ident_name_folds_bare_names_to_lower_case_like_postgres() {
        assert_eq!(ident_name("Email").as_deref(), Some("email"));
        assert_eq!(ident_name("\"Email\"").as_deref(), Some("Email"));
        assert_eq!(ident_name("\"a\"\"b\"").as_deref(), Some("a\"b"));
        assert_eq!(ident_name("t.email"), None);
        assert_eq!(ident_name("\"a\"b\""), None);
    }

    // ── sortState ──

    #[test]
    fn sort_state_maps_sorted_columns_to_direction_and_position() {
        assert_eq!(
            sort_state("created_at desc, lower(x), id"),
            [
                (
                    "created_at".to_owned(),
                    ColumnSort {
                        dir: SortDir::Desc,
                        rank: 1
                    }
                ),
                (
                    "id".to_owned(),
                    ColumnSort {
                        dir: SortDir::Asc,
                        rank: 3
                    }
                ),
            ]
        );
    }

    // ── cycleSort ──

    #[test]
    fn cycle_sort_goes_ascending_descending_off() {
        let a = cycle_sort("", "email", false);
        assert_eq!(a, "email");
        let b = cycle_sort(&a, "email", false);
        assert_eq!(b, "email desc");
        assert_eq!(cycle_sort(&b, "email", false), "");
    }

    #[test]
    fn cycle_sort_replaces_the_sort_or_appends_with_add() {
        assert_eq!(cycle_sort("email desc", "id", false), "id");
        assert_eq!(cycle_sort("email desc", "id", true), "email desc, id");
    }

    #[test]
    fn cycle_sort_changes_a_column_in_place_keeping_other_items() {
        assert_eq!(
            cycle_sort("lower(name), id NULLS FIRST, x", "id", false),
            "lower(name), id desc nulls first, x"
        );
        assert_eq!(
            cycle_sort("lower(name), id desc, x", "id", false),
            "lower(name), x"
        );
    }

    #[test]
    fn cycle_sort_quotes_column_names_that_need_it() {
        assert_eq!(cycle_sort("", "Created At", false), "\"Created At\"");
        assert_eq!(
            cycle_sort("\"Created At\"", "Created At", false),
            "\"Created At\" desc"
        );
        assert_eq!(cycle_sort("", "order", false), "\"order\"");
    }
}
