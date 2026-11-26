//! CSV writer for results built on the UI side (`csv.ts`): rows already
//! loaded (a page copied to the clipboard, or a result that must not be
//! re-run). It writes what Postgres's `COPY … TO STDOUT WITH (FORMAT csv)`
//! writes for the same rows, so both export paths give identical files:
//! fields quoted only when they hold the delimiter, a quote, CR or LF
//! (quotes doubled, RFC 4180), NULL as an empty unquoted field, an empty
//! string as `""`, `\n` line ends.

use super::js::is_js_space;
use crate::model::Value;

/// The field separator.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum CsvDelimiter {
    #[default]
    Comma,
    Tab,
    Semicolon,
}

impl CsvDelimiter {
    /// In menu order.
    pub const ALL: [CsvDelimiter; 3] = [Self::Comma, Self::Tab, Self::Semicolon];

    pub fn as_char(self) -> char {
        match self {
            Self::Comma => ',',
            Self::Tab => '\t',
            Self::Semicolon => ';',
        }
    }

    /// Menu label: "Comma", "Tab", "Semicolon".
    pub fn label(self) -> &'static str {
        match self {
            Self::Comma => "Comma",
            Self::Tab => "Tab",
            Self::Semicolon => "Semicolon",
        }
    }
}

/// CSV export options.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CsvOptions {
    pub delimiter: CsvDelimiter,
    /// A first row of column names.
    pub header: bool,
}

impl Default for CsvOptions {
    fn default() -> Self {
        Self {
            delimiter: CsvDelimiter::Comma,
            header: true,
        }
    }
}

/// One field. `single`: the only column (Postgres then quotes `\.`, its
/// end-of-data marker).
pub fn csv_field(value: Option<&str>, delimiter: CsvDelimiter, single: bool) -> String {
    let Some(v) = value else {
        return String::new();
    };
    let quote = v.is_empty()
        || v.contains(delimiter.as_char())
        || v.contains(['"', '\r', '\n'])
        || (single && v == "\\.");
    if quote {
        format!("\"{}\"", v.replace('"', "\"\""))
    } else {
        v.to_owned()
    }
}

fn push_line<'a>(
    out: &mut String,
    values: impl ExactSizeIterator<Item = Option<&'a str>>,
    delimiter: CsvDelimiter,
) {
    let single = values.len() == 1;
    for (i, v) in values.enumerate() {
        if i > 0 {
            out.push(delimiter.as_char());
        }
        out.push_str(&csv_field(v, delimiter, single));
    }
    out.push('\n');
}

/// One line, with its `\n`.
pub fn csv_line(values: &[Value], delimiter: CsvDelimiter) -> String {
    let mut out = String::new();
    push_line(&mut out, values.iter().map(Option::as_deref), delimiter);
    out
}

/// The whole file: an optional header row of column names, then the rows.
pub fn to_csv<S: AsRef<str>>(columns: &[S], rows: &[Vec<Value>], options: CsvOptions) -> String {
    let mut out = String::new();
    if options.header {
        push_line(
            &mut out,
            columns.iter().map(|c| Some(c.as_ref())),
            options.delimiter,
        );
    }
    for r in rows {
        push_line(&mut out, r.iter().map(Option::as_deref), options.delimiter);
    }
    out
}

/// `"(?:[^"]|"")+"` or `[\w$]+` at `i`: the end positions it can match at,
/// longest first.
fn ident_ends(s: &[char], i: usize) -> Vec<usize> {
    let mut ends = Vec::new();
    match s.get(i) {
        Some('"') => {
            let mut k = i + 1;
            let mut unit_ends = Vec::new();
            while k < s.len() {
                if s[k] != '"' {
                    k += 1;
                } else if s.get(k + 1) == Some(&'"') {
                    k += 2;
                } else {
                    break;
                }
                unit_ends.push(k);
            }
            for &e in unit_ends.iter().rev() {
                if s.get(e) == Some(&'"') {
                    ends.push(e + 1);
                }
            }
        }
        Some(_) => {
            let n = s[i..]
                .iter()
                .take_while(|&&c| c.is_ascii_alphanumeric() || c == '_' || c == '$')
                .count();
            ends.extend((1..=n).rev().map(|l| i + l));
        }
        None => {}
    }
    ends
}

fn skip_ws(s: &[char], mut i: usize) -> usize {
    while i < s.len() && is_js_space(s[i]) {
        i += 1;
    }
    i
}

/// The table name of `\bfrom\s+(?:IDENT\s*\.\s*)?(IDENT)` in `sql`, as typed.
fn from_table(sql: &str) -> Option<String> {
    let s: Vec<char> = sql.chars().collect();
    let word = |c: char| c.is_ascii_alphanumeric() || c == '_';
    for at in 0..s.len() {
        let is_from = s.len() >= at + 4
            && s[at..at + 4]
                .iter()
                .zip("from".chars())
                .all(|(a, b)| a.to_ascii_lowercase() == b);
        if !is_from || (at > 0 && word(s[at - 1])) {
            continue;
        }
        let p = skip_ws(&s, at + 4);
        if p == at + 4 {
            continue;
        }
        // With a schema first, then without.
        for e1 in ident_ends(&s, p) {
            let mut q = skip_ws(&s, e1);
            if s.get(q) != Some(&'.') {
                continue;
            }
            q = skip_ws(&s, q + 1);
            if let Some(&e2) = ident_ends(&s, q).first() {
                return Some(s[q..e2].iter().collect());
            }
        }
        if let Some(&e) = ident_ends(&s, p).first() {
            return Some(s[p..e].iter().collect());
        }
    }
    None
}

