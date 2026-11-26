//! Cell values in the result grid (`values.ts`): what kind of value a column
//! holds (from the type name the driver reports, e.g. Postgres' `int4`,
//! `timestamptz`, `_text`), and a small JSON tokenizer / parser for json and
//! jsonb cells and the JSON viewer. The parser keeps number and string
//! literals as their source text, so 64-bit ids and exact decimals survive.

use super::js::{is_js_space, is_word, js_trim};
use std::fmt;

/// How a column's values are shown and written.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ValueKind {
    Number,
    Boolean,
    Json,
    Temporal,
    Uuid,
    Binary,
    Array,
    Text,
}

/// The text colour role of each kind (the UI's `VALUE_CLASS`); JSON is
/// coloured token by token instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ValueStyle {
    /// `text-sql-number`
    Number,
    /// `text-sql-keyword`
    Keyword,
    /// `text-sql-param`
    Param,
    /// `text-sql-table`
    Table,
    /// `text-dim`
    Dim,
    /// `text-soft`
    Soft,
}

impl ValueKind {
    /// The colour role cells of this kind use.
    pub fn style(self) -> ValueStyle {
        match self {
            Self::Number => ValueStyle::Number,
            Self::Boolean => ValueStyle::Keyword,
            Self::Temporal => ValueStyle::Param,
            Self::Uuid => ValueStyle::Table,
            Self::Binary => ValueStyle::Dim,
            Self::Array | Self::Json | Self::Text => ValueStyle::Soft,
        }
    }
}

const TEMPORAL: &[&str] = &[
    "date",
    "time",
    "timetz",
    "timestamp",
    "timestamptz",
    "datetime",
    "datetime0",
    "datetime1",
    "datetime2",
    "datetime3",
    "datetime4",
    "datetime5",
    "datetime6",
    "datetime7",
    "datetime8",
    "datetime9",
    "smalldatetime",
    "interval",
    "year",
];

const NUMERIC: &[&str] = &[
    "int",
    "int2",
    "int4",
    "int8",
    "integer",
    "smallint",
    "bigint",
    "tinyint",
    "mediumint",
    "serial",
    "serial2",
    "serial4",
    "serial8",
    "bigserial",
    "smallserial",
    "oid",
    "decimal",
    "numeric",
    "dec",
    "real",
    "double",
    "float",
    "float4",
    "float8",
    "money",
];

/// `t` starts with one of `words`, followed by a word boundary (`^(…)\b`).
fn starts_with_word(t: &str, words: &[&str]) -> bool {
    words.iter().any(|w| {
        t.strip_prefix(w)
            .is_some_and(|rest| !rest.chars().next().is_some_and(is_word))
    })
}

/// The kind of value a column of driver type `ty` holds.
pub fn value_kind(ty: &str) -> ValueKind {
    let t = ty.to_lowercase();
    let t = js_trim(&t);
    if t.starts_with('_') || t.ends_with("[]") {
        return ValueKind::Array;
    }
    match t {
        "json" | "jsonb" => return ValueKind::Json,
        "bool" | "boolean" => return ValueKind::Boolean,
        "uuid" => return ValueKind::Uuid,
        _ => {}
    }
    if t == "bytea" || t.contains("blob") || t.contains("binary") {
        return ValueKind::Binary;
    }
    if starts_with_word(t, TEMPORAL) {
        return ValueKind::Temporal;
    }
    if starts_with_word(t, NUMERIC) {
        return ValueKind::Number;
    }
    ValueKind::Text
}

// ── JSON tokens ─────────────────────────────────────────

/// What a JSON token is, for colouring (the UI's `JSON_CLASS`: key → column,
/// string → string, number → number, literal → keyword, punct → operator).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum JsonTokenKind {
    Key,
    String,
    Number,
    Literal,
    Punct,
    Plain,
}

/// A run of JSON text of one kind.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JsonToken {
    pub kind: JsonTokenKind,
    pub text: String,
}

/// `^"(?:[^"\\]|\\.)*"?` at the start of `rest` (which starts with `"`): its length.
fn match_string(rest: &str) -> usize {
    let mut it = rest.char_indices().skip(1).peekable();
    while let Some((k, c)) = it.next() {
        match c {
            '"' => return k + 1,
            '\\' => match it.peek() {
                // `.` matches anything but a line terminator.
                Some(&(_, n)) if !matches!(n, '\n' | '\r' | '\u{2028}' | '\u{2029}') => {
                    it.next();
                }
                _ => return k,
            },
            _ => {}
        }
    }
    rest.len()
}

fn digits(b: &[u8], from: usize) -> usize {
    b[from..].iter().take_while(|c| c.is_ascii_digit()).count()
}

