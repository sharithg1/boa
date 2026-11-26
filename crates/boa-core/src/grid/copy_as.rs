//! "Copy as" for a grid selection (`copyAs.ts`, DataGrip's data
//! extractors): the selected rows × columns written as TSV, CSV, a Markdown
//! table, JSON, SQL INSERT / UPDATE statements or an SQL IN list. Values are
//! the grid's text (what Postgres prints), and the column type decides how
//! each is written:
//!
//! ```text
//! NULL     TSV / CSV: empty field (`""` is the empty string, as COPY writes)
//!          Markdown: NULL · JSON: null · SQL: NULL
//! numbers  unquoted in JSON and SQL (when they are plain numeric literals;
//!          NaN / Infinity / money text is quoted)
//! booleans t / f → true / false in JSON, TRUE / FALSE in SQL
//! json     embedded as JSON in JSON output (not a string), a string literal in SQL
//! arrays   a JSON array in JSON output when the literal parses, else text
//! others   strings (bytea stays its \x… text)
//! ```

use super::csv::{CsvDelimiter, CsvOptions, to_csv};
use super::grid_edits::sql_literal;
use super::js::{js_trim, json_string};
use super::types::GridError;
use super::values::{JsonNode, ValueKind, parse_json, value_kind};
use crate::model::Value;
use crate::sql::quote_ident;
use std::fmt;

/// A "Copy as" format.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CopyFormat {
    Tsv,
    TsvHeader,
    Csv,
    Markdown,
    Json,
    SqlInsert,
    SqlUpdate,
    SqlIn,
}

impl CopyFormat {
    /// In menu order.
    pub const ALL: [CopyFormat; 8] = [
        Self::Tsv,
        Self::TsvHeader,
        Self::Csv,
        Self::Markdown,
        Self::Json,
        Self::SqlInsert,
        Self::SqlUpdate,
        Self::SqlIn,
    ];

    /// Menu label: "TSV with header", "SQL IN list"…
    pub fn label(self) -> &'static str {
        match self {
            Self::Tsv => "TSV",
            Self::TsvHeader => "TSV with header",
            Self::Csv => "CSV",
            Self::Markdown => "Markdown table",
            Self::Json => "JSON",
            Self::SqlInsert => "SQL INSERT",
            Self::SqlUpdate => "SQL UPDATE",
            Self::SqlIn => "SQL IN list",
        }
    }

    /// Short name for toasts ("Copied 3 rows as JSON").
    pub fn name(self) -> &'static str {
        match self {
            Self::Tsv | Self::TsvHeader => "TSV",
            Self::Csv => "CSV",
            Self::Markdown => "Markdown",
            Self::Json => "JSON",
            Self::SqlInsert => "INSERT statements",
            Self::SqlUpdate => "UPDATE statements",
            Self::SqlIn => "SQL IN list",
        }
    }
}

/// INSERT target of console results (DataGrip's placeholder).
pub const PLACEHOLDER_TABLE: &str = "MY_TABLE";

/// A selected column: name and driver type name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CopyColumn {
    pub name: String,
    pub ty: String,
}

impl CopyColumn {
    pub fn new(name: impl Into<String>, ty: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            ty: ty.into(),
        }
    }
}

/// For UPDATE: the primary key columns and each selected row's key values
/// (from the whole row, so key columns need not be selected).
#[derive(Debug, Clone, Copy)]
pub struct CopyKey<'a> {
    pub columns: &'a [CopyColumn],
    pub rows: &'a [Vec<Value>],
}

/// What is copied.
#[derive(Clone, Copy)]
pub struct CopySelection<'a> {
    /// The selected columns, in grid order.
    pub columns: &'a [CopyColumn],
    /// The selected rows, values of the selected columns only (as displayed).
    pub rows: &'a [Vec<Value>],
    /// Qualified, quoted table name (table tabs); console results use `MY_TABLE`.
    pub table: Option<&'a str>,
    /// For UPDATE: the primary key.
    pub key: Option<CopyKey<'a>>,
    /// Cells still showing `<default>` (new rows), by (row, column) in the
    /// selection: DEFAULT in SQL, else NULL.
    pub is_default: Option<&'a dyn Fn(usize, usize) -> bool>,
}

