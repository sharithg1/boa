//! Redis connections: one multiplexed connection per (saved connection,
//! logical database `dbN`), the explorer's key listing, and the console's
//! commands (redis-cli syntax, one command per line) with their replies
//! turned into grid results.
//!
//! The databases are named `db0`, `db1`, … ([`db_name`], [`db_index`]),
//! as DataGrip names them; `ConnConfig::database` holds the default one's
//! index (`0`) or name.
//!
//! Console runs: every line is one command (blank lines and `#` / `//`
//! comments are skipped), run in order on the database's connection; the
//! last command's reply is the result. A run holding `MULTI` is sent as one
//! pipeline, so nothing else interleaves with the transaction. Commands that
//! need a connection of their own (`SELECT`, `SUBSCRIBE`, `MONITOR`, `WATCH`, …)
//! are refused. On a read-only connection, commands the server flags as
//! writing (`COMMAND INFO`) are refused before anything is sent.
//!
//! Replies become a grid (see [`reply_grid`]): a scalar is one `value` cell
//! (nil is NULL), a map or a field / value list (`HGETALL`, `CONFIG GET`, scores
//! `WITHSCORES`) two columns, a stream entry list `id` / `fields`, `INFO`
//! `section` / `key` / `value`, other lists one numbered row per item (nested
//! lists as compact JSON-like text). Bytes that aren't UTF-8 are escaped as
//! redis-cli does (`\xNN`).

use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use redis::aio::MultiplexedConnection;
use redis::{
    AsyncConnectionConfig, ConnectionAddr, IntoConnectionInfo, RedisConnectionInfo, RedisError,
    Value,
};
use serde::Serialize;

use crate::connections::{ConnConfig, SslMode};
use crate::error::{CmdError, CmdResult, UNREACHABLE};
use crate::model::{QueryResult, ResultColumn};
use crate::pg::{CancelHandle, ConnectInfo, PingInfo};
use crate::sql::guard::{Risk, RiskKind};
use crate::sql::{Token, TokenKind};
use crate::ssh::Tunnel;
use crate::tx::TxState;

/// The port Redis listens on by default.
pub const DEFAULT_PORT: u16 = 6379;

/// How many keys the explorer lists per database at first.
pub const KEY_LIMIT: usize = 1000;

/// How many more keys the explorer's "Load more keys" asks for.
pub const KEY_STEP: usize = 5000;

/// How long connecting may take.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// How long one command may take before the connection counts as lost.
const RESPONSE_TIMEOUT: Duration = Duration::from_secs(60);
/// How long `ping` waits.
const PING_TIMEOUT: Duration = Duration::from_secs(5);

/// A key as the explorer lists it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RedisKey {
    pub name: String,
    /// `TYPE`'s answer: `string`, `list`, `set`, `zset`, `hash`, `stream`, `ReJSON-RL`, …
    #[serde(rename = "type")]
    pub ty: String,
}

/// A database's keys (the first `limit` found, sorted by name).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RedisKeys {
    pub keys: Vec<RedisKey>,
    /// `DBSIZE`.
    pub total: u64,
    /// More keys exist than were listed.
    pub truncated: bool,
}

/// `db3` for 3.
pub fn db_name(index: u32) -> String {
    format!("db{index}")
}

/// 3 for `db3` or `3`.
pub fn db_index(name: &str) -> Option<u32> {
    let name = name.trim();
    name.strip_prefix("db").unwrap_or(name).parse().ok()
}

/// The default database's name (`db0` when `database` is empty or not an index).
pub fn default_db(config: &ConnConfig) -> String {
    db_name(db_index(&config.database).unwrap_or(0))
}

// ── Console syntax ───────────────────────────────────────────────────────

