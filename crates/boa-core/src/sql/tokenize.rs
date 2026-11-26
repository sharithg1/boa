//! SQL tokenizer for the console editor's highlighting. One pass, no parser:
//! comments, strings, quoted identifiers, numbers, parameters, keywords, type
//! names, function calls and, given the database's catalog, schema / table /
//! column references. Tokens cover the input exactly (they are contiguous
//! byte ranges from 0 to `sql.len()`), so the highlight runs line up with the
//! buffer. Runs every keystroke: one `Vec` of small `Copy` tokens, no
//! per-token allocation (see [`tokenize_into`] to reuse the buffer).

use std::collections::HashSet;
use std::ops::Range;

use super::chars::{char_at, char_before, is_blank, is_ident, is_ident_start, with_lower};
use crate::model::Schema;

/// What a token is, for colouring.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TokenKind {
    Keyword,
    Type,
    Function,
    String,
    Number,
    Comment,
    Param,
    Schema,
    Table,
    Column,
    Operator,
    /// Blanks, punctuation `( ) , ;` and names that are none of the above.
    Plain,
}

impl TokenKind {
    /// Lower-case name (`keyword`, `type`, …), as in the TypeScript original.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Keyword => "keyword",
            Self::Type => "type",
            Self::Function => "function",
            Self::String => "string",
            Self::Number => "number",
            Self::Comment => "comment",
            Self::Param => "param",
            Self::Schema => "schema",
            Self::Table => "table",
            Self::Column => "column",
            Self::Operator => "operator",
            Self::Plain => "plain",
        }
    }
}

/// A token: its kind and byte range in the source.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Token {
    pub kind: TokenKind,
    /// Byte offset of the first byte.
    pub start: usize,
    /// Byte offset after the last byte.
    pub end: usize,
}

impl Token {
    /// The byte range.
    pub fn range(&self) -> Range<usize> {
        self.start..self.end
    }

    /// The token's text in `src` (the string it was produced from).
    pub fn text<'a>(&self, src: &'a str) -> &'a str {
        &src[self.start..self.end]
    }
}

/// Names the database knows, lower-cased, to colour identifiers that refer to them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Catalog {
    pub schemas: HashSet<String>,
    pub tables: HashSet<String>,
    pub columns: HashSet<String>,
}

impl Catalog {
    /// Every schema, table and column name of `schemas`, lower-cased.
    pub fn from_schemas(schemas: &[Schema]) -> Self {
        let mut cat = Self::default();
        for s in schemas {
            cat.schemas.insert(s.name.to_lowercase());
            for t in &s.tables {
                cat.tables.insert(t.name.to_lowercase());
                for c in &t.columns {
                    cat.columns.insert(c.name.to_lowercase());
                }
            }
        }
        cat
    }
}

/// Reserved and common SQL keywords, lower-case, sorted.
pub const KEYWORDS: &[&str] = &[
    "add",
    "all",
    "alter",
    "analyze",
    "and",
    "any",
    "as",
    "asc",
    "begin",
    "between",
    "by",
    "cascade",
    "case",
    "cast",
    "check",
    "collate",
    "column",
    "commit",
    "constraint",
    "create",
    "cross",
    "current_date",
    "current_time",
    "current_timestamp",
    "database",
    "default",
    "delete",
    "desc",
    "distinct",
    "do",
    "drop",
    "else",
    "end",
    "except",
    "exists",
    "explain",
    "false",
    "fetch",
    "filter",
    "first",
    "for",
    "foreign",
    "from",
    "full",
    "grant",
    "group",
    "having",
    "if",
    "ilike",
    "in",
    "index",
    "inner",
    "insert",
    "intersect",
    "into",
    "is",
    "join",
    "key",
    "last",
    "lateral",
    "left",
    "like",
    "limit",
    "materialized",
    "natural",
    "not",
    "null",
    "nulls",
    "offset",
    "on",
    "only",
    "or",
    "order",
    "outer",
    "over",
    "partition",
    "primary",
    "recursive",
    "references",
    "rename",
    "replace",
    "restrict",
    "returning",
    "revoke",
    "right",
    "rollback",
    "row",
    "rows",
    "schema",
    "select",
    "set",
    "show",
    "similar",
    "table",
    "temp",
    "temporary",
    "then",
    "to",
    "transaction",
    "true",
    "truncate",
    "union",
    "unique",
    "update",
    "using",
    "vacuum",
    "values",
    "view",
    "when",
    "where",
    "window",
    "with",
    "within",
];