impl fmt::Debug for CopySelection<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CopySelection")
            .field("columns", &self.columns)
            .field("rows", &self.rows)
            .field("table", &self.table)
            .field("key", &self.key)
            .field("is_default", &self.is_default.is_some())
            .finish()
    }
}

impl<'a> CopySelection<'a> {
    /// A plain selection: no table, key or defaults.
    pub fn new(columns: &'a [CopyColumn], rows: &'a [Vec<Value>]) -> Self {
        Self {
            columns,
            rows,
            table: None,
            key: None,
            is_default: None,
        }
    }

    fn default_at(&self, r: usize, c: usize) -> bool {
        self.is_default.is_some_and(|f| f(r, c))
    }
}

/// Why `format` can't be used for this selection, or `None` when it can.
pub fn unavailable(format: CopyFormat, s: &CopySelection<'_>) -> Option<&'static str> {
    if format == CopyFormat::SqlIn && s.columns.len() != 1 {
        return Some("Select a single column");
    }
    if format == CopyFormat::SqlUpdate {
        if s.table.is_none() || s.key.is_none_or(|k| k.columns.is_empty()) {
            return Some("Needs a table with a primary key");
        }
        if update_columns(s).is_empty() {
            return Some("Select a column outside the primary key");
        }
    }
    None
}

/// The selection written in `format` (a trailing newline after each line /
/// statement). Fails when [`unavailable`] gives a reason (UPDATE, IN list).
pub fn copy_as(format: CopyFormat, s: &CopySelection<'_>) -> Result<String, GridError> {
    if matches!(format, CopyFormat::SqlUpdate | CopyFormat::SqlIn)
        && let Some(why) = unavailable(format, s)
    {
        return Err(GridError::new(why));
    }
    let names: Vec<&str> = s.columns.iter().map(|c| c.name.as_str()).collect();
    let rows = || -> Vec<Vec<Value>> {
        s.rows
            .iter()
            .enumerate()
            .map(|(i, r)| {
                r.iter()
                    .enumerate()
                    .map(|(c, v)| if s.default_at(i, c) { None } else { v.clone() })
                    .collect()
            })
            .collect()
    };
    let csv = |delimiter, header| to_csv(&names, &rows(), CsvOptions { delimiter, header });
    Ok(match format {
        CopyFormat::Tsv => csv(CsvDelimiter::Tab, false),
        CopyFormat::TsvHeader => csv(CsvDelimiter::Tab, true),
        CopyFormat::Csv => csv(CsvDelimiter::Comma, true),
        CopyFormat::Markdown => markdown(s.columns, &rows()),
        CopyFormat::Json => json(s.columns, &rows()),
        CopyFormat::SqlInsert => inserts(s),
        CopyFormat::SqlUpdate => updates(s),
        CopyFormat::SqlIn => {
            let kind = kind_of(&s.columns[0]);
            let items: Vec<String> = s
                .rows
                .iter()
                .map(|r| sql_value(r.first().cloned().flatten().as_deref(), kind))
                .collect();
            format!("{}\n", items.join(", "))
        }
    })
}

/// TSV of the values only: what ⌘C copies (a single cell: its raw text).
pub fn copy_plain(s: &CopySelection<'_>) -> String {
    if s.rows.len() == 1 && s.columns.len() == 1 {
        return s.rows[0].first().cloned().flatten().unwrap_or_default();
    }
    let names: Vec<&str> = s.columns.iter().map(|c| c.name.as_str()).collect();
    let rows: Vec<Vec<Value>> = s
        .rows
        .iter()
        .enumerate()
        .map(|(i, r)| {
            r.iter()
                .enumerate()
                .map(|(c, v)| if s.default_at(i, c) { None } else { v.clone() })
                .collect()
        })
        .collect();
    to_csv(
        &names,
        &rows,
        CsvOptions {
            delimiter: CsvDelimiter::Tab,
            header: false,
        },
    )
}

fn kind_of(c: &CopyColumn) -> ValueKind {
    value_kind(&c.ty)
}

// ── Markdown ──────────────────────────────────────────────────────────────

fn md_cell(v: Option<&str>) -> String {
    let Some(v) = v else {
        return "NULL".into();
    };
    v.replace('\\', "\\\\")
        .replace('|', "\\|")
        .replace("\r\n", "<br>")
        .replace(['\r', '\n'], "<br>")
}

fn markdown(columns: &[CopyColumn], rows: &[Vec<Value>]) -> String {
    let line = |cells: Vec<String>| format!("| {} |\n", cells.join(" | "));
    let mut out = line(columns.iter().map(|c| md_cell(Some(&c.name))).collect());
    out += &line(
        columns
            .iter()
            .map(|c| {
                if kind_of(c) == ValueKind::Number {
                    "---:".into()
                } else {
                    "---".into()
                }
            })
            .collect(),
    );
    for r in rows {
        out += &line(r.iter().map(|v| md_cell(v.as_deref())).collect());
    }
    out
}

// ── JSON ──────────────────────────────────────────────────────────────────

fn count_digits(b: &[u8], from: usize) -> usize {
    b.get(from..)
        .map_or(0, |s| s.iter().take_while(|c| c.is_ascii_digit()).count())
}

/// The length of an exponent `(?:[eE][+-]?\d+)` at `i`, or 0.
fn exponent(b: &[u8], i: usize) -> usize {
    if !matches!(b.get(i), Some(b'e' | b'E')) {
        return 0;
    }
    let sign = usize::from(matches!(b.get(i + 1), Some(b'+' | b'-')));
    let d = count_digits(b, i + 1 + sign);
    if d == 0 { 0 } else { 1 + sign + d }
}

/// A JSON number: `^-?(?:0|[1-9]\d*)(?:\.\d+)?(?:[eE][+-]?\d+)?$`.
fn is_json_number(v: &str) -> bool {
    let b = v.as_bytes();
    let mut i = usize::from(b.first() == Some(&b'-'));
    match b.get(i) {
        Some(b'0') => i += 1,
        Some(b'1'..=b'9') => i += count_digits(b, i),
        _ => return false,
    }
    if b.get(i) == Some(&b'.') {
        let f = count_digits(b, i + 1);
        if f == 0 {
            return false;
        }
        i += 1 + f;
    }
    i += exponent(b, i);
    i == b.len()
}

/// An SQL numeric literal: `^[+-]?(?:\d+\.?\d*|\.\d+)(?:[eE][+-]?\d+)?$`.
fn is_sql_number(v: &str) -> bool {
    let b = v.as_bytes();
    let mut i = usize::from(matches!(b.first(), Some(b'+' | b'-')));
    let int = count_digits(b, i);
    if int > 0 {
        i += int;
        if b.get(i) == Some(&b'.') {
            i += 1 + count_digits(b, i + 1);
        }
    } else if b.get(i) == Some(&b'.') {
        let f = count_digits(b, i + 1);
        if f == 0 {
            return false;
        }
        i += 1 + f;
    } else {
        return false;
    }
    i += exponent(b, i);
    i == b.len()
}

/// JSON text for a node, indented `depth` levels (literals kept as written).
fn write_node(n: &JsonNode, depth: usize) -> String {
    let pad = "  ".repeat(depth + 1);
    let end = "  ".repeat(depth);
    match n {
        JsonNode::Object(entries) if entries.is_empty() => "{}".into(),
        JsonNode::Object(entries) => {
            let parts: Vec<String> = entries
                .iter()
                .map(|(k, v)| format!("{pad}{}: {}", json_string(k), write_node(v, depth + 1)))
                .collect();
            format!("{{\n{}\n{end}}}", parts.join(",\n"))
        }
        JsonNode::Array(items) if items.is_empty() => "[]".into(),
        JsonNode::Array(items) => {
            let parts: Vec<String> = items
                .iter()
                .map(|v| format!("{pad}{}", write_node(v, depth + 1)))
                .collect();
            format!("[\n{}\n{end}]", parts.join(",\n"))
        }
        JsonNode::String(src) | JsonNode::Number(src) | JsonNode::Literal(src) => src.clone(),
    }
}

/// One value as JSON, per its column kind. `ty` is the column's type name
/// (arrays use it for their element kind); `depth` the indent level.
pub fn json_value(v: Option<&str>, kind: ValueKind, ty: &str, depth: usize) -> String {
    let Some(v) = v else {
        return "null".into();
    };
    match kind {
        ValueKind::Number if is_json_number(v) => v.to_owned(),
        ValueKind::Boolean => match bool_of(v) {
            Some(b) => b.to_string(),
            None => json_string(v),
        },
        ValueKind::Json => match parse_json(v) {
            Ok(node) => write_node(&node, depth),
            Err(_) => json_string(v),
        },
        ValueKind::Array => match parse_pg_array(v) {
            Some(items) => {
                let t = ty.strip_prefix('_').unwrap_or(ty);
                let t = t.strip_suffix("[]").unwrap_or(t);
                write_array(&items, value_kind(t), depth)
            }
            None => json_string(v),
        },
        _ => json_string(v),
    }
}

fn write_array(items: &[PgArrayItem], kind: ValueKind, depth: usize) -> String {
    if items.is_empty() {
        return "[]".into();
    }
    let pad = "  ".repeat(depth + 1);
    let parts: Vec<String> = items
        .iter()
        .map(|x| {
            let body = match x {
                PgArrayItem::Array(sub) => write_array(sub, kind, depth + 1),
                PgArrayItem::Null => json_value(None, kind, "", depth + 1),
                PgArrayItem::Text(t) => json_value(Some(t), kind, "", depth + 1),
            };
            format!("{pad}{body}")
        })
        .collect();
    format!("[\n{}\n{}]", parts.join(",\n"), "  ".repeat(depth))
}

fn json(columns: &[CopyColumn], rows: &[Vec<Value>]) -> String {
    if rows.is_empty() {
        return "[]\n".into();
    }
    let objs: Vec<String> = rows
        .iter()
        .map(|r| {
            let fields: Vec<String> = columns
                .iter()
                .enumerate()
                .map(|(i, c)| {
                    let v = r.get(i).cloned().flatten();
                    format!(
                        "    {}: {}",
                        json_string(&c.name),
                        json_value(v.as_deref(), kind_of(c), &c.ty, 2)
                    )
                })
                .collect();
            if fields.is_empty() {
                "  {}".into()
            } else {
                format!("  {{\n{}\n  }}", fields.join(",\n"))
            }
        })
        .collect();
    format!("[\n{}\n]\n", objs.join(",\n"))
}

fn bool_of(v: &str) -> Option<bool> {
    match v.to_lowercase().as_str() {
        "t" | "true" => Some(true),
        "f" | "false" => Some(false),
        _ => None,
    }
}

// ── Postgres array literals ───────────────────────────────────────────────

/// An element of a Postgres array literal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PgArrayItem {
    Null,
    Text(String),
    Array(Vec<PgArrayItem>),
}

