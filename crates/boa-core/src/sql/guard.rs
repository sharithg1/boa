//! Write guard rails: which statements are destructive enough to confirm
//! before they run, and which would talk a read-only session out of being
//! read-only. Works on the tokenizer and statement splitter, never on raw
//! text, so comments, strings, quoted names and dollar quotes cannot fool it.
//!
//! Destructive: UPDATE / DELETE without a WHERE (also inside a CTE or
//! EXPLAIN ANALYZE, which run them), TRUNCATE, DROP of anything, and ALTER
//! TABLE … DROP COLUMN. Not looked into: the bodies of DO blocks and
//! functions (they are strings).

use std::borrow::Cow;

use super::chars::{is_ident, is_ident_start, is_space};
use super::statements::split_statements;
use super::tokenize::{TokenKind, tokenize};

/// What kind of destructive statement was found.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RiskKind {
    /// UPDATE / DELETE without a WHERE.
    NoWhere,
    Truncate,
    Drop,
    /// ALTER TABLE … DROP COLUMN.
    DropColumn,
}

impl RiskKind {
    /// `no-where`, `truncate`, `drop`, `drop-column` (the TypeScript names).
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NoWhere => "no-where",
            Self::Truncate => "truncate",
            Self::Drop => "drop",
            Self::DropColumn => "drop-column",
        }
    }
}

/// A destructive part of a script.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Risk {
    pub kind: RiskKind,
    /// What is risky, for the confirmation dialog.
    pub message: String,
    /// The statement it was found in.
    pub statement: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TokKind {
    Word,
    Quoted,
    Punct,
    String,
    Other,
}

#[derive(Debug, Clone)]
struct Tok<'a> {
    kind: TokKind,
    /// Words lower-cased; everything else as written.
    text: Cow<'a, str>,
    raw: &'a str,
}

/// Splits a run of non-literal text the way the original's
/// `/"(?:[^"]|"")*"?|`[^`]*`?|[A-Za-z_À-￿][\w$À-￿]*|::|\S/g` does.
fn pieces<'a>(s: &'a str, out: &mut Vec<Tok<'a>>) {
    let b = s.as_bytes();
    let mut i = 0;
    while let Some(c) = s[i..].chars().next() {
        let start = i;
        if is_space(c) {
            i += c.len_utf8();
            continue;
        }
        let kind = if c == '"' {
            i += 1;
            loop {
                match b.get(i) {
                    None => break,
                    Some(b'"') if b.get(i + 1) == Some(&b'"') => i += 2,
                    Some(b'"') => {
                        i += 1;
                        break;
                    }
                    Some(_) => i += 1,
                }
            }
            TokKind::Quoted
        } else if c == '`' {
            i += s[i + 1..].find('`').map_or(s.len() - i, |e| e + 2);
            TokKind::Quoted
        } else if is_ident_start(c) {
            i += c.len_utf8();
            while let Some(d) = s[i..].chars().next().filter(|d| is_ident(*d)) {
                i += d.len_utf8();
            }
            TokKind::Word
        } else if s[i..].starts_with("::") {
            i += 2;
            TokKind::Punct
        } else {
            i += c.len_utf8();
            TokKind::Punct
        };
        let raw = &s[start..i];
        let text = if kind == TokKind::Word {
            Cow::Owned(raw.to_lowercase())
        } else {
            Cow::Borrowed(raw)
        };
        out.push(Tok { kind, text, raw });
    }
}

/// Significant tokens: no comments or blanks; words lower-cased, one per token.
fn lex(sql: &str) -> Vec<Tok<'_>> {
    let mut out = Vec::new();
    for t in tokenize(sql, None) {
        let raw = t.text(sql);
        let kind = match t.kind {
            TokenKind::Comment => continue,
            TokenKind::String => TokKind::String,
            TokenKind::Number | TokenKind::Param => TokKind::Other,
            // Plain runs merge words, blanks and quoted names: split them again.
            _ => {
                pieces(raw, &mut out);
                continue;
            }
        };
        out.push(Tok {
            kind,
            text: Cow::Borrowed(raw),
            raw,
        });
    }
    out
}