/// Type names (Postgres and common others), lower-case, sorted.
pub const TYPES: &[&str] = &[
    "bigint",
    "bigserial",
    "binary",
    "bit",
    "blob",
    "bool",
    "boolean",
    "box",
    "bpchar",
    "bytea",
    "char",
    "character",
    "cidr",
    "circle",
    "citext",
    "clob",
    "date",
    "daterange",
    "datetime",
    "datetime2",
    "dec",
    "decimal",
    "double",
    "enum",
    "float",
    "float4",
    "float8",
    "geography",
    "geometry",
    "hstore",
    "inet",
    "int",
    "int2",
    "int4",
    "int4range",
    "int8",
    "int8range",
    "integer",
    "interval",
    "json",
    "jsonb",
    "line",
    "longblob",
    "longtext",
    "lseg",
    "macaddr",
    "macaddr8",
    "mediumblob",
    "mediumint",
    "mediumtext",
    "money",
    "nchar",
    "ntext",
    "numeric",
    "numrange",
    "nvarchar",
    "oid",
    "path",
    "point",
    "polygon",
    "precision",
    "real",
    "regclass",
    "regproc",
    "regtype",
    "serial",
    "serial2",
    "serial4",
    "serial8",
    "smalldatetime",
    "smallint",
    "smallserial",
    "text",
    "time",
    "timestamp",
    "timestamptz",
    "timetz",
    "tinyblob",
    "tinyint",
    "tinytext",
    "tsquery",
    "tsrange",
    "tstzrange",
    "tsvector",
    "unsigned",
    "uuid",
    "varbinary",
    "varchar",
    "varying",
    "xml",
    "year",
    "zone",
];

/// `lower` (already lower-cased) is in [`KEYWORDS`].
pub fn is_keyword(lower: &str) -> bool {
    KEYWORDS.binary_search(&lower).is_ok()
}

/// `lower` (already lower-cased) is in [`TYPES`].
pub fn is_type(lower: &str) -> bool {
    TYPES.binary_search(&lower).is_ok()
}

/// Tokenizes `sql`; with a catalog, names it knows are coloured as schema /
/// table / column.
pub fn tokenize(sql: &str, catalog: Option<&Catalog>) -> Vec<Token> {
    let mut out = Vec::new();
    tokenize_into(sql, catalog, &mut out);
    out
}

/// [`tokenize`] into a reused buffer (cleared first).
pub fn tokenize_into(sql: &str, catalog: Option<&Catalog>, out: &mut Vec<Token>) {
    out.clear();
    let mut t = Tokenizer {
        sql,
        out,
        last_blank: false,
    };
    t.run(catalog);
}

struct Tokenizer<'a, 'o> {
    sql: &'a str,
    out: &'o mut Vec<Token>,
    /// The last token is a plain run of blanks only.
    last_blank: bool,
}

impl Tokenizer<'_, '_> {
    fn push(&mut self, kind: TokenKind, start: usize, end: usize) {
        let mergeable = matches!(kind, TokenKind::Plain | TokenKind::Operator);
        if let Some(last) = self.out.last_mut()
            && mergeable
            && last.kind == kind
        {
            // Merge runs of the same kind to keep the highlight runs few.
            last.end = end;
            self.last_blank = self.last_blank && is_blank(&self.sql[start..end]);
            return;
        }
        self.out.push(Token { kind, start, end });
        self.last_blank = kind == TokenKind::Plain && is_blank(&self.sql[start..end]);
    }

    /// The previous non-blank token: after a `.` a word is a name, never a keyword.
    fn prev_significant(&self) -> Option<Token> {
        let n = self.out.len();
        // Plain runs merge, so a blank run is never preceded by another plain one.
        if self.last_blank {
            n.checked_sub(2).map(|k| self.out[k])
        } else {
            self.out.last().copied()
        }
    }