/// `^-?\d+(?:\.\d+)?(?:[eE][+-]?\d+)?`: its length.
fn match_number(rest: &str) -> Option<usize> {
    let b = rest.as_bytes();
    let mut i = usize::from(b.first() == Some(&b'-'));
    let n = digits(b, i);
    if n == 0 {
        return None;
    }
    i += n;
    if b.get(i) == Some(&b'.') {
        let f = digits(b, i + 1);
        if f > 0 {
            i += 1 + f;
        }
    }
    if matches!(b.get(i), Some(b'e' | b'E')) {
        let mut j = i + 1;
        if matches!(b.get(j), Some(b'+' | b'-')) {
            j += 1;
        }
        let e = digits(b, j);
        if e > 0 {
            i = j + e;
        }
    }
    Some(i)
}

/// `^(?:true|false|null)\b`: its length.
fn match_literal(rest: &str) -> Option<usize> {
    ["true", "false", "null"].iter().find_map(|w| {
        let after = rest.strip_prefix(w)?;
        (!after.chars().next().is_some_and(is_word)).then_some(w.len())
    })
}

/// Tokens covering `text` exactly (joined, they give the input back).
/// Lenient: anything that is not JSON comes out as plain text. A string is
/// a key when the next non-blank character is a colon.
pub fn json_tokens(text: &str) -> Vec<JsonToken> {
    let mut out: Vec<JsonToken> = Vec::new();
    let mut i = 0;
    while i < text.len() {
        let rest = &text[i..];
        let Some(c) = rest.chars().next() else { break };
        if c == '"' {
            let n = match_string(rest);
            let after = text[i + n..].trim_start_matches(is_js_space);
            let kind = if after.starts_with(':') {
                JsonTokenKind::Key
            } else {
                JsonTokenKind::String
            };
            out.push(JsonToken {
                kind,
                text: rest[..n].to_owned(),
            });
            i += n;
        } else if let Some(n) = match_number(rest) {
            out.push(JsonToken {
                kind: JsonTokenKind::Number,
                text: rest[..n].to_owned(),
            });
            i += n;
        } else if let Some(n) = match_literal(rest) {
            out.push(JsonToken {
                kind: JsonTokenKind::Literal,
                text: rest[..n].to_owned(),
            });
            i += n;
        } else {
            let kind = if "{}[]:,".contains(c) {
                JsonTokenKind::Punct
            } else {
                JsonTokenKind::Plain
            };
            match out.last_mut() {
                Some(last) if last.kind == kind => last.text.push(c),
                _ => out.push(JsonToken {
                    kind,
                    text: c.to_string(),
                }),
            }
            i += c.len_utf8();
        }
    }
    out
}

/// Pretty-printed with two-space indents, literals kept exactly as written.
pub fn format_json(text: &str) -> String {
    // Split merged punctuation and drop whitespace between tokens.
    let toks: Vec<JsonToken> = json_tokens(text)
        .into_iter()
        .flat_map(|t| {
            if t.kind == JsonTokenKind::Punct {
                t.text
                    .chars()
                    .map(|ch| JsonToken {
                        kind: JsonTokenKind::Punct,
                        text: ch.to_string(),
                    })
                    .collect()
            } else {
                vec![t]
            }
        })
        .collect();
    let nl = |depth: usize| format!("\n{}", "  ".repeat(depth));
    let mut out = String::new();
    let mut depth = 0usize;
    let mut k = 0;
    while k < toks.len() {
        let t = &toks[k];
        if t.kind == JsonTokenKind::Plain {
            out.push_str(js_trim(&t.text));
            k += 1;
            continue;
        }
        let next = (k + 1..toks.len())
            .find(|&x| toks[x].kind != JsonTokenKind::Plain || !js_trim(&toks[x].text).is_empty());
        match t.text.as_str() {
            open @ ("{" | "[") => {
                let close = if open == "{" { "}" } else { "]" };
                if let Some(n) = next.filter(|&n| toks[n].text == close) {
                    out.push_str(open);
                    out.push_str(close);
                    k = n + 1;
                    continue;
                }
                depth += 1;
                out.push_str(open);
                out.push_str(&nl(depth));
            }
            close @ ("}" | "]") => {
                depth = depth.saturating_sub(1);
                out.push_str(&nl(depth));
                out.push_str(close);
            }
            "," => {
                out.push(',');
                out.push_str(&nl(depth));
            }
            ":" => out.push_str(": "),
            other => out.push_str(other),
        }
        k += 1;
    }
    out
}

// ── JSON tree ───────────────────────────────────────────