struct ArrayParser {
    s: Vec<char>,
    i: usize,
}

impl ArrayParser {
    fn at(&self) -> Option<char> {
        self.s.get(self.i).copied()
    }

    fn parse(&mut self) -> Option<Vec<PgArrayItem>> {
        if self.at() != Some('{') {
            return None;
        }
        self.i += 1;
        let mut out = Vec::new();
        if self.at() == Some('}') {
            self.i += 1;
            return Some(out);
        }
        loop {
            match self.at() {
                Some('{') => out.push(PgArrayItem::Array(self.parse()?)),
                Some('"') => {
                    self.i += 1;
                    let mut s = String::new();
                    while self.i < self.s.len() && self.s[self.i] != '"' {
                        if self.s[self.i] == '\\' {
                            self.i += 1;
                        }
                        if let Some(c) = self.at() {
                            s.push(c);
                        }
                        self.i += 1;
                    }
                    if self.at() != Some('"') {
                        return None;
                    }
                    self.i += 1;
                    out.push(PgArrayItem::Text(s));
                }
                _ => {
                    let mut s = String::new();
                    while let Some(c) = self.at().filter(|&c| c != ',' && c != '}') {
                        s.push(c);
                        self.i += 1;
                    }
                    let s = js_trim(&s);
                    if s.is_empty() {
                        return None;
                    }
                    out.push(if s.eq_ignore_ascii_case("null") {
                        PgArrayItem::Null
                    } else {
                        PgArrayItem::Text(s.to_owned())
                    });
                }
            }
            match self.at() {
                Some(',') => self.i += 1,
                Some('}') => {
                    self.i += 1;
                    return Some(out);
                }
                _ => return None,
            }
        }
    }
}