    fn run(&mut self, catalog: Option<&Catalog>) {
        let sql = self.sql;
        let b = sql.as_bytes();
        let n = b.len();
        let at = |k: usize| b.get(k).copied().unwrap_or(0);
        let mut i = 0;
        while i < n {
            let c = b[i];
            let next = at(i + 1);

            // -- line comment
            if c == b'-' && next == b'-' {
                let j = find_byte(b, i, b'\n').unwrap_or(n);
                self.push(TokenKind::Comment, i, j);
                i = j;
                continue;
            }
            // /* block comment */ (unterminated: to the end)
            if c == b'/' && next == b'*' {
                let j = sql[i + 2..].find("*/").map_or(n, |e| i + 2 + e + 2);
                self.push(TokenKind::Comment, i, j);
                i = j;
                continue;
            }
            // 'string' with '' escapes; E'…' and N'…' prefixes stay with the string.
            if c == b'\'' || (matches!(c, b'E' | b'e' | b'N' | b'n') && next == b'\'') {
                let mut j = if c == b'\'' { i + 1 } else { i + 2 };
                while j < n {
                    if b[j] == b'\'' {
                        if at(j + 1) == b'\'' {
                            j += 2;
                        } else {
                            j += 1;
                            break;
                        }
                    } else {
                        j += 1;
                    }
                }
                self.push(TokenKind::String, i, j);
                i = j;
                continue;
            }
            if c == b'$' {
                // $$ dollar-quoted $$ / $tag$ … $tag$
                if let Some(tag_end) = dollar_tag(b, i) {
                    let tag = &sql[i..tag_end];
                    let j = sql[tag_end..]
                        .find(tag)
                        .map_or(n, |e| tag_end + e + tag.len());
                    self.push(TokenKind::String, i, j);
                    i = j;
                    continue;
                }
                // $1 positional parameter
                if next.is_ascii_digit() {
                    let mut j = i + 1;
                    while at(j).is_ascii_digit() {
                        j += 1;
                    }
                    self.push(TokenKind::Param, i, j);
                    i = j;
                    continue;
                }
            }
            // :name / @name parameters (not :: casts)
            if ((c == b':' && next != b':' && (i == 0 || b[i - 1] != b':')) || c == b'@')
                && (next.is_ascii_alphabetic() || next == b'_')
            {
                let mut j = i + 2;
                while at(j).is_ascii_alphanumeric() || at(j) == b'_' {
                    j += 1;
                }
                self.push(TokenKind::Param, i, j);
                i = j;
                continue;
            }
            // "quoted" or `quoted` identifier: coloured by what it names.
            if c == b'"' || c == b'`' {
                let close = find_byte(b, i + 1, c);
                let j = close.map_or(n, |e| e + 1);
                let name = &sql[i + 1..close.unwrap_or(n)];
                let kind = with_lower(name, |lower| ident_kind(lower, sql, j, catalog));
                self.push(kind, i, j);
                i = j;
                continue;
            }
            // numbers: 12, 1.5, .5, 1e10, 0x1f
            if c.is_ascii_digit() || (c == b'.' && next.is_ascii_digit()) {
                let j = i + number_len(&b[i..]);
                // Part of an identifier like t1? Then it is plain text.
                let kind = if char_before(sql, i).is_some_and(is_ident) {
                    TokenKind::Plain
                } else {
                    TokenKind::Number
                };
                self.push(kind, i, j);
                i = j;
                continue;
            }
            // words
            let ch = if c < 0x80 {
                c as char
            } else {
                char_at(sql, i).unwrap_or('\0')
            };
            if is_ident_start(ch) {
                let mut j = i + ch.len_utf8();
                while let Some(d) = char_at(sql, j) {
                    if !is_ident(d) {
                        break;
                    }
                    j += d.len_utf8();
                }
                let word = &sql[i..j];
                let prev = self.prev_significant();
                let op_ending = |suffix: &str| {
                    prev.is_some_and(|p| {
                        p.kind == TokenKind::Operator && p.text(sql).ends_with(suffix)
                    })
                };
                let after_dot = op_ending(".");
                // `x::name`: whatever follows a cast is a type, even a user-defined one.
                let after_cast = op_ending("::");
                let kind = with_lower(word, |lower| {
                    if after_cast {
                        TokenKind::Type
                    } else if matches!(lower, "with" | "without")
                        && prev.is_some_and(|p| p.kind == TokenKind::Type)
                        && time_zone_follows(sql, j)
                    {
                        // `timestamp with time zone`: the whole phrase is the type.
                        TokenKind::Type
                    } else if !after_dot && is_keyword(lower) {
                        TokenKind::Keyword
                    } else if !after_dot && is_type(lower) {
                        TokenKind::Type
                    } else {
                        ident_kind(lower, sql, j, catalog)
                    }
                });
                self.push(kind, i, j);
                i = j;
                continue;
            }
            if c < 0x80 && b"=<>!+-*/%|&^~.,;:()[]".contains(&c) {
                let kind = if matches!(c, b'(' | b')' | b',' | b';') {
                    TokenKind::Plain
                } else {
                    TokenKind::Operator
                };
                self.push(kind, i, i + 1);
                i += 1;
                continue;
            }
            self.push(TokenKind::Plain, i, i + ch.len_utf8());
            i += ch.len_utf8();
        }
    }
}