/// A console argument quoted as redis-cli reads it (bare when it needs no quotes).
pub fn quote_arg(arg: &str) -> String {
    let bare = !arg.is_empty()
        && arg
            .chars()
            .all(|c| !c.is_whitespace() && !c.is_control() && !matches!(c, '"' | '\'' | '\\'));
    if bare {
        return arg.to_owned();
    }
    let mut out = String::with_capacity(arg.len() + 2);
    out.push('"');
    for c in arg.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() && (c as u32) < 0x80 => {
                out.push_str(&format!("\\x{:02x}", c as u32));
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// One console line split into arguments as redis-cli does (`sdssplitargs`):
/// whitespace separates; `"…"` takes `\n \r \t \b \a \\ \" \xHH`
/// escapes, `'…'` only `\'`; a closing quote must end the argument.
pub fn split_args(line: &str) -> CmdResult<Vec<Vec<u8>>> {
    let unbalanced = || CmdError::msg("Invalid argument(s): unbalanced quotes");
    let b = line.as_bytes();
    let mut args: Vec<Vec<u8>> = Vec::new();
    let mut last_quoted = false;
    let mut i = 0;
    loop {
        while i < b.len() && b[i].is_ascii_whitespace() {
            i += 1;
        }
        if i >= b.len() {
            // A trailing `;` (SQL habit) ends the command, it isn't part of it.
            if !last_quoted
                && let Some(last) = args.last_mut()
                && last.last() == Some(&b';')
            {
                last.pop();
                if last.is_empty() {
                    args.pop();
                }
            }
            return Ok(args);
        }
        last_quoted = matches!(b[i], b'"' | b'\'');
        let mut cur = Vec::new();
        let (mut dq, mut sq) = (false, false);
        loop {
            if dq {
                let Some(&c) = b.get(i) else {
                    return Err(unbalanced());
                };
                if c == b'\\' && i + 3 < b.len() && b[i + 1] == b'x' {
                    let hex = std::str::from_utf8(&b[i + 2..i + 4]).ok();
                    if let Some(v) = hex.and_then(|h| u8::from_str_radix(h, 16).ok()) {
                        cur.push(v);
                        i += 4;
                        continue;
                    }
                }
                if c == b'\\' && i + 1 < b.len() {
                    cur.push(match b[i + 1] {
                        b'n' => b'\n',
                        b'r' => b'\r',
                        b't' => b'\t',
                        b'b' => 8,
                        b'a' => 7,
                        other => other,
                    });
                    i += 2;
                } else if c == b'"' {
                    if b.get(i + 1).is_some_and(|n| !n.is_ascii_whitespace()) {
                        return Err(unbalanced());
                    }
                    i += 1;
                    break;
                } else {
                    cur.push(c);
                    i += 1;
                }
            } else if sq {
                let Some(&c) = b.get(i) else {
                    return Err(unbalanced());
                };
                if c == b'\\' && b.get(i + 1) == Some(&b'\'') {
                    cur.push(b'\'');
                    i += 2;
                } else if c == b'\'' {
                    if b.get(i + 1).is_some_and(|n| !n.is_ascii_whitespace()) {
                        return Err(unbalanced());
                    }
                    i += 1;
                    break;
                } else {
                    cur.push(c);
                    i += 1;
                }
            } else {
                match b.get(i) {
                    None => break,
                    Some(c) if c.is_ascii_whitespace() => break,
                    Some(b'"') => dq = true,
                    Some(b'\'') => sq = true,
                    Some(&c) => cur.push(c),
                }
                i += 1;
            }
        }
        args.push(cur);
    }
}

/// A line that holds no command.
fn is_blank_or_comment(line: &str) -> bool {
    let t = line.trim();
    t.is_empty() || t.starts_with('#') || t.starts_with("//")
}

/// A console text's commands, one per line.
pub fn parse_commands(text: &str) -> CmdResult<Vec<Vec<Vec<u8>>>> {
    text.lines()
        .filter(|l| !is_blank_or_comment(l))
        .map(split_args)
        .filter(|r| r.as_ref().map_or(true, |a| !a.is_empty()))
        .collect()
}

/// The upper-cased command names of a console text (unparsable lines skipped).
fn command_names(text: &str) -> Vec<String> {
    text.lines()
        .filter(|l| !is_blank_or_comment(l))
        .filter_map(|l| split_args(l).ok())
        .filter_map(|a| {
            a.first()
                .map(|n| String::from_utf8_lossy(n).to_ascii_uppercase())
        })
        .collect()
}

/// Command options (flags and keywords after the key), coloured like SQL types.
const OPTIONS: &[&str] = &[
    "AGGREGATE",
    "ALPHA",
    "ASC",
    "BLOCK",
    "BY",
    "BYLEX",
    "BYSCORE",
    "CH",
    "COUNT",
    "DESC",
    "EX",
    "EXAT",
    "FIELDS",
    "FORCE",
    "GET",
    "GT",
    "ID",
    "IDLE",
    "INCR",
    "JUSTID",
    "KEEPTTL",
    "LEFT",
    "LIMIT",
    "LT",
    "MATCH",
    "MAXLEN",
    "MINID",
    "MKSTREAM",
    "NOMKSTREAM",
    "NOVALUES",
    "NX",
    "PX",
    "PXAT",
    "REPLACE",
    "RETRYCOUNT",
    "REV",
    "RIGHT",
    "STORE",
    "STREAMS",
    "SUM",
    "MIN",
    "MAX",
    "TYPE",
    "WEIGHTS",
    "WITHCOORD",
    "WITHDIST",
    "WITHHASH",
    "WITHSCORE",
    "WITHSCORES",
    "WITHVALUES",
    "XX",
    "ASYNC",
    "SYNC",
    "BEFORE",
    "AFTER",
    "FROMMEMBER",
    "FROMLONLAT",
    "BYRADIUS",
    "BYBOX",
    "M",
    "KM",
    "FT",
    "MI",
    "ANY",
];

/// Commands whose arguments are subcommands (`CONFIG GET`, `CLIENT LIST`).
const CONTAINERS: &[&str] = &[
    "ACL", "CLIENT", "CLUSTER", "COMMAND", "CONFIG", "DEBUG", "FUNCTION", "LATENCY", "MEMORY",
    "MODULE", "OBJECT", "PUBSUB", "SCRIPT", "SLOWLOG", "XGROUP", "XINFO",
];

/// Commands whose first argument is not a key.
const NO_KEY: &[&str] = &[
    "AUTH",
    "BGSAVE",
    "DBSIZE",
    "ECHO",
    "EVAL",
    "EVALSHA",
    "EVAL_RO",
    "EVALSHA_RO",
    "FCALL",
    "FCALL_RO",
    "FLUSHALL",
    "FLUSHDB",
    "INFO",
    "KEYS",
    "LASTSAVE",
    "MULTI",
    "EXEC",
    "DISCARD",
    "PING",
    "PUBLISH",
    "RANDOMKEY",
    "ROLE",
    "SAVE",
    "SCAN",
    "SELECT",
    "SWAPDB",
    "TIME",
    "WAIT",
    "LOLWUT",
    "SHUTDOWN",
    "REPLICAOF",
    "SLAVEOF",
    "MONITOR",
    "SUBSCRIBE",
    "PSUBSCRIBE",
];

fn is_number(word: &str) -> bool {
    let w = word.trim_start_matches(['-', '+']);
    !w.is_empty()
        && (w.parse::<f64>().is_ok() || w.eq_ignore_ascii_case("inf"))
        && !w.eq_ignore_ascii_case("nan")
        && !w.eq_ignore_ascii_case("infinity")
        || matches!(word, "(" | "[")
}

/// Highlighting tokens of a console text, contiguous from 0 (like
/// `sql::tokenize`): per line, the command (and a container's subcommand)
/// as keywords, its first argument as a key (table colour), known options
/// as types, numbers, quoted strings, and `#` / `//` comment lines.
pub fn tokenize(text: &str) -> Vec<Token> {
    let mut out: Vec<Token> = Vec::new();
    let push = |out: &mut Vec<Token>, start: usize, end: usize, kind: TokenKind| {
        if end <= start {
            return;
        }
        if let Some(last) = out.last_mut()
            && last.kind == kind
            && last.end == start
        {
            last.end = end;
            return;
        }
        out.push(Token { kind, start, end });
    };
    let b = text.as_bytes();
    let mut line_start = 0;
    while line_start <= text.len() {
        let line_end = text[line_start..]
            .find('\n')
            .map_or(text.len(), |i| line_start + i);
        let line = &text[line_start..line_end];
        if is_blank_or_comment(line) {
            let lead = line.len() - line.trim_start().len();
            push(&mut out, line_start, line_start + lead, TokenKind::Plain);
            push(
                &mut out,
                line_start + lead,
                line_end,
                if line.trim().is_empty() {
                    TokenKind::Plain
                } else {
                    TokenKind::Comment
                },
            );
        } else {
            let mut i = line_start;
            let mut word_ix = 0;
            let mut command = String::new();
            while i < line_end {
                if b[i].is_ascii_whitespace() {
                    let s = i;
                    while i < line_end && b[i].is_ascii_whitespace() {
                        i += 1;
                    }
                    push(&mut out, s, i, TokenKind::Plain);
                    continue;
                }
                let s = i;
                let quoted = matches!(b[i], b'"' | b'\'');
                if quoted {
                    let q = b[i];
                    i += 1;
                    while i < line_end && b[i] != q {
                        if b[i] == b'\\' && i + 1 < line_end {
                            i += 1;
                        }
                        i += 1;
                    }
                    i = (i + 1).min(line_end);
                } else {
                    while i < line_end && !b[i].is_ascii_whitespace() {
                        i += 1;
                    }
                }
                // A trailing `;` is not part of the word (see `split_args`).
                let end = if quoted {
                    i
                } else {
                    s + text[s..i].trim_end_matches(';').len()
                };
                let word = &text[s..end];
                let upper = word.to_ascii_uppercase();
                let kind = if word_ix == 0 {
                    command = upper;
                    TokenKind::Keyword
                } else if quoted {
                    if word_ix == 1
                        && !NO_KEY.contains(&command.as_str())
                        && !CONTAINERS.contains(&command.as_str())
                    {
                        TokenKind::Table
                    } else {
                        TokenKind::String
                    }
                } else if word_ix == 1 && CONTAINERS.contains(&command.as_str()) {
                    TokenKind::Keyword
                } else if word_ix == 1 && !NO_KEY.contains(&command.as_str()) {
                    TokenKind::Table
                } else if OPTIONS.contains(&upper.as_str()) {
                    TokenKind::Type
                } else if is_number(word) {
                    TokenKind::Number
                } else {
                    TokenKind::Plain
                };
                push(&mut out, s, end, kind);
                push(&mut out, end, i, TokenKind::Plain);
                if end > s {
                    word_ix += 1;
                }
            }
        }
        if line_end < text.len() {
            push(&mut out, line_end, line_end + 1, TokenKind::Plain);
        }
        line_start = line_end + 1;
    }
    out
}

/// Where a key sits in the explorer's folders: the byte offsets just past
/// each `:` run in its name (`a:b::c` → 2, 5), so `name[..end]` is each
/// folder's prefix (`a:`, `a:b::`). A trailing separator doesn't make a
/// folder of the key itself.
pub fn folder_ends(name: &str) -> Vec<usize> {
    let b = name.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b':' {
            while i < b.len() && b[i] == b':' {
                i += 1;
            }
            if i < b.len() && i > 0 && !out.contains(&i) {
                out.push(i);
            }
        } else {
            i += 1;
        }
    }
    // A leading run (`:x`) is not a folder.
    out.retain(|&e| !name[..e].trim_end_matches(':').is_empty());
    out
}

/// The command that shows a key of type `ty` (`GET`, `HGETALL`,
/// `LRANGE k 0 -1`, `SMEMBERS`, `ZRANGE k 0 -1 WITHSCORES`, `XRANGE k - +`, …).
pub fn open_command(key: &str, ty: &str) -> String {
    let k = quote_arg(key);
    match ty {
        "string" => format!("GET {k}"),
        "hash" => format!("HGETALL {k}"),
        "list" => format!("LRANGE {k} 0 -1"),
        "set" => format!("SMEMBERS {k}"),
        "zset" => format!("ZRANGE {k} 0 -1 WITHSCORES"),
        "stream" => format!("XRANGE {k} - +"),
        "ReJSON-RL" => format!("JSON.GET {k}"),
        _ => format!("TYPE {k}"),
    }
}

/// The command under the caret at byte `pos`: its line (trimmed), unless blank
/// or a comment.
pub fn command_at(text: &str, pos: usize) -> Option<Range<usize>> {
    let pos = pos.min(text.len());
    let start = text[..pos].rfind('\n').map_or(0, |i| i + 1);
    let end = text[pos..].find('\n').map_or(text.len(), |i| pos + i);
    let line = &text[start..end];
    if is_blank_or_comment(line) {
        return None;
    }
    let lead = line.len() - line.trim_start().len();
    let trail = line.len() - line.trim_end().len();
    Some(start + lead..end - trail)
}

/// Commands that drop data.
const DESTRUCTIVE: &[&str] = &["FLUSHALL", "FLUSHDB", "DEL", "UNLINK", "SWAPDB"];

/// The guard rails' risks of a console text: emptying or swapping whole
/// databases (`FLUSHALL`, `FLUSHDB`, `SWAPDB`) always; deleting keys
/// (`DEL`, `UNLINK`) only when `production`.
pub fn destructive_risks(text: &str, production: bool) -> Vec<Risk> {
    let mut out = Vec::new();
    for line in text.lines().filter(|l| !is_blank_or_comment(l)) {
        let Ok(args) = split_args(line) else { continue };
        let Some(name) = args
            .first()
            .map(|n| String::from_utf8_lossy(n).to_ascii_uppercase())
        else {
            continue;
        };
        let (kind, message) = match name.as_str() {
            "FLUSHALL" => (
                RiskKind::Truncate,
                "FLUSHALL deletes every key of every database".to_owned(),
            ),
            "FLUSHDB" => (
                RiskKind::Truncate,
                "FLUSHDB deletes every key of this database".to_owned(),
            ),
            "SWAPDB" => (
                RiskKind::Drop,
                "SWAPDB swaps the contents of two whole databases".to_owned(),
            ),
            "DEL" | "UNLINK" if production => (
                RiskKind::Drop,
                format!("{name} deletes {} key(s)", args.len().saturating_sub(1)),
            ),
            _ => continue,
        };
        out.push(Risk {
            kind,
            message,
            statement: line.trim().to_owned(),
        });
    }
    out
}

/// Commands that drop data (`FLUSHALL`, `FLUSHDB`, `DEL`, …) in `text`, as
/// the names to show in the "Run a destructive statement?" confirmation.
pub fn destructive_commands(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for name in command_names(text) {
        if DESTRUCTIVE.contains(&name.as_str()) && !out.contains(&name) {
            out.push(name);
        }
    }
    out
}

/// Commands that only read (and so may be re-run, and never change the key list).
const READ_ONLY: &[&str] = &[
    "BITCOUNT",
    "BITFIELD_RO",
    "BITPOS",
    "DBSIZE",
    "DUMP",
    "ECHO",
    "EVALSHA_RO",
    "EVAL_RO",
    "EXISTS",
    "EXPIRETIME",
    "FCALL_RO",
    "GEODIST",
    "GEOHASH",
    "GEOPOS",
    "GEORADIUSBYMEMBER_RO",
    "GEORADIUS_RO",
    "GEOSEARCH",
    "GET",
    "GETBIT",
    "GETRANGE",
    "HEXISTS",
    "HGET",
    "HGETALL",
    "HKEYS",
    "HLEN",
    "HMGET",
    "HRANDFIELD",
    "HSCAN",
    "HSTRLEN",
    "HVALS",
    "INFO",
    "JSON.ARRLEN",
    "JSON.GET",
    "JSON.MGET",
    "JSON.OBJKEYS",
    "JSON.OBJLEN",
    "JSON.STRLEN",
    "JSON.TYPE",
    "KEYS",
    "LASTSAVE",
    "LCS",
    "LINDEX",
    "LLEN",
    "LOLWUT",
    "LPOS",
    "LRANGE",
    "MEMORY",
    "MGET",
    "OBJECT",
    "PEXPIRETIME",
    "PFCOUNT",
    "PING",
    "PTTL",
    "RANDOMKEY",
    "ROLE",
    "SCAN",
    "SCARD",
    "SDIFF",
    "SINTER",
    "SINTERCARD",
    "SISMEMBER",
    "SMEMBERS",
    "SMISMEMBER",
    "SORT_RO",
    "SRANDMEMBER",
    "SSCAN",
    "STRLEN",
    "SUBSTR",
    "SUNION",
    "TIME",
    "TTL",
    "TYPE",
    "XINFO",
    "XLEN",
    "XPENDING",
    "XRANGE",
    "XREVRANGE",
    "ZCARD",
    "ZCOUNT",
    "ZDIFF",
    "ZINTER",
    "ZINTERCARD",
    "ZLEXCOUNT",
    "ZMSCORE",
    "ZRANDMEMBER",
    "ZRANGE",
    "ZRANGEBYLEX",
    "ZRANGEBYSCORE",
    "ZRANK",
    "ZREVRANGE",
    "ZREVRANGEBYLEX",
    "ZREVRANGEBYSCORE",
    "ZREVRANK",
    "ZSCAN",
    "ZSCORE",
    "ZUNION",
];

fn is_read_only(name: &str) -> bool {
    READ_ONLY.contains(&name)
}

/// `text`'s commands may have added, removed or renamed keys (the explorer
/// re-reads the database after them): anything but known read-only commands.
pub fn changes_keys(text: &str) -> bool {
    !command_names(text).iter().all(|n| is_read_only(n))
}

/// Commands that need a connection of their own (the console shares one per database).
fn refused(name: &str) -> Option<&'static str> {
    Some(match name {
        "SELECT" => {
            "SELECT is not available: pick the database (db0, db1, …) in the explorer or the console's toolbar instead"
        }
        "SUBSCRIBE" | "PSUBSCRIBE" | "SSUBSCRIBE" | "MONITOR" | "SYNC" | "PSYNC" => {
            "This command streams replies forever and can't run in a console"
        }
        "WATCH" | "UNWATCH" => "WATCH is not available in the console",
        "QUIT" | "RESET" | "HELLO" | "AUTH" => "This command would change the console's connection",
        _ => return None,
    })
}