fn text<'t>(toks: &'t [Tok<'_>], i: usize) -> Option<&'t str> {
    toks.get(i).map(|t| t.text.as_ref())
}

fn is_name(t: Option<&Tok<'_>>) -> bool {
    t.is_some_and(|t| matches!(t.kind, TokKind::Word | TokKind::Quoted))
}

/// A possibly qualified name starting at `i`: its text and the index after it.
fn name_at(toks: &[Tok<'_>], i: usize) -> Option<(String, usize)> {
    if !is_name(toks.get(i)) {
        return None;
    }
    let mut name = toks[i].raw.to_owned();
    let mut k = i + 1;
    while text(toks, k) == Some(".") && is_name(toks.get(k + 1)) {
        name.push('.');
        name.push_str(toks[k + 1].raw);
        k += 2;
    }
    Some((name, k))
}

/// Index after the `)` matching the `(` at `i`.
fn skip_parens(toks: &[Tok<'_>], i: usize) -> usize {
    let mut depth = 0i32;
    for (k, t) in toks.iter().enumerate().skip(i) {
        if t.text == "(" {
            depth += 1;
        } else if t.text == ")" {
            depth -= 1;
            if depth == 0 {
                return k + 1;
            }
        }
    }
    toks.len()
}

/// Where the statement really starts: after EXPLAIN ANALYZE options. Plain
/// EXPLAIN only plans, so it returns None (nothing runs).
fn body_start(toks: &[Tok<'_>]) -> Option<usize> {
    if text(toks, 0) != Some("explain") {
        return Some(0);
    }
    let mut analyze = false;
    let mut i = 1;
    if text(toks, 1) == Some("(") {
        let end = skip_parens(toks, 1);
        for k in 2..end {
            if !matches!(text(toks, k), Some("analyze" | "analyse")) {
                continue;
            }
            analyze = !matches!(text(toks, k + 1), Some("false" | "off" | "0"));
        }
        i = end;
    } else {
        while let Some(w @ ("analyze" | "analyse" | "verbose")) = text(toks, i) {
            if w != "verbose" {
                analyze = true;
            }
            i += 1;
        }
    }
    analyze.then_some(i)
}

/// UPDATE / DELETE at `i`: is there a WHERE at its own level?
fn has_where(toks: &[Tok<'_>], i: usize) -> bool {
    let mut depth = 0i32;
    for t in toks.iter().skip(i + 1) {
        if t.text == "(" {
            depth += 1;
        } else if t.text == ")" {
            depth -= 1;
            if depth < 0 {
                return false; // end of the CTE / subquery it is in
            }
        } else if depth == 0 && t.text == "where" && t.kind == TokKind::Word {
            return true;
        }
    }
    false
}

const DROP_OBJECT_WORDS: &[&str] = &[
    "access",
    "aggregate",
    "cast",
    "collation",
    "conversion",
    "data",
    "database",
    "default",
    "domain",
    "event",
    "extension",
    "foreign",
    "function",
    "group",
    "index",
    "language",
    "materialized",
    "method",
    "operator",
    "owned",
    "policy",
    "procedure",
    "publication",
    "role",
    "routine",
    "rule",
    "schema",
    "sequence",
    "server",
    "statistics",
    "subscription",
    "table",
    "tablespace",
    "text",
    "search",
    "configuration",
    "dictionary",
    "parser",
    "template",
    "transform",
    "trigger",
    "type",
    "user",
    "mapping",
    "view",
    "wrapper",
    "class",
    "family",
    "concurrently",
];

fn risks_of(statement: &str, out: &mut Vec<Risk>) {
    let toks = lex(statement);
    let Some(start) = body_start(&toks) else {
        return;
    };
    let mut add = |kind: RiskKind, message: String| {
        out.push(Risk {
            kind,
            message,
            statement: statement.to_owned(),
        });
    };
    let first = text(&toks, start);

    if first == Some("truncate") {
        let mut k = start + 1;
        if text(&toks, k) == Some("table") {
            k += 1;
        }
        let mut names = Vec::new();
        while k < toks.len() {
            if text(&toks, k) == Some("only") {
                k += 1;
            }
            let Some((name, next)) = name_at(&toks, k) else {
                break;
            };
            names.push(name);
            k = next;
            if text(&toks, k) != Some(",") {
                break;
            }
            k += 1;
        }
        let what = if names.is_empty() {
            "the table".to_owned()
        } else {
            names.join(", ")
        };
        add(
            RiskKind::Truncate,
            format!("TRUNCATE removes every row of {what}"),
        );
    }

    if first == Some("drop") {
        let mut k = start + 1;
        let mut target = vec!["DROP".to_owned()];
        while let Some(t) = toks.get(k)
            && t.kind == TokKind::Word
            && DROP_OBJECT_WORDS.contains(&t.text.as_ref())
        {
            target.push(t.raw.to_uppercase());
            k += 1;
        }
        if text(&toks, k) == Some("if") && text(&toks, k + 1) == Some("exists") {
            k += 2;
        }
        let mut names = Vec::new();
        let mut n = name_at(&toks, k);
        while let Some((name, next)) = n {
            names.push(name);
            n = if text(&toks, next) == Some(",") {
                name_at(&toks, next + 1)
            } else {
                None
            };
        }
        if !names.is_empty() {
            target.push(names.join(", "));
        }
        let cascade = toks
            .iter()
            .any(|t| t.kind == TokKind::Word && t.text == "cascade");
        add(
            RiskKind::Drop,
            format!(
                "{} cannot be undone{}",
                target.join(" "),
                if cascade {
                    "; CASCADE also drops everything that depends on it"
                } else {
                    ""
                }
            ),
        );
    }

    if first == Some("alter") && text(&toks, start + 1) == Some("table") {
        let mut k = start + 2;
        if text(&toks, k) == Some("if") && text(&toks, k + 1) == Some("exists") {
            k += 2;
        }
        if text(&toks, k) == Some("only") {
            k += 1;
        }
        let table = name_at(&toks, k).map_or_else(|| "the table".to_owned(), |(n, _)| n);
        let mut depth = 0i32;
        for j in k..toks.len() {
            let t = &toks[j];
            if t.text == "(" {
                depth += 1;
            } else if t.text == ")" {
                depth -= 1;
            }
            if depth != 0 || t.kind != TokKind::Word || t.text != "drop" {
                continue;
            }
            let mut c = j + 1;
            let next = text(&toks, c);
            if matches!(
                next,
                Some("constraint" | "not" | "default" | "identity" | "expression")
            ) {
                continue;
            }
            if next == Some("column") {
                c += 1;
            }
            if text(&toks, c) == Some("if") && text(&toks, c + 1) == Some("exists") {
                c += 2;
            }
            let col = name_at(&toks, c)
                .map(|(n, _)| format!("{n} "))
                .unwrap_or_default();
            add(
                RiskKind::DropColumn,
                format!("DROP COLUMN {col}deletes column data from {table}"),
            );
        }
    }

    // UPDATE / DELETE in command position: at the start, opening a CTE or
    // subquery `(`, or after a CTE list `)`. Elsewhere (`ON DELETE`, `FOR
    // UPDATE`, `GRANT UPDATE`, `DO UPDATE`) they are not commands.
    for k in start..toks.len() {
        let t = &toks[k];
        if t.kind != TokKind::Word || (t.text != "update" && t.text != "delete") {
            continue;
        }
        let prev = k.checked_sub(1).and_then(|p| text(&toks, p));
        if k != start && prev != Some("(") && prev != Some(")") {
            continue;
        }
        if has_where(&toks, k) {
            continue;
        }
        let delete = t.text == "delete";
        let mut j = k + 1;
        if delete && text(&toks, j) == Some("from") {
            j += 1;
        }
        if text(&toks, j) == Some("only") {
            j += 1;
        }
        let target = name_at(&toks, j).map_or_else(|| "the table".to_owned(), |(n, _)| n);
        add(
            RiskKind::NoWhere,
            if delete {
                format!("DELETE without WHERE removes every row of {target}")
            } else {
                format!("UPDATE without WHERE changes every row of {target}")
            },
        );
    }
}

/// Every destructive part of `sql` (one or several statements), in order.
pub fn destructive_risks(sql: &str) -> Vec<Risk> {
    let mut out = Vec::new();
    for s in split_statements(sql) {
        risks_of(&sql[s.from..s.to], &mut out);
    }
    out
}

/// Why `sql` would make a read-only session writable (`BEGIN READ WRITE`,
/// `SET default_transaction_read_only = off`, `set_config(…)`), or None.
/// Reading the settings (`SHOW`, `current_setting`) is fine.
pub fn read_only_escape(sql: &str) -> Option<&'static str> {
    for r in split_statements(sql) {
        let toks = lex(&sql[r.from..r.to]);
        if toks
            .windows(2)
            .any(|w| w[0].text == "read" && w[1].text == "write")
        {
            return Some("READ WRITE transactions are not allowed on a read-only connection");
        }
        let setting = |t: &Tok<'_>| {
            matches!(t.kind, TokKind::Word | TokKind::Quoted | TokKind::String)
                && t.text.to_lowercase().contains("transaction_read_only")
        };
        let head = text(&toks, 0);
        let changes =
            matches!(head, Some("set" | "alter")) || toks.iter().any(|t| t.text == "set_config");
        if changes && toks.iter().any(setting) {
            return Some("The read-only setting cannot be changed on a read-only connection");
        }
    }
    None
}

/// When to ask before running something risky.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ConfirmOptions {
    /// The user's "confirm destructive statements" preference.
    pub confirm_destructive: bool,
    /// The connection is tagged Production (always confirms).
    pub production: bool,
}

/// Ask before running: something is risky and the preference (or a production tag) says so.
pub fn needs_confirm(risks: &[Risk], opts: ConfirmOptions) -> bool {
    !risks.is_empty() && (opts.confirm_destructive || opts.production)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(sql: &str) -> Vec<&'static str> {
        destructive_risks(sql)
            .iter()
            .map(|r| r.kind.as_str())
            .collect()
    }
    fn message(sql: &str) -> String {
        destructive_risks(sql)
            .first()
            .map(|r| r.message.clone())
            .unwrap_or_default()
    }
    const NONE: [&str; 0] = [];

    #[test]
    fn flags_update_and_delete_without_where_naming_the_table() {
        assert_eq!(
            destructive_risks("delete from public.\"Users\""),
            [Risk {
                kind: RiskKind::NoWhere,
                message: "DELETE without WHERE removes every row of public.\"Users\"".into(),
                statement: "delete from public.\"Users\"".into(),
            }]
        );
        assert_eq!(
            message("UPDATE ONLY t SET a = 1"),
            "UPDATE without WHERE changes every row of t"
        );
        assert_eq!(kinds("update t set a = 1 from u"), ["no-where"]);
    }

    #[test]
    fn accepts_a_where_at_the_statement_level_only() {
        assert_eq!(kinds("delete from t where id = 1"), NONE);
        assert_eq!(
            kinds("update t set a = (select b from u where u.id = 1)"),
            ["no-where"]
        );
        assert_eq!(kinds("delete from t using u where t.id = u.id"), NONE);
        assert_eq!(kinds("update t set a = 1 where current of c"), NONE);
    }

    #[test]
    fn is_not_fooled_by_comments_strings_and_quoted_names() {
        assert_eq!(kinds("delete from t -- where id = 1"), ["no-where"]);
        assert_eq!(kinds("delete from t /* where id = 1 */"), ["no-where"]);
        assert_eq!(kinds("update t set note = 'where x'"), ["no-where"]);
        assert_eq!(kinds("update t set note = $$ where $$"), ["no-where"]);
        assert_eq!(kinds("update t set \"where\" = 1"), ["no-where"]);
        assert_eq!(kinds("select 'delete from t'"), NONE);
        assert_eq!(kinds("-- drop table t\nselect 1"), NONE);
        assert_eq!(kinds("select \"drop\", \"truncate\" from t"), NONE);
    }

    #[test]
    fn looks_into_ctes_in_both_directions() {
        assert_eq!(
            kinds("with d as (delete from t returning *) select * from d where id > 1"),
            ["no-where"]
        );
        assert_eq!(
            kinds("with d as (delete from t where a returning *) select * from d"),
            NONE
        );
        assert_eq!(
            kinds("with x as (select 1 where true) update t set a = 1"),
            ["no-where"]
        );
        assert_eq!(
            kinds("with x as (select 1) update t set a = 1 where id in (select * from x)"),
            NONE
        );
    }

    #[test]
    fn ignores_update_and_delete_that_are_not_commands() {
        assert_eq!(kinds("select * from t for update"), NONE);
        assert_eq!(
            kinds("create table c (p int references p (id) on delete cascade on update set null)"),
            NONE
        );
        assert_eq!(kinds("grant select, update, delete on t to bob"), NONE);
        assert_eq!(
            kinds("insert into t values (1) on conflict (id) do update set a = 1"),
            NONE
        );
        assert_eq!(
            kinds("create trigger x before update or delete on t execute function f()"),
            NONE
        );
    }

    #[test]
    fn treats_explain_as_harmless_unless_it_analyzes() {
        assert_eq!(kinds("explain delete from t"), NONE);
        assert_eq!(kinds("explain (costs off) delete from t"), NONE);
        assert_eq!(kinds("explain analyze delete from t"), ["no-where"]);
        assert_eq!(
            kinds("explain (analyze, buffers) update t set a = 1"),
            ["no-where"]
        );
        assert_eq!(kinds("explain (analyze false) update t set a = 1"), NONE);
    }

    #[test]
    fn flags_truncate_and_drop() {
        assert_eq!(
            message("truncate table a, only b"),
            "TRUNCATE removes every row of a, b"
        );
        assert_eq!(
            message("DROP TABLE IF EXISTS s.a, b CASCADE"),
            "DROP TABLE s.a, b cannot be undone; CASCADE also drops everything that depends on it"
        );
        assert_eq!(
            message("drop materialized view v"),
            "DROP MATERIALIZED VIEW v cannot be undone"
        );
        assert_eq!(kinds("drop schema app"), ["drop"]);
    }

    #[test]
    fn flags_dropped_columns_not_dropped_defaults_or_constraints() {
        assert_eq!(
            message("alter table t drop column c"),
            "DROP COLUMN c deletes column data from t"
        );
        assert_eq!(
            kinds("alter table t drop if exists c, add d int"),
            ["drop-column"]
        );
        assert_eq!(kinds("alter table t drop c"), ["drop-column"]);
        assert_eq!(kinds("alter table t alter column c drop default"), NONE);
        assert_eq!(kinds("alter table t alter column c drop not null"), NONE);
        assert_eq!(kinds("alter table t drop constraint t_pkey"), NONE);
    }

    #[test]
    fn checks_every_statement_of_a_script_keeping_each_one() {
        let r = destructive_risks(
            "select 1;\ndelete from a;\nupdate b set x = 1 where y;\ndrop view v;",
        );
        assert_eq!(
            r.iter()
                .map(|x| (x.kind.as_str(), x.statement.as_str()))
                .collect::<Vec<_>>(),
            [("no-where", "delete from a"), ("drop", "drop view v")]
        );
        assert_eq!(kinds("insert into t select * from u"), NONE);
        assert_eq!(kinds(""), NONE);
    }

    #[test]
    fn read_only_escape_catches_ways_to_make_the_session_writable() {
        let rw = |sql: &str| read_only_escape(sql).is_some_and(|m| m.contains("READ WRITE"));
        assert!(rw("begin read write"));
        assert!(rw("BEGIN ISOLATION LEVEL SERIALIZABLE, READ /* x */ WRITE"));
        assert!(
            read_only_escape("set session characteristics as transaction read write").is_some()
        );
        assert!(read_only_escape("SET default_transaction_read_only = off").is_some());
        assert!(read_only_escape("set transaction_read_only to false").is_some());
        assert!(
            read_only_escape("select set_config('default_transaction_read_only', 'off', false)")
                .is_some()
        );
    }

    #[test]
    fn read_only_escape_lets_reads_of_the_setting_and_other_statements_through() {
        assert_eq!(read_only_escape("show default_transaction_read_only"), None);
        assert_eq!(
            read_only_escape("select current_setting('transaction_read_only')"),
            None
        );
        assert_eq!(read_only_escape("select 'read write'"), None);
        assert_eq!(read_only_escape("-- read write\nselect 1"), None);
        assert_eq!(read_only_escape("begin read only"), None);
    }

    #[test]
    fn needs_confirm_follows_the_preference_except_on_production() {
        let risks = destructive_risks("truncate t");
        let o = |confirm_destructive, production| ConfirmOptions {
            confirm_destructive,
            production,
        };
        assert!(needs_confirm(&risks, o(true, false)));
        assert!(!needs_confirm(&risks, o(false, false)));
        assert!(needs_confirm(&risks, o(false, true)));
        assert!(!needs_confirm(&[], o(true, true)));
    }

    // --- Rust-port specific ---------------------------------------------------

    #[test]
    fn names_non_ascii_targets() {
        assert_eq!(
            message("delete from öffentlich.\"Tabelle ä\""),
            "DELETE without WHERE removes every row of öffentlich.\"Tabelle ä\""
        );
        assert_eq!(
            message("drop table \"a\"\"b\", `c`"),
            "DROP TABLE \"a\"\"b\", `c` cannot be undone"
        );
        assert_eq!(kinds("select 'é'; update t set x = 'ü' where y"), NONE);
    }
}