fn find_byte(b: &[u8], from: usize, needle: u8) -> Option<usize> {
    b.get(from..)?
        .iter()
        .position(|&x| x == needle)
        .map(|p| from + p)
}

/// `$tag$` at `i` (`/^\$[A-Za-z_]*\$/`): the byte index after it.
pub(crate) fn dollar_tag(b: &[u8], i: usize) -> Option<usize> {
    if b.get(i) != Some(&b'$') {
        return None;
    }
    let mut j = i + 1;
    while j < b.len() && (b[j].is_ascii_alphabetic() || b[j] == b'_') {
        j += 1;
    }
    (b.get(j) == Some(&b'$')).then_some(j + 1)
}

/// Length of `/^(0x[0-9a-f]+|\d*\.?\d+(e[+-]?\d+)?)/i` at the start of `b`.
fn number_len(b: &[u8]) -> usize {
    let at = |k: usize| b.get(k).copied().unwrap_or(0);
    if at(0) == b'0' && matches!(at(1), b'x' | b'X') && at(2).is_ascii_hexdigit() {
        let mut j = 3;
        while at(j).is_ascii_hexdigit() {
            j += 1;
        }
        return j;
    }
    let digits = |mut j: usize| {
        while at(j).is_ascii_digit() {
            j += 1;
        }
        j
    };
    let a = digits(0);
    let mut j = if at(a) == b'.' && at(a + 1).is_ascii_digit() {
        digits(a + 1)
    } else {
        a
    };
    if matches!(at(j), b'e' | b'E') {
        let sign = usize::from(matches!(at(j + 1), b'+' | b'-'));
        if at(j + 1 + sign).is_ascii_digit() {
            j = digits(j + 1 + sign);
        }
    }
    j
}

/// `/^\s+time\s+zone\b/i` at byte `j`.
fn time_zone_follows(sql: &str, j: usize) -> bool {
    let word = |k: usize, w: &str| {
        let k2 = super::chars::skip_space(sql, k);
        (k2 > k
            && sql
                .get(k2..k2 + w.len())
                .is_some_and(|s| s.eq_ignore_ascii_case(w)))
        .then_some(k2 + w.len())
    };
    let Some(k) = word(j, "time") else {
        return false;
    };
    let Some(k) = word(k, "zone") else {
        return false;
    };
    !sql.as_bytes()
        .get(k)
        .is_some_and(|c| c.is_ascii_alphanumeric() || *c == b'_')
}

/// Function call, schema, table, column, or plain, for an identifier
/// (lower-cased) ending at byte `end`.
fn ident_kind(lower: &str, sql: &str, end: usize, catalog: Option<&Catalog>) -> TokenKind {
    let b = sql.as_bytes();
    let mut k = end;
    while k < b.len() && (b[k] == b' ' || b[k] == b'\t') {
        k += 1;
    }
    if b.get(k) == Some(&b'(') {
        return TokenKind::Function;
    }
    let Some(catalog) = catalog else {
        return TokenKind::Plain;
    };
    let dotted = b.get(end) == Some(&b'.');
    if dotted && catalog.schemas.contains(lower) {
        TokenKind::Schema
    } else if catalog.tables.contains(lower) {
        TokenKind::Table
    } else if catalog.columns.contains(lower) {
        TokenKind::Column
    } else {
        TokenKind::Plain
    }
}