// ── Replies as grids ─────────────────────────────────────────────────────

/// Bytes as text: UTF-8 as is, other bytes escaped as redis-cli shows them.
pub fn bytes_text(b: &[u8]) -> String {
    match std::str::from_utf8(b) {
        Ok(s) => s.to_owned(),
        Err(_) => {
            let mut out = String::with_capacity(b.len());
            for chunk in b.utf8_chunks() {
                out.push_str(chunk.valid());
                for byte in chunk.invalid() {
                    out.push_str(&format!("\\x{byte:02x}"));
                }
            }
            out
        }
    }
}

/// A scalar reply as text (None: nil). Lists render compactly.
fn cell(v: &Value) -> Option<String> {
    Some(match v {
        Value::Nil => return None,
        Value::Attribute { data, .. } => return cell(data),
        Value::Int(i) => i.to_string(),
        Value::BulkString(b) => bytes_text(b),
        Value::SimpleString(s) => s.clone(),
        Value::Okay => "OK".into(),
        Value::Double(d) => d.to_string(),
        Value::Boolean(b) => b.to_string(),
        Value::VerbatimString { text, .. } => text.clone(),
        Value::BigNumber(n) => format!("{n:?}"),
        Value::ServerError(e) => format!("(error) {e}"),
        other => nested(other),
    })
}