/// A parsed JSON value. Scalars keep their source text: strings keep their
/// quotes and escapes, numbers their exact digits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JsonNode {
    /// Entries in source order, keys decoded.
    Object(Vec<(String, JsonNode)>),
    Array(Vec<JsonNode>),
    String(String),
    Number(String),
    /// `true`, `false` or `null`.
    Literal(String),
}

/// Bad JSON: what was expected, and where (in UTF-16 code units, like the UI).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JsonError {
    pub message: String,
    pub position: usize,
}

impl fmt::Display for JsonError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} at position {}", self.message, self.position)
    }
}

impl std::error::Error for JsonError {}

/// Decodes a JSON string literal (with its quotes), as `JSON.parse` would.
fn decode_json_string(src: &str) -> Option<String> {
    let inner = src.strip_prefix('"')?.strip_suffix('"')?;
    let mut out = String::with_capacity(inner.len());
    let mut it = inner.chars();
    while let Some(c) = it.next() {
        match c {
            '"' => return None,
            c if (c as u32) < 0x20 => return None,
            '\\' => {
                let e = it.next()?;
                match e {
                    '"' | '\\' | '/' => out.push(e),
                    'b' => out.push('\u{8}'),
                    'f' => out.push('\u{c}'),
                    'n' => out.push('\n'),
                    'r' => out.push('\r'),
                    't' => out.push('\t'),
                    'u' => {
                        let hex = |it: &mut std::str::Chars<'_>| -> Option<u32> {
                            let h: String = it.by_ref().take(4).collect();
                            if h.len() != 4 {
                                return None;
                            }
                            u32::from_str_radix(&h, 16).ok()
                        };
                        let hi = hex(&mut it)?;
                        if (0xD800..0xDC00).contains(&hi) {
                            // A surrogate pair, or a lone surrogate (U+FFFD here).
                            let save = it.clone();
                            let lo = (it.next() == Some('\\') && it.next() == Some('u'))
                                .then(|| hex(&mut it))
                                .flatten()
                                .filter(|lo| (0xDC00..0xE000).contains(lo));
                            match lo {
                                Some(lo) => out.push(
                                    char::from_u32(0x10000 + ((hi - 0xD800) << 10) + (lo - 0xDC00))
                                        .unwrap_or('\u{fffd}'),
                                ),
                                None => {
                                    it = save;
                                    out.push('\u{fffd}');
                                }
                            }
                        } else {
                            out.push(char::from_u32(hi).unwrap_or('\u{fffd}'));
                        }
                    }
                    _ => return None,
                }
            }
            c => out.push(c),
        }
    }
    Some(out)
}

struct Parser<'a> {
    text: &'a str,
    i: usize,
}

impl Parser<'_> {
    fn fail<T>(&self, what: &str) -> Result<T, JsonError> {
        Err(JsonError {
            message: what.to_owned(),
            position: self.text[..self.i].encode_utf16().count(),
        })
    }

    fn ws(&mut self) {
        let rest = &self.text[self.i..];
        self.i += rest.len() - rest.trim_start_matches(is_js_space).len();
    }

    fn peek(&self) -> Option<char> {
        self.text[self.i..].chars().next()
    }

    fn value(&mut self) -> Result<JsonNode, JsonError> {
        self.ws();
        match self.peek() {
            Some('{') => {
                self.i += 1;
                let mut entries = Vec::new();
                self.ws();
                if self.peek() == Some('}') {
                    self.i += 1;
                    return Ok(JsonNode::Object(entries));
                }
                loop {
                    self.ws();
                    let rest = &self.text[self.i..];
                    let key = if rest.starts_with('"') {
                        &rest[..match_string(rest)]
                    } else {
                        ""
                    };
                    if key.len() < 2 || !key.ends_with('"') {
                        return self.fail("Expected a key");
                    }
                    self.i += key.len();
                    let Some(name) = decode_json_string(key) else {
                        return self.fail("Bad string");
                    };
                    self.ws();
                    if self.peek() != Some(':') {
                        return self.fail("Expected \":\"");
                    }
                    self.i += 1;
                    entries.push((name, self.value()?));
                    self.ws();
                    match self.peek() {
                        Some(',') => self.i += 1,
                        Some('}') => {
                            self.i += 1;
                            return Ok(JsonNode::Object(entries));
                        }
                        _ => return self.fail("Expected \",\" or \"}\""),
                    }
                }
            }
            Some('[') => {
                self.i += 1;
                let mut items = Vec::new();
                self.ws();
                if self.peek() == Some(']') {
                    self.i += 1;
                    return Ok(JsonNode::Array(items));
                }
                loop {
                    items.push(self.value()?);
                    self.ws();
                    match self.peek() {
                        Some(',') => self.i += 1,
                        Some(']') => {
                            self.i += 1;
                            return Ok(JsonNode::Array(items));
                        }
                        _ => return self.fail("Expected \",\" or \"]\""),
                    }
                }
            }
            c => {
                let rest = &self.text[self.i..];
                if c == Some('"') {
                    let n = match_string(rest);
                    if n > 1 && rest[..n].ends_with('"') {
                        self.i += n;
                        return Ok(JsonNode::String(rest[..n].to_owned()));
                    }
                }
                if let Some(n) = match_number(rest) {
                    self.i += n;
                    return Ok(JsonNode::Number(rest[..n].to_owned()));
                }
                if let Some(n) = match_literal(rest) {
                    self.i += n;
                    return Ok(JsonNode::Literal(rest[..n].to_owned()));
                }
                self.fail(if c.is_none() {
                    "Unexpected end"
                } else {
                    "Unexpected character"
                })
            }
        }
    }
}