/// An identifier as SQL: bare when it is a plain lower-case name, else "double-quoted".
pub fn quote_ident(name: &str) -> String {
    let b = name.as_bytes();
    let bare = matches!(b.first(), Some(c) if c.is_ascii_lowercase() || *c == b'_')
        && b[1..]
            .iter()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'_' || *c == b'$')
        && !is_keyword(name);
    if bare {
        name.to_owned()
    } else {
        format!("\"{}\"", name.replace('"', "\"\""))
    }
}

/// `schema.table`, each part quoted as needed.
pub fn qualified(schema: &str, table: &str) -> String {
    format!("{}.{}", quote_ident(schema), quote_ident(table))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sql::chars::trim;

    fn catalog() -> Catalog {
        let set = |v: &[&str]| v.iter().map(|s| s.to_string()).collect();
        Catalog {
            schemas: set(&["public"]),
            tables: set(&["users"]),
            columns: set(&["email", "id"]),
        }
    }

    fn kinds(sql: &str) -> Vec<String> {
        tokenize(sql, Some(&catalog()))
            .iter()
            .filter(|t| !trim(t.text(sql)).is_empty())
            .map(|t| format!("{}:{}", t.kind.as_str(), trim(t.text(sql))))
            .collect()
    }

    fn assert_covers(s: &str) {
        let toks = tokenize(s, Some(&catalog()));
        let joined: String = toks.iter().map(|t| t.text(s)).collect();
        assert_eq!(joined, s);
        let mut at = 0;
        for t in &toks {
            assert_eq!(t.start, at, "contiguous in {s:?}");
            assert!(t.end > t.start);
            at = t.end;
        }
        assert_eq!(at, s.len());
    }

    #[test]
    fn covers_the_input_exactly() {
        for s in [
            "select * from \"Clean\".\"T\" where a = 'it''s' -- tail\n/* open",
            "e'x' $$ body $$ $1 :name ::text 1.5e3 t1 [x]",
            "",
        ] {
            assert_covers(s);
        }
    }

    #[test]
    fn classifies_keywords_catalog_names_literals_and_calls() {
        assert_eq!(
            kinds("SELECT id, count(*) FROM public.users WHERE email = 'a' AND id > 10"),
            [
                "keyword:SELECT",
                "column:id",
                "plain:,",
                "function:count",
                "plain:(",
                "operator:*",
                "plain:)",
                "keyword:FROM",
                "schema:public",
                "operator:.",
                "table:users",
                "keyword:WHERE",
                "column:email",
                "operator:=",
                "string:'a'",
                "keyword:AND",
                "column:id",
                "operator:>",
                "number:10",
            ]
        );
    }

    #[test]
    fn treats_words_after_a_dot_as_names_not_keywords() {
        assert_eq!(kinds("u.order"), ["plain:u", "operator:.", "plain:order"]);
    }

    #[test]
    fn handles_comments_casts_params_and_types() {
        assert_eq!(
            kinds("-- hi\nx::timestamptz /* c */ $1 :p"),
            [
                "comment:-- hi",
                "plain:x",
                "operator:::",
                "type:timestamptz",
                "comment:/* c */",
                "param:$1",
                "param::p",
            ]
        );
    }

    #[test]
    fn colours_type_phrases_and_anything_after_a_cast() {
        assert_eq!(
            kinds("created timestamp with time zone, x::regclass, y::mood, z::text[]"),
            [
                "plain:created",
                "type:timestamp",
                "type:with",
                "type:time",
                "type:zone",
                "plain:, x",
                "operator:::",
                "type:regclass",
                "plain:, y",
                "operator:::",
                "type:mood",
                "plain:, z",
                "operator:::",
                "type:text",
                "operator:[]",
            ]
        );
        // `with` that does not continue a type stays a keyword.
        assert_eq!(kinds("with time")[0], "keyword:with");
    }

    #[test]
    fn quote_ident_quotes_only_names_that_need_it() {
        assert_eq!(quote_ident("users"), "users");
        assert_eq!(quote_ident("CleanClaims"), "\"CleanClaims\"");
        assert_eq!(quote_ident("order"), "\"order\"");
        assert_eq!(quote_ident("a\"b"), "\"a\"\"b\"");
        assert_eq!(
            qualified("public", "ClaimNarratives"),
            "public.\"ClaimNarratives\""
        );
    }

    // --- Rust-port specific ---------------------------------------------------

    #[test]
    fn keyword_and_type_lists_are_sorted_for_binary_search() {
        assert!(KEYWORDS.windows(2).all(|w| w[0] < w[1]));
        assert!(TYPES.windows(2).all(|w| w[0] < w[1]));
        assert!(is_keyword("current_timestamp") && is_type("timestamptz"));
    }

    #[test]
    fn uses_byte_offsets_on_non_ascii_text() {
        let s = "select 'héllo', \"Tâble\".x, naïve, 😀x, ü1 from t -- ç\n/* ∑ */ é";
        assert_covers(s);
        assert_eq!(
            kinds(s),
            [
                "keyword:select",
                "string:'héllo'",
                "plain:, \"Tâble\"",
                "operator:.",
                "plain:x, naïve, 😀x, ü1",
                "keyword:from",
                "plain:t",
                "comment:-- ç",
                "comment:/* ∑ */",
                "plain:é",
            ]
        );
        let toks = tokenize(s, None);
        let hello = toks.iter().find(|t| t.kind == TokenKind::String).unwrap();
        assert_eq!(hello.range(), 7..15); // 'héllo' is 7 chars, 8 bytes
        // Non-blank, non-ident characters are single plain chars; NBSP is blank.
        assert_covers("a\u{a0}§b¶");
        assert_eq!(kinds("a\u{a0}b"), ["plain:a\u{a0}b"]);
    }

    #[test]
    fn unterminated_constructs_run_to_the_end() {
        for s in [
            "'abc",
            "\"abc",
            "$tag$ body",
            "/* x",
            "e'",
            "x::",
            "$",
            "@",
            ":",
        ] {
            assert_covers(s);
        }
        assert_eq!(kinds("$q$ a $q$ b"), ["string:$q$ a $q$", "plain:b"]);
        assert_eq!(
            kinds("0x1F 1.5e+3 .5 12."),
            [
                "number:0x1F",
                "number:1.5e+3",
                "number:.5",
                "number:12",
                "operator:."
            ]
        );
    }

    #[test]
    fn tokenizes_a_2000_line_buffer_fast() {
        let chunk = "-- report of the day\n\
            SELECT u.id, u.email, count(o.id) AS n, sum(o.total)::numeric(12, 2)\n\
            FROM public.users u\n\
            LEFT JOIN orders o ON o.user_id = u.id AND o.created_at > now() - interval '7 days'\n\
            WHERE u.email ILIKE '%@example.com' /* inline */ AND u.id <> $1\n\
            GROUP BY u.id, u.email\n\
            HAVING count(o.id) > 10\n\
            ORDER BY n DESC NULLS LAST\n\
            LIMIT 100;\n\
            UPDATE users SET email = lower(email) WHERE id = :id;\n";
        let lines_per_chunk = chunk.lines().count();
        let sql = chunk.repeat(2000 / lines_per_chunk);
        assert_eq!(sql.lines().count(), 2000);
        let cat = catalog();
        let mut out = Vec::new();
        let mut best = std::time::Duration::MAX;
        for _ in 0..5 {
            let t0 = std::time::Instant::now();
            tokenize_into(&sql, Some(&cat), &mut out);
            best = best.min(t0.elapsed());
        }
        assert_eq!(out.last().map(|t| t.end), Some(sql.len()));
        eprintln!(
            "tokenized {} lines ({} bytes, {} tokens) in {best:?}",
            2000,
            sql.len(),
            out.len()
        );
        // Debug builds are far slower; the budget applies to release.
        if !cfg!(debug_assertions) {
            assert!(best.as_millis() < 10, "took {best:?}");
        }
    }
}