/// A value inside a list, as compact JSON-like text.
fn nested(v: &Value) -> String {
    let list = |items: &[Value]| {
        let parts: Vec<String> = items.iter().map(nested).collect();
        format!("[{}]", parts.join(", "))
    };
    match v {
        Value::Nil => "null".into(),
        Value::Array(items) | Value::Set(items) => list(items),
        Value::Push { data, .. } => list(data),
        Value::Map(pairs) => {
            let parts: Vec<String> = pairs
                .iter()
                .map(|(k, v)| format!("{}: {}", nested(k), nested(v)))
                .collect();
            format!("{{{}}}", parts.join(", "))
        }
        Value::Attribute { data, .. } => nested(data),
        Value::BulkString(_) | Value::SimpleString(_) | Value::VerbatimString { .. } => {
            serde_json::to_string(&cell(v).unwrap_or_default()).unwrap_or_default()
        }
        other => cell(other).unwrap_or_default(),
    }
}

/// The grid type of a scalar reply.
fn scalar_type(v: &Value) -> &'static str {
    match v {
        Value::Int(_) => "int8",
        Value::Double(_) => "float8",
        Value::Boolean(_) => "bool",
        _ => "text",
    }
}

type Grid = (Vec<ResultColumn>, Vec<Vec<Option<String>>>);