/// Default file name: the table a query reads from (`orders` for
/// `select … from public.orders`), else `fallback` (the console name), made
/// safe for a file system; `.tsv` for tab-separated output.
pub fn csv_file_name(sql: &str, fallback: &str, delimiter: CsvDelimiter) -> String {
    let raw = match from_table(sql) {
        Some(t) if t.starts_with('"') => t[1..t.len() - 1].replace("\"\"", "\""),
        Some(t) => t.to_lowercase(),
        None => fallback.to_owned(),
    };
    let mut safe = String::with_capacity(raw.len());
    let mut in_run = false;
    for c in raw.chars() {
        let bad =
            (c as u32) < 0x20 || matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|');
        if bad {
            if !in_run {
                safe.push('_');
            }
        } else {
            safe.push(c);
        }
        in_run = bad;
    }
    let base = safe.trim_matches(|c: char| c == '.' || is_js_space(c));
    let base = if base.is_empty() { "result" } else { base };
    let ext = if delimiter == CsvDelimiter::Tab {
        "tsv"
    } else {
        "csv"
    };
    format!("{base}.{ext}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use CsvDelimiter::{Comma, Semicolon, Tab};

    fn f(v: &str, d: CsvDelimiter) -> String {
        csv_field(Some(v), d, false)
    }

    #[test]
    fn csv_field_leaves_plain_values_unquoted() {
        assert_eq!(f("abc", Comma), "abc");
        assert_eq!(f(" padded ", Comma), " padded ");
        assert_eq!(f("a;b", Comma), "a;b");
    }

    #[test]
    fn csv_field_quotes_delimiters_quotes_and_line_breaks() {
        assert_eq!(f("a,b", Comma), "\"a,b\"");
        assert_eq!(f("a;b", Semicolon), "\"a;b\"");
        assert_eq!(f("a\tb", Tab), "\"a\tb\"");
        assert_eq!(f("say \"hi\"", Comma), "\"say \"\"hi\"\"\"");
        assert_eq!(f("l1\nl2", Comma), "\"l1\nl2\"");
        assert_eq!(f("l1\r\nl2", Comma), "\"l1\r\nl2\"");
    }

    #[test]
    fn csv_field_writes_null_empty_and_empty_string_quoted() {
        assert_eq!(csv_field(None, Comma, false), "");
        assert_eq!(f("", Comma), "\"\"");
    }

    #[test]
    fn csv_field_quotes_end_of_data_marker_only_in_a_single_column() {
        assert_eq!(csv_field(Some("\\."), Comma, true), "\"\\.\"");
        assert_eq!(csv_field(Some("\\."), Comma, false), "\\.");
    }

    fn rows() -> Vec<Vec<Value>> {
        vec![
            vec![Some("1".into()), Some("a,b".into()), Some("".into())],
            vec![Some("2".into()), None, None],
        ]
    }
    const COLUMNS: [&str; 3] = ["id", "note", "e"];

    fn opts(delimiter: CsvDelimiter, header: bool) -> CsvOptions {
        CsvOptions { delimiter, header }
    }

    #[test]
    fn to_csv_writes_a_header_row_and_newlines() {
        assert_eq!(
            to_csv(&COLUMNS, &rows(), opts(Comma, true)),
            "id,note,e\n1,\"a,b\",\"\"\n2,,\n"
        );
    }

    #[test]
    fn to_csv_can_leave_the_header_out_and_use_another_delimiter() {
        assert_eq!(
            to_csv(&COLUMNS, &rows(), opts(Tab, false)),
            "1\ta,b\t\"\"\n2\t\t\n"
        );
        assert_eq!(
            to_csv(&COLUMNS, &rows(), opts(Semicolon, false)),
            "1;a,b;\"\"\n2;;\n"
        );
    }

    #[test]
    fn to_csv_quotes_header_names_like_values() {
        assert_eq!(
            to_csv(&["a,b", ""], &[], opts(Comma, true)),
            "\"a,b\",\"\"\n"
        );
    }

    #[test]
    fn to_csv_is_empty_without_rows_or_header() {
        assert_eq!(to_csv(&COLUMNS, &[], opts(Comma, false)), "");
    }

    #[test]
    fn csv_file_name_names_the_file_after_the_table_read() {
        assert_eq!(
            csv_file_name("select * from public.orders where x", "console_1", Comma),
            "orders.csv"
        );
        assert_eq!(
            csv_file_name("SELECT * FROM Orders", "c", Comma),
            "orders.csv"
        );
        assert_eq!(
            csv_file_name("select * from \"Sales\".\"My Table\"", "c", Tab),
            "My Table.tsv"
        );
        // Regex backtracking cases.
        assert_eq!(csv_file_name("select * from a.", "c", Comma), "a.csv");
        assert_eq!(csv_file_name("select * from \"a\"\"", "c", Comma), "a.csv");
        assert_eq!(csv_file_name("select x_from from t", "c", Comma), "t.csv");
    }

    #[test]
    fn csv_file_name_falls_back_to_the_console_name_made_file_safe() {
        assert_eq!(
            csv_file_name("select 1", "console_1", Comma),
            "console_1.csv"
        );
        assert_eq!(csv_file_name("select 1", "a/b:c", Comma), "a_b_c.csv");
        assert_eq!(csv_file_name("select 1", "..", Comma), "result.csv");
        assert_eq!(csv_file_name("select 1", "a//b", Comma), "a_b.csv");
    }
}