/// Parses a Postgres array literal (`{1,2}`, `{"a b",NULL}`,
/// `{{1,2},{3,4}}`); `None` when it isn't one (e.g. a `[0:1]={…}` bounds prefix).
pub fn parse_pg_array(text: &str) -> Option<Vec<PgArrayItem>> {
    let mut p = ArrayParser {
        s: text.chars().collect(),
        i: 0,
    };
    let out = p.parse()?;
    (p.i == p.s.len()).then_some(out)
}

// ── SQL ───────────────────────────────────────────────────────────────────

/// One value as an SQL literal, per its column kind.
pub fn sql_value(v: Option<&str>, kind: ValueKind) -> String {
    let Some(v) = v else {
        return "NULL".into();
    };
    if kind == ValueKind::Number && is_sql_number(v) {
        return v.to_owned();
    }
    if kind == ValueKind::Boolean
        && let Some(b) = bool_of(v)
    {
        return if b { "TRUE" } else { "FALSE" }.into();
    }
    sql_literal(Some(v))
}

fn sql_cell(s: &CopySelection<'_>, r: usize, c: usize) -> String {
    if s.default_at(r, c) {
        return "DEFAULT".into();
    }
    let v = s.rows.get(r).and_then(|row| row.get(c)).cloned().flatten();
    let kind = s.columns.get(c).map_or(ValueKind::Text, kind_of);
    sql_value(v.as_deref(), kind)
}