fn text_cols(names: &[&str]) -> Vec<ResultColumn> {
    names
        .iter()
        .map(|n| ResultColumn::new(*n, "text"))
        .collect()
}

/// The reply items as (first, second) pairs.
fn pairs(items: &[Value]) -> Vec<Vec<Option<String>>> {
    items
        .chunks(2)
        .map(|p| vec![cell(&p[0]), p.get(1).and_then(cell)])
        .collect()
}

/// A command returning a flat field / value (or member / score) list:
/// Some(column names).
fn pair_columns(name: &str, args: &[String]) -> Option<[&'static str; 2]> {
    let has = |flag: &str| args.iter().any(|a| a.eq_ignore_ascii_case(flag));
    match name {
        "HGETALL" | "CONFIG" => Some(["field", "value"]),
        "HRANDFIELD" if has("WITHVALUES") => Some(["field", "value"]),
        "ZPOPMIN" | "ZPOPMAX" => Some(["member", "score"]),
        "ZRANGE" | "ZRANGEBYSCORE" | "ZREVRANGE" | "ZREVRANGEBYSCORE" | "ZRANDMEMBER"
        | "ZINTER" | "ZUNION" | "ZDIFF"
            if has("WITHSCORES") =>
        {
            Some(["member", "score"])
        }
        _ => None,
    }
}

/// `INFO`'s text as (section, key, value) rows.
fn info_rows(text: &str) -> Vec<Vec<Option<String>>> {
    let mut section = String::new();
    let mut rows = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if let Some(s) = line.strip_prefix('#') {
            section = s.trim().to_owned();
        } else if let Some((k, v)) = line.split_once(':') {
            rows.push(vec![
                Some(section.clone()),
                Some(k.to_owned()),
                Some(v.to_owned()),
            ]);
        }
    }
    rows
}

/// The grid of one command's reply (see the module docs). `cmd` is the
/// command as run (name first).
pub fn reply_grid(cmd: &[String], v: &Value) -> Grid {
    let name = cmd
        .first()
        .map(|n| n.to_ascii_uppercase())
        .unwrap_or_default();
    let args = cmd.get(1..).unwrap_or_default();
    match v {
        Value::Attribute { data, .. } => reply_grid(cmd, data),
        Value::Map(entries) => {
            let rows = entries
                .iter()
                .map(|(k, v)| vec![cell(k), cell(v)])
                .collect();
            (text_cols(&["field", "value"]), rows)
        }
        Value::Array(items) | Value::Set(items) | Value::Push { data: items, .. } => {
            let scan = matches!(name.as_str(), "SCAN" | "HSCAN" | "SSCAN" | "ZSCAN");
            if scan && let [cursor, Value::Array(found)] = items.as_slice() {
                let cursor = cell(cursor).unwrap_or_default();
                let next = format!("value (next cursor: {cursor})");
                return match name.as_str() {
                    "HSCAN" => (text_cols(&["field", &next]), pairs(found)),
                    "ZSCAN" => (text_cols(&["member", &next]), pairs(found)),
                    _ => list_grid(found, &next),
                };
            }
            if matches!(name.as_str(), "XRANGE" | "XREVRANGE") {
                let rows = items
                    .iter()
                    .map(|e| match e {
                        Value::Array(parts) if parts.len() == 2 => {
                            let fields = match &parts[1] {
                                Value::Array(f) => f
                                    .chunks(2)
                                    .map(|p| {
                                        format!(
                                            "{}={}",
                                            cell(&p[0]).unwrap_or_default(),
                                            p.get(1).and_then(cell).unwrap_or_default()
                                        )
                                    })
                                    .collect::<Vec<_>>()
                                    .join(" "),
                                other => nested(other),
                            };
                            vec![cell(&parts[0]), Some(fields)]
                        }
                        other => vec![None, Some(nested(other))],
                    })
                    .collect();
                return (text_cols(&["id", "fields"]), rows);
            }
            if let Some(cols) = pair_columns(&name, args)
                && items.len() % 2 == 0
                && items.iter().all(|i| !matches!(i, Value::Array(_)))
            {
                return (text_cols(&cols), pairs(items));
            }
            list_grid(items, "value")
        }
        Value::BulkString(_) | Value::VerbatimString { .. } if name == "INFO" => {
            let text = cell(v).unwrap_or_default();
            (text_cols(&["section", "key", "value"]), info_rows(&text))
        }
        scalar => (
            vec![ResultColumn::new("value", scalar_type(scalar))],
            vec![vec![cell(scalar)]],
        ),
    }
}

/// One numbered row per item.
fn list_grid(items: &[Value], value_col: &str) -> Grid {
    let rows = items
        .iter()
        .enumerate()
        .map(|(i, v)| vec![Some((i + 1).to_string()), cell(v)])
        .collect();
    (
        vec![
            ResultColumn::new("#", "int8"),
            ResultColumn::new(value_col, "text"),
        ],
        rows,
    )
}

// ── Errors ───────────────────────────────────────────────────────────────

/// A dropped or unreachable connection (worth reconnecting), not a server answer.
fn is_network(e: &RedisError) -> bool {
    e.is_io_error() || e.is_connection_refusal() || e.is_timeout() || e.is_connection_dropped()
}

/// A Redis error as the app's error. `had_password`: NOAUTH means the
/// password is missing (`password_required`) or else wrong.
fn cmd_error(e: RedisError, had_password: bool) -> CmdError {
    let code = e.code().unwrap_or_default().to_owned();
    let text = match e.detail() {
        Some(d) if !code.is_empty() => format!("{code} {d}"),
        _ => e.to_string(),
    };
    let auth = code == "NOAUTH"
        || code == "WRONGPASS"
        || e.kind() == redis::ErrorKind::AuthenticationFailed
        || text.contains("Authentication required")
        || text.contains("invalid password");
    if auth {
        if !had_password && code != "WRONGPASS" {
            return CmdError::code("password_required", "The server asked for a password");
        }
        return CmdError::code("28P01", text);
    }
    if is_network(&e) {
        return CmdError::code(UNREACHABLE, text);
    }
    CmdError {
        tx: Some(TxState::Idle),
        ..CmdError::msg(text)
    }
}