/// Parses JSON into a tree, keeping scalar literals as written.
pub fn parse_json(text: &str) -> Result<JsonNode, JsonError> {
    let mut p = Parser { text, i: 0 };
    let root = p.value()?;
    p.ws();
    if p.i < text.len() {
        return p.fail("Unexpected text");
    }
    Ok(root)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn value_kind_maps_driver_type_names_to_kinds() {
        assert_eq!(value_kind("int4"), ValueKind::Number);
        assert_eq!(value_kind("numeric"), ValueKind::Number);
        assert_eq!(value_kind("interval"), ValueKind::Temporal);
        assert_eq!(value_kind("point"), ValueKind::Text);
        assert_eq!(value_kind("timestamptz"), ValueKind::Temporal);
        assert_eq!(value_kind("jsonb"), ValueKind::Json);
        assert_eq!(value_kind("bool"), ValueKind::Boolean);
        assert_eq!(value_kind("uuid"), ValueKind::Uuid);
        assert_eq!(value_kind("_int4"), ValueKind::Array);
        assert_eq!(value_kind("text"), ValueKind::Text);
        // Word boundaries, as the source's regexes.
        assert_eq!(value_kind("double precision"), ValueKind::Number);
        assert_eq!(
            value_kind("timestamp(3) with time zone"),
            ValueKind::Temporal
        );
        assert_eq!(value_kind("datetime2"), ValueKind::Temporal);
        assert_eq!(value_kind("interior"), ValueKind::Text);
        assert_eq!(value_kind("datemark"), ValueKind::Text);
        assert_eq!(value_kind("longblob"), ValueKind::Binary);
    }

    #[test]
    fn json_tokens_cover_the_input_and_tell_keys_from_strings() {
        let s = r#"{"a": "b", "n": [1, -2.5e3, true, null], "x": oops}"#;
        let toks = json_tokens(s);
        assert_eq!(toks.iter().map(|t| t.text.as_str()).collect::<String>(), s);
        let kinds: Vec<String> = toks
            .iter()
            .filter(|t| !js_trim(&t.text).is_empty())
            .map(|t| format!("{:?}:{}", t.kind, js_trim(&t.text)))
            .collect();
        for k in [
            r#"Key:"a""#,
            r#"String:"b""#,
            "Number:-2.5e3",
            "Literal:null",
            "Plain:oops",
        ] {
            assert!(kinds.iter().any(|x| x == k), "{k} in {kinds:?}");
        }
    }

    #[test]
    fn format_json_pretty_prints_and_keeps_literals_exact() {
        assert_eq!(
            format_json(r#"{"id":12345678901234567890,"a":[],"b":{"c":"x, {y}"}}"#),
            "{\n  \"id\": 12345678901234567890,\n  \"a\": [],\n  \"b\": {\n    \"c\": \"x, {y}\"\n  }\n}"
        );
    }

    #[test]
    fn parse_json_builds_a_tree_with_source_literals() {
        assert_eq!(
            parse_json(r#"{"a\n": [1.50, "s"], "b": {}}"#),
            Ok(JsonNode::Object(vec![
                (
                    "a\n".into(),
                    JsonNode::Array(vec![
                        JsonNode::Number("1.50".into()),
                        JsonNode::String("\"s\"".into()),
                    ])
                ),
                ("b".into(), JsonNode::Object(vec![])),
            ]))
        );
    }

    #[test]
    fn parse_json_rejects_bad_input() {
        assert!(parse_json(r#"{"a": }"#).is_err());
        let e = parse_json("[1] x").unwrap_err();
        assert_eq!(e.to_string(), "Unexpected text at position 4");
        assert!(parse_json("").is_err());
        assert!(parse_json(r#"{"a\x": 1}"#).is_err());
    }
}