fn inserts(s: &CopySelection<'_>) -> String {
    let table = s.table.unwrap_or(PLACEHOLDER_TABLE);
    let cols: Vec<String> = s.columns.iter().map(|c| quote_ident(&c.name)).collect();
    let cols = cols.join(", ");
    s.rows
        .iter()
        .enumerate()
        .map(|(i, r)| {
            let values: Vec<String> = (0..r.len()).map(|c| sql_cell(s, i, c)).collect();
            format!(
                "INSERT INTO {table} ({cols}) VALUES ({});\n",
                values.join(", ")
            )
        })
        .collect()
}

/// Selected columns an UPDATE sets: those outside the key.
fn update_columns(s: &CopySelection<'_>) -> Vec<usize> {
    let key: Vec<&str> = s
        .key
        .map(|k| k.columns.iter().map(|c| c.name.as_str()).collect())
        .unwrap_or_default();
    (0..s.columns.len())
        .filter(|&i| !key.contains(&s.columns[i].name.as_str()))
        .collect()
}

fn updates(s: &CopySelection<'_>) -> String {
    let (Some(table), Some(key)) = (s.table, s.key) else {
        return String::new();
    };
    let sets = update_columns(s);
    (0..s.rows.len())
        .map(|i| {
            let set: Vec<String> = sets
                .iter()
                .map(|&c| {
                    format!(
                        "{} = {}",
                        quote_ident(&s.columns[c].name),
                        sql_cell(s, i, c)
                    )
                })
                .collect();
            let conds: Vec<String> = key
                .columns
                .iter()
                .enumerate()
                .map(|(j, k)| {
                    let v = key.rows.get(i).and_then(|r| r.get(j)).cloned().flatten();
                    let name = quote_ident(&k.name);
                    match v {
                        None => format!("{name} IS NULL"),
                        Some(v) => format!("{name} = {}", sql_value(Some(&v), kind_of(k))),
                    }
                })
                .collect();
            format!(
                "UPDATE {table} SET {} WHERE {};\n",
                set.join(", "),
                conds.join(" AND ")
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value as J, json};

    fn v(s: &str) -> Value {
        Some(s.to_owned())
    }

    fn cols() -> Vec<CopyColumn> {
        vec![
            CopyColumn::new("id", "int4"),
            CopyColumn::new("name", "text"),
            CopyColumn::new("active", "bool"),
            CopyColumn::new("meta", "jsonb"),
        ]
    }

    fn sel_rows() -> Vec<Vec<Value>> {
        vec![
            vec![
                v("1"),
                v("O'Brien"),
                v("t"),
                v(r#"{"a": 1, "big": 12345678901234567890}"#),
            ],
            vec![v("2"), None, v("f"), None],
        ]
    }

    fn copy(format: CopyFormat, s: &CopySelection<'_>) -> String {
        copy_as(format, s).unwrap()
    }

    fn parse(text: &str) -> J {
        serde_json::from_str(text).unwrap()
    }

    // ── TSV / CSV ──

    #[test]
    fn writes_tsv_without_and_with_a_header() {
        let (c, r) = (cols(), sel_rows());
        let sel = CopySelection::new(&c, &r);
        assert_eq!(
            copy(CopyFormat::Tsv, &sel),
            "1\tO'Brien\tt\t\"{\"\"a\"\": 1, \"\"big\"\": 12345678901234567890}\"\n2\t\tf\t\n"
        );
        assert_eq!(
            copy(CopyFormat::TsvHeader, &sel).split('\n').next(),
            Some("id\tname\tactive\tmeta")
        );
    }

    #[test]
    fn quotes_per_rfc_4180_in_csv() {
        let c = vec![CopyColumn::new("a", "text"), CopyColumn::new("b", "text")];
        let r = vec![
            vec![v("x,y"), v("say \"hi\"")],
            vec![v(""), None],
            vec![v("l1\nl2"), v("plain")],
        ];
        assert_eq!(
            copy(CopyFormat::Csv, &CopySelection::new(&c, &r)),
            "a,b\n\"x,y\",\"say \"\"hi\"\"\"\n\"\",\n\"l1\nl2\",plain\n"
        );
    }

    #[test]
    fn copies_a_single_cell_raw_and_several_as_tsv() {
        let c = vec![cols()[1].clone()];
        assert_eq!(
            copy_plain(&CopySelection::new(&c, &[vec![v("a\tb")]])),
            "a\tb"
        );
        assert_eq!(copy_plain(&CopySelection::new(&c, &[vec![None]])), "");
        assert_eq!(
            copy_plain(&CopySelection::new(&c, &[vec![v("a")], vec![v("b")]])),
            "a\nb\n"
        );
    }

    // ── Markdown ──

    #[test]
    fn writes_a_markdown_table() {
        let c = vec![
            CopyColumn::new("id", "int8"),
            CopyColumn::new("note", "text"),
        ];
        let r = vec![
            vec![v("1"), v("a | b")],
            vec![v("2"), v("line1\nline2")],
            vec![v("3"), None],
            vec![v("4"), v("back\\slash")],
        ];
        assert_eq!(
            copy(CopyFormat::Markdown, &CopySelection::new(&c, &r)),
            [
                "| id | note |",
                "| ---: | --- |",
                "| 1 | a \\| b |",
                "| 2 | line1<br>line2 |",
                "| 3 | NULL |",
                "| 4 | back\\\\slash |",
                "",
            ]
            .join("\n")
        );
    }

    // ── JSON ──

    #[test]
    fn writes_an_array_of_objects_with_typed_values() {
        let (c, r) = (cols(), sel_rows());
        assert_eq!(
            copy(CopyFormat::Json, &CopySelection::new(&c, &r)),
            [
                "[",
                "  {",
                "    \"id\": 1,",
                "    \"name\": \"O'Brien\",",
                "    \"active\": true,",
                "    \"meta\": {",
                "      \"a\": 1,",
                "      \"big\": 12345678901234567890",
                "    }",
                "  },",
                "  {",
                "    \"id\": 2,",
                "    \"name\": null,",
                "    \"active\": false,",
                "    \"meta\": null",
                "  }",
                "]",
                "",
            ]
            .join("\n")
        );
    }

    #[test]
    fn json_parses_with_the_right_types() {
        let (c, r) = (cols(), sel_rows());
        let out = parse(&copy(CopyFormat::Json, &CopySelection::new(&c, &r)));
        assert_eq!(out[0]["id"], json!(1));
        assert_eq!(out[0]["name"], json!("O'Brien"));
        assert_eq!(out[0]["active"], json!(true));
        assert_eq!(out[0]["meta"]["a"], json!(1));
        assert_eq!(
            out[1],
            json!({"id": 2, "name": null, "active": false, "meta": null})
        );
    }

    #[test]
    fn writes_an_empty_selection_as_empty_array() {
        let c = cols();
        assert_eq!(copy(CopyFormat::Json, &CopySelection::new(&c, &[])), "[]\n");
    }

    #[test]
    fn quotes_numbers_that_are_not_json_numbers_and_invalid_json() {
        let jv = |s: &str, k| json_value(Some(s), k, "", 0);
        assert_eq!(jv("NaN", ValueKind::Number), "\"NaN\"");
        assert_eq!(jv("$1,200.00", ValueKind::Number), "\"$1,200.00\"");
        assert_eq!(jv("007", ValueKind::Number), "\"007\"");
        assert_eq!(jv("-1.5e3", ValueKind::Number), "-1.5e3");
        assert_eq!(jv("{not json", ValueKind::Json), "\"{not json\"");
        assert_eq!(jv("maybe", ValueKind::Boolean), "\"maybe\"");
    }

    #[test]
    fn escapes_strings_and_keeps_bytea_and_dates_as_text() {
        let jv = |s: &str, k| json_value(Some(s), k, "", 0);
        assert_eq!(jv("a\"b\\c\nd", ValueKind::Text), r#""a\"b\\c\nd""#);
        assert_eq!(jv("\\x0102", ValueKind::Binary), r#""\\x0102""#);
        assert_eq!(
            jv("2024-01-02 03:04:05+00", ValueKind::Temporal),
            "\"2024-01-02 03:04:05+00\""
        );
    }

    #[test]
    fn writes_postgres_arrays_as_json_arrays_of_the_element_type() {
        let arr = |s: &str, ty: &str| json_value(Some(s), ValueKind::Array, ty, 0);
        assert_eq!(parse(&arr("{1,2,NULL}", "_int4")), json!([1, 2, null]));
        assert_eq!(
            parse(&arr(r#"{"a b","c\"d",NULL,"NULL"}"#, "_text")),
            json!(["a b", "c\"d", null, "NULL"])
        );
        assert_eq!(
            parse(&arr("{{1,2},{3,4}}", "int4[]")),
            json!([[1, 2], [3, 4]])
        );
        assert_eq!(parse(&arr("{t,f}", "_bool")), json!([true, false]));
        assert_eq!(arr("{}", "_int4"), "[]");
        assert_eq!(arr("[0:1]={1,2}", "_int4"), "\"[0:1]={1,2}\"");
    }

    #[test]
    fn parse_pg_array_rejects_text_that_is_not_an_array_literal() {
        assert_eq!(parse_pg_array("{1,2"), None);
        assert_eq!(parse_pg_array("{1,2}x"), None);
        assert_eq!(parse_pg_array("1,2"), None);
        assert_eq!(parse_pg_array("{\"open}"), None);
    }

    // ── SQL ──

    #[test]
    fn writes_sql_literals_per_type() {
        assert_eq!(sql_value(None, ValueKind::Text), "NULL");
        assert_eq!(sql_value(Some("42"), ValueKind::Number), "42");
        assert_eq!(sql_value(Some("-0.5"), ValueKind::Number), "-0.5");
        assert_eq!(sql_value(Some("NaN"), ValueKind::Number), "'NaN'");
        assert_eq!(sql_value(Some("t"), ValueKind::Boolean), "TRUE");
        assert_eq!(sql_value(Some("f"), ValueKind::Boolean), "FALSE");
        assert_eq!(sql_value(Some("it's"), ValueKind::Text), "'it''s'");
        assert_eq!(
            sql_value(Some(r#"{"a": 1}"#), ValueKind::Json),
            r#"'{"a": 1}'"#
        );
        assert_eq!(sql_value(Some("{1,2}"), ValueKind::Array), "'{1,2}'");
        assert_eq!(sql_value(Some("\\x01"), ValueKind::Binary), "E'\\\\x01'");
    }

    #[test]
    fn writes_one_insert_per_row_into_my_table_for_console_results() {
        let (c, r) = (cols(), sel_rows());
        assert_eq!(
            copy(CopyFormat::SqlInsert, &CopySelection::new(&c, &r)),
            concat!(
                r#"INSERT INTO MY_TABLE (id, name, active, meta) VALUES (1, 'O''Brien', TRUE, '{"a": 1, "big": 12345678901234567890}');"#,
                "\n",
                "INSERT INTO MY_TABLE (id, name, active, meta) VALUES (2, NULL, FALSE, NULL);\n"
            )
        );
    }

    #[test]
    fn uses_the_table_name_quotes_identifiers_and_writes_default() {
        let c = vec![
            CopyColumn::new("id", "int4"),
            CopyColumn::new("User Name", "text"),
        ];
        let r = vec![vec![None, v("x")]];
        let is_default = |r: usize, c: usize| r == 0 && c == 0;
        let s = CopySelection {
            table: Some("public.\"Order\""),
            is_default: Some(&is_default),
            ..CopySelection::new(&c, &r)
        };
        assert_eq!(
            copy(CopyFormat::SqlInsert, &s),
            "INSERT INTO public.\"Order\" (id, \"User Name\") VALUES (DEFAULT, 'x');\n"
        );
        // Other formats show the default as NULL.
        assert!(copy(CopyFormat::Json, &s).contains("\"id\": null"));
    }

    #[test]
    fn writes_updates_by_primary_key() {
        let c = vec![
            CopyColumn::new("id", "int4"),
            CopyColumn::new("name", "text"),
            CopyColumn::new("active", "bool"),
        ];
        let r = vec![
            vec![v("1"), v("O'Brien"), v("t")],
            vec![v("2"), None, v("f")],
        ];
        let kc = vec![
            CopyColumn::new("id", "int4"),
            CopyColumn::new("tenant", "text"),
        ];
        let kr = vec![vec![v("1"), v("acme")], vec![v("2"), None]];
        let s = CopySelection {
            table: Some("public.users"),
            key: Some(CopyKey {
                columns: &kc,
                rows: &kr,
            }),
            ..CopySelection::new(&c, &r)
        };
        assert_eq!(
            copy(CopyFormat::SqlUpdate, &s),
            "UPDATE public.users SET name = 'O''Brien', active = TRUE WHERE id = 1 AND tenant = 'acme';\n\
             UPDATE public.users SET name = NULL, active = FALSE WHERE id = 2 AND tenant IS NULL;\n"
        );
    }

    #[test]
    fn makes_update_unavailable_without_a_table_a_key_or_a_non_key_column() {
        let (c, r) = (cols(), sel_rows());
        let sel = CopySelection::new(&c, &r);
        let kc = vec![CopyColumn::new("id", "int4")];
        let kr = vec![vec![v("1")]];
        let key = Some(CopyKey {
            columns: &kc,
            rows: &kr,
        });
        let u = |s: &CopySelection<'_>| unavailable(CopyFormat::SqlUpdate, s);
        assert!(u(&sel).is_some());
        assert!(u(&CopySelection { key, ..sel }).is_some());
        assert!(
            u(&CopySelection {
                table: Some("t"),
                ..sel
            })
            .is_some()
        );
        assert!(
            u(&CopySelection {
                table: Some("t"),
                key,
                ..sel
            })
            .is_none()
        );
        let only_id = vec![cols()[0].clone()];
        let one = vec![vec![v("1")]];
        assert!(
            u(&CopySelection {
                table: Some("t"),
                key,
                ..CopySelection::new(&only_id, &one)
            })
            .is_some()
        );
        assert!(copy_as(CopyFormat::SqlUpdate, &sel).is_err());
    }

    #[test]
    fn writes_an_in_list_for_a_single_column_only() {
        let c = vec![CopyColumn::new("name", "text")];
        let r = vec![vec![v("a")], vec![v("b'c")], vec![None]];
        let s = CopySelection::new(&c, &r);
        assert_eq!(copy(CopyFormat::SqlIn, &s), "'a', 'b''c', NULL\n");
        let ids = vec![CopyColumn::new("id", "int8")];
        let ir = vec![vec![v("1")], vec![v("2")]];
        assert_eq!(
            copy(CopyFormat::SqlIn, &CopySelection::new(&ids, &ir)),
            "1, 2\n"
        );
        assert_eq!(unavailable(CopyFormat::SqlIn, &s), None);
        let (c2, r2) = (cols(), sel_rows());
        assert_eq!(
            unavailable(CopyFormat::SqlIn, &CopySelection::new(&c2, &r2)),
            Some("Select a single column")
        );
    }

    #[test]
    fn offers_every_format_for_a_plain_selection_except_update_and_in() {
        let (c, r) = (cols(), sel_rows());
        let sel = CopySelection::new(&c, &r);
        let offered: Vec<CopyFormat> = CopyFormat::ALL
            .into_iter()
            .filter(|f| unavailable(*f, &sel).is_none())
            .collect();
        assert_eq!(
            offered,
            [
                CopyFormat::Tsv,
                CopyFormat::TsvHeader,
                CopyFormat::Csv,
                CopyFormat::Markdown,
                CopyFormat::Json,
                CopyFormat::SqlInsert,
            ]
        );
    }
}