fn lost(e: CmdError) -> CmdError {
    if e.code.as_deref() == Some(UNREACHABLE) {
        return CmdError {
            tx: Some(TxState::Idle),
            detail: Some(e.message),
            ..CmdError::code("connection_lost", "The connection to the server was lost")
        };
    }
    e
}

fn read_only_error(name: &str) -> CmdError {
    CmdError {
        tx: Some(TxState::Idle),
        hint: Some("Turn read-only off in the data source settings to write.".into()),
        ..CmdError::code(
            "read_only",
            format!("The connection is read-only: {name} writes, so nothing was run"),
        )
    }
}

fn cancelled() -> CmdError {
    CmdError::code("57014", "canceling statement due to user request")
}

// ── Connections ──────────────────────────────────────────────────────────

/// How to reach the server (through the tunnel's local port when there is one).
fn connection_info(
    config: &ConnConfig,
    password: Option<&str>,
    db: u32,
    via: Option<u16>,
) -> CmdResult<redis::ConnectionInfo> {
    let (host, port) = match via {
        Some(p) => ("127.0.0.1".to_owned(), p),
        None => (config.host.trim().to_owned(), config.port),
    };
    let addr = match config.ssl_mode {
        SslMode::Disable | SslMode::Prefer => ConnectionAddr::Tcp(host, port),
        mode => ConnectionAddr::TcpTls {
            host,
            port,
            insecure: mode == SslMode::Require,
            tls_params: None,
        },
    };
    let mut settings = RedisConnectionInfo::default()
        .set_db(i64::from(db))
        .set_lib_name("boa", env!("CARGO_PKG_VERSION"));
    if let Some(pw) = password.filter(|p| !p.is_empty()) {
        let user = config.user.trim();
        if !user.is_empty() {
            settings = settings.set_username(user);
        }
        settings = settings.set_password(pw);
    }
    let info = addr
        .into_connection_info()
        .map_err(|e| cmd_error(e, password.is_some()))?;
    Ok(info.set_redis_settings(settings))
}

async fn open_conn(
    config: &ConnConfig,
    password: Option<&str>,
    db: u32,
    via: Option<u16>,
) -> CmdResult<MultiplexedConnection> {
    let had = password.is_some_and(|p| !p.is_empty());
    let info = connection_info(config, password, db, via)?;
    let client = redis::Client::open(info).map_err(|e| cmd_error(e, had))?;
    let opts = AsyncConnectionConfig::new()
        .set_connection_timeout(Some(CONNECT_TIMEOUT))
        .set_response_timeout(Some(RESPONSE_TIMEOUT));
    let mut conn = client
        .get_multiplexed_async_connection_with_config(&opts)
        .await
        .map_err(|e| cmd_error(e, had))?;
    // Without a password the handshake doesn't authenticate: find out now.
    redis::cmd("PING")
        .query_async::<Value>(&mut conn)
        .await
        .map_err(|e| cmd_error(e, had))?;
    Ok(conn)
}

/// `Redis 7.2.4` (or `Valkey 8.0.1`) from `INFO server`.
fn server_version(info: &str) -> String {
    let field = |name: &str| {
        info.lines()
            .find_map(|l| l.trim().strip_prefix(name)?.strip_prefix(':'))
            .map(str::to_owned)
    };
    if let Some(v) = field("valkey_version") {
        return format!("Valkey {v}");
    }
    match field("redis_version") {
        Some(v) => format!("Redis {v}"),
        None => "Redis".into(),
    }
}

async fn version_of(conn: &mut MultiplexedConnection, had: bool) -> CmdResult<String> {
    let info: Value = redis::cmd("INFO")
        .arg("server")
        .query_async(conn)
        .await
        .map_err(|e| cmd_error(e, had))?;
    Ok(server_version(&cell(&info).unwrap_or_default()))
}

/// A saved connection that is open.
#[derive(Debug)]
struct Open {
    config: ConnConfig,
    password: Option<String>,
    /// `COMMAND INFO` answers: does the command write?
    writes: Mutex<HashMap<String, bool>>,
}

/// The open Redis connections.
#[derive(Default)]
pub struct Redis {
    open: Mutex<HashMap<String, Arc<Open>>>,
    sessions: tokio::sync::Mutex<HashMap<(String, u32), MultiplexedConnection>>,
    tunnels: Mutex<HashMap<String, Tunnel>>,
}

impl std::fmt::Debug for Redis {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Redis")
            .field("open", &self.lock_open().keys().collect::<Vec<_>>())
            .finish_non_exhaustive()
    }
}

impl Redis {
    fn lock_open(&self) -> std::sync::MutexGuard<'_, HashMap<String, Arc<Open>>> {
        self.open.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn tunnels(&self) -> std::sync::MutexGuard<'_, HashMap<String, Tunnel>> {
        self.tunnels.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn get_open(&self, id: &str) -> CmdResult<Arc<Open>> {
        self.lock_open()
            .get(id)
            .cloned()
            .ok_or_else(|| CmdError::code("not_connected", "Not connected"))
    }

    /// The tunnel's local port (none without one), starting ssh when needed.
    async fn tunnel_port(&self, config: &ConnConfig) -> CmdResult<Option<u16>> {
        let Some(ssh) = config.tunnel() else {
            return Ok(None);
        };
        if let Some(t) = self.tunnels().get(&config.id)
            && t.is_alive()
        {
            return Ok(Some(t.port));
        }
        let t = Tunnel::open(ssh, &config.host, config.port).await?;
        let port = t.port;
        self.tunnels().insert(config.id.clone(), t);
        Ok(Some(port))
    }

    fn close_tunnel(&self, id: &str) -> Option<Tunnel> {
        let t = self.tunnels().remove(id);
        if let Some(t) = &t {
            t.kill();
        }
        t
    }

    /// Connects (through the SSH tunnel when enabled) and lists the databases.
    pub async fn connect(
        &self,
        config: ConnConfig,
        password: Option<String>,
    ) -> CmdResult<ConnectInfo> {
        self.disconnect(&config.id).await;
        let db = db_index(&config.database).unwrap_or(0);
        let had = password.as_deref().is_some_and(|p| !p.is_empty());
        let first = async {
            let via = self.tunnel_port(&config).await?;
            let mut conn = open_conn(&config, password.as_deref(), db, via).await?;
            let version = version_of(&mut conn, had).await?;
            // CONFIG is often disabled on managed servers: 16, the default.
            let count = redis::cmd("CONFIG")
                .arg("GET")
                .arg("databases")
                .query_async::<Value>(&mut conn)
                .await
                .ok()
                .and_then(|v| match v {
                    Value::Array(items) => items.get(1).and_then(cell),
                    Value::Map(pairs) => pairs.first().and_then(|(_, v)| cell(v)),
                    _ => None,
                })
                .and_then(|n| n.parse::<u32>().ok())
                .filter(|n| *n > 0)
                .unwrap_or(16)
                .max(db + 1);
            Ok::<_, CmdError>((conn, version, count))
        };
        let (conn, server_version, count) = match first.await {
            Ok(x) => x,
            Err(e) => {
                return Err(match self.close_tunnel(&config.id) {
                    Some(t) => t.explain(e).await,
                    None => e,
                });
            }
        };
        let id = config.id.clone();
        self.sessions.lock().await.insert((id.clone(), db), conn);
        self.lock_open().insert(
            id,
            Arc::new(Open {
                config,
                password,
                writes: Mutex::default(),
            }),
        );
        Ok(ConnectInfo {
            server_version,
            databases: (0..count).map(db_name).collect(),
        })
    }

    /// Connects with unsaved settings and returns the server version (`Redis 7.2.4`).
    pub async fn test(config: &ConnConfig, password: Option<&str>) -> CmdResult<String> {
        let tunnel = match config.tunnel() {
            Some(t) => Some(Tunnel::open(t, &config.host, config.port).await?),
            None => None,
        };
        let had = password.is_some_and(|p| !p.is_empty());
        let version = async {
            let via = tunnel.as_ref().map(|t| t.port);
            let db = db_index(&config.database).unwrap_or(0);
            let mut conn = open_conn(config, password, db, via).await?;
            version_of(&mut conn, had).await
        };
        match (version.await, &tunnel) {
            (Err(e), Some(t)) => Err(t.explain(e).await),
            (r, _) => r,
        }
    }

    /// The connection is open.
    pub fn is_open(&self, id: &str) -> bool {
        self.lock_open().contains_key(id)
    }

    /// The database's connection, opened on first use.
    async fn session(&self, id: &str, db: u32) -> CmdResult<(Arc<Open>, MultiplexedConnection)> {
        let open = self.get_open(id)?;
        if let Some(c) = self.sessions.lock().await.get(&(id.to_owned(), db)) {
            return Ok((open, c.clone()));
        }
        let via = self.tunnel_port(&open.config).await?;
        let conn = open_conn(&open.config, open.password.as_deref(), db, via).await?;
        self.sessions
            .lock()
            .await
            .insert((id.to_owned(), db), conn.clone());
        Ok((open, conn))
    }

    /// Forgets a database's connection (it dropped): the next call reopens it.
    async fn drop_session(&self, id: &str, db: u32) {
        self.sessions.lock().await.remove(&(id.to_owned(), db));
    }

    fn db_of(database: &str) -> CmdResult<u32> {
        db_index(database).ok_or_else(|| CmdError::msg(format!("Unknown database {database}")))
    }

    /// Checks the connection, reopening its default database's connection
    /// when it dropped. Redis has no transactions to lose.
    pub async fn ping(&self, id: &str) -> CmdResult<PingInfo> {
        let open = self.get_open(id)?;
        let db = db_index(&open.config.database).unwrap_or(0);
        let had = open.password.as_deref().is_some_and(|p| !p.is_empty());
        let check = async {
            let (_, mut conn) = self.session(id, db).await?;
            redis::cmd("PING")
                .query_async::<Value>(&mut conn)
                .await
                .map_err(|e| cmd_error(e, had))
        };
        match tokio::time::timeout(PING_TIMEOUT, check).await {
            Ok(Ok(_)) => Ok(PingInfo {
                lost_tx: Vec::new(),
            }),
            Ok(Err(e)) => {
                self.sessions.lock().await.retain(|(cid, _), _| cid != id);
                Err(e)
            }
            Err(_) => {
                self.sessions.lock().await.retain(|(cid, _), _| cid != id);
                Err(CmdError::code(UNREACHABLE, "The server did not answer"))
            }
        }
    }

    pub async fn disconnect(&self, id: &str) {
        self.lock_open().remove(id);
        self.sessions.lock().await.retain(|(cid, _), _| cid != id);
        self.close_tunnel(id);
    }

    /// Kills every SSH tunnel at once.
    pub fn close_tunnels(&self) {
        for (_, t) in self.tunnels().drain() {
            t.kill();
        }
    }

    /// The database's keys (`SCAN`, then their `TYPE`s). Retried once on a
    /// fresh connection when the old one dropped.
    pub async fn keys(&self, id: &str, database: &str, limit: usize) -> CmdResult<RedisKeys> {
        let db = Self::db_of(database)?;
        let limit = limit.max(1);
        match self.keys_once(id, db, limit).await {
            Err(e) if e.code.as_deref() == Some(UNREACHABLE) => {
                self.drop_session(id, db).await;
                self.keys_once(id, db, limit).await
            }
            r => r,
        }
    }

    async fn keys_once(&self, id: &str, db: u32, limit: usize) -> CmdResult<RedisKeys> {
        let (open, mut conn) = self.session(id, db).await?;
        let had = open.password.is_some();
        let err = |e| cmd_error(e, had);
        let mut names: Vec<Vec<u8>> = Vec::new();
        let mut seen: HashSet<Vec<u8>> = HashSet::new();
        let mut cursor: u64 = 0;
        let mut stopped = false;
        loop {
            let (next, batch): (u64, Vec<Vec<u8>>) = redis::cmd("SCAN")
                .arg(cursor)
                .arg("COUNT")
                .arg(1000)
                .query_async(&mut conn)
                .await
                .map_err(err)?;
            for k in batch {
                if seen.insert(k.clone()) {
                    names.push(k);
                }
            }
            if names.len() >= limit {
                stopped = next != 0 || names.len() > limit;
                names.truncate(limit);
                break;
            }
            if next == 0 {
                break;
            }
            cursor = next;
        }
        let mut keys = Vec::with_capacity(names.len());
        for chunk in names.chunks(1000) {
            let mut p = redis::pipe();
            for k in chunk {
                p.cmd("TYPE").arg(k.as_slice());
            }
            let types: Vec<String> = p.query_async(&mut conn).await.map_err(err)?;
            keys.extend(
                chunk
                    .iter()
                    .zip(types)
                    .filter(|(_, ty)| ty != "none")
                    .map(|(k, ty)| RedisKey {
                        name: bytes_text(k),
                        ty,
                    }),
            );
        }
        keys.sort_by(|a, b| a.name.cmp(&b.name));
        let total: u64 = redis::cmd("DBSIZE")
            .query_async(&mut conn)
            .await
            .map_err(err)?;
        let truncated = stopped || total > keys.len() as u64;
        Ok(RedisKeys {
            keys,
            total,
            truncated,
        })
    }

    /// Whether a command writes, by the server's own flags (cached), else by
    /// the static read-only list.
    async fn writes(&self, open: &Open, conn: &mut MultiplexedConnection, args: &[String]) -> bool {
        let name = args[0].to_ascii_lowercase();
        // Container commands are flagged per subcommand (`config|set`).
        let full = match (name.as_str(), args.get(1)) {
            (
                "config" | "client" | "acl" | "script" | "function" | "memory" | "object" | "xinfo"
                | "slowlog" | "latency" | "cluster" | "module" | "xgroup" | "pubsub" | "command",
                Some(sub),
            ) => format!("{name}|{}", sub.to_ascii_lowercase()),
            _ => name.clone(),
        };
        if let Some(w) = open
            .writes
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&full)
        {
            return *w;
        }
        let flags: Option<Vec<String>> = redis::cmd("COMMAND")
            .arg("INFO")
            .arg(&full)
            .query_async::<Value>(conn)
            .await
            .ok()
            .and_then(|v| match v {
                Value::Array(mut items) if !items.is_empty() => match items.swap_remove(0) {
                    Value::Array(info) => match info.get(2) {
                        Some(Value::Array(f) | Value::Set(f)) => {
                            Some(f.iter().filter_map(cell).collect())
                        }
                        _ => None,
                    },
                    _ => None,
                },
                _ => None,
            });
        let w = match flags {
            Some(flags) => {
                let has = |f: &str| flags.iter().any(|x| x.eq_ignore_ascii_case(f));
                has("write")
                    || has("may_replicate")
                    || (has("admin") && !full.ends_with("|get") && !full.starts_with("slowlog|"))
            }
            None => !is_read_only(&args[0].to_ascii_uppercase()),
        };
        open.writes
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(full, w);
        w
    }

    /// Runs the console's commands (one per line) and returns the last one's
    /// reply as a grid: `limit` rows after `offset`.
    pub async fn query(
        &self,
        id: &str,
        database: &str,
        text: &str,
        limit: usize,
        offset: usize,
        cancel: &CancelHandle,
    ) -> CmdResult<QueryResult> {
        let started = Instant::now();
        let db = Self::db_of(database)?;
        let raw = parse_commands(text)?;
        if raw.is_empty() {
            return Err(CmdError::msg("Nothing to run"));
        }
        let shown: Vec<Vec<String>> = raw
            .iter()
            .map(|c| c.iter().map(|a| bytes_text(a)).collect())
            .collect();
        let names: Vec<String> = shown.iter().map(|c| c[0].to_ascii_uppercase()).collect();
        if let Some(why) = names.iter().find_map(|n| refused(n)) {
            return Err(CmdError::msg(why));
        }
        if cancel.is_cancelled() {
            return Err(cancelled());
        }
        let (open, mut conn) = self.session(id, db).await?;
        let had = open.password.as_deref().is_some_and(|p| !p.is_empty());
        if open.config.read_only {
            for (cmd, name) in shown.iter().zip(&names) {
                if name != "MULTI"
                    && name != "EXEC"
                    && name != "DISCARD"
                    && self.writes(&open, &mut conn, cmd).await
                {
                    return Err(read_only_error(name));
                }
            }
        }
        let build = |args: &[Vec<u8>]| {
            let mut c = redis::cmd(&String::from_utf8_lossy(&args[0]));
            for a in &args[1..] {
                c.arg(a.as_slice());
            }
            c
        };
        let last = if names.iter().any(|n| n == "MULTI") {
            // One pipeline: nothing else on this connection lands inside the transaction.
            let mut p = redis::pipe();
            for c in &raw {
                p.add_command(build(c));
            }
            let replies: Vec<Value> = p
                .query_async(&mut conn)
                .await
                .map_err(|e| lost(cmd_error(e, had)))?;
            replies.into_iter().last().unwrap_or(Value::Nil)
        } else {
            let mut last = Value::Nil;
            for c in &raw {
                if cancel.is_cancelled() {
                    return Err(cancelled());
                }
                last = match build(c).query_async::<Value>(&mut conn).await {
                    Ok(v) => v,
                    Err(e) => {
                        let e = cmd_error(e, had);
                        if e.code.as_deref() == Some(UNREACHABLE) {
                            self.drop_session(id, db).await;
                        }
                        return Err(lost(e));
                    }
                };
            }
            last
        };
        let (columns, all) = reply_grid(shown.last().map(Vec::as_slice).unwrap_or_default(), &last);
        let total = all.len();
        let rows: Vec<Vec<Option<String>>> = all.into_iter().skip(offset).take(limit).collect();
        let pageable = names.iter().all(|n| is_read_only(n));
        Ok(QueryResult {
            columns,
            affected: Some(total as u64),
            truncated: offset + rows.len() < total,
            rows,
            pageable,
            ms: started.elapsed().as_millis() as u64,
            tx: TxState::Idle,
        })
    }
}

#[cfg(test)]
mod tests;
