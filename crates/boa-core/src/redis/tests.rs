//! Unit tests (no server) and live tests against a Redis server: set
//! `BOA_REDIS_PORT` (no password, on localhost) and optionally
//! `BOA_REDIS_AUTH_PORT` (password `boa-secret`). Live tests write only
//! `boa_test:*` keys in db 15 and delete them.

use super::*;
use crate::connections::{DbKind, SslMode};

fn s(v: &str) -> Value {
    Value::BulkString(v.as_bytes().to_vec())
}

fn args(line: &str) -> Vec<String> {
    split_args(line)
        .unwrap()
        .into_iter()
        .map(|a| String::from_utf8(a).unwrap())
        .collect()
}

#[test]
fn splits_arguments_like_redis_cli() {
    assert_eq!(args("  SET  k   v "), ["SET", "k", "v"]);
    assert_eq!(args(r#"SET "a b" 'c d'"#), ["SET", "a b", "c d"]);
    assert_eq!(
        args(r#"SET k "line\nnext\t\"q\" \\""#),
        ["SET", "k", "line\nnext\t\"q\" \\"]
    );
    assert_eq!(args(r"SET k 'it\'s'"), ["SET", "k", "it's"]);
    assert_eq!(args(r#"SET k "\x41\x42""#), ["SET", "k", "AB"]);
    assert_eq!(args(r#"SET k """#), ["SET", "k", ""]);
    assert_eq!(
        split_args(r#"SET "k"#).unwrap_err().message,
        "Invalid argument(s): unbalanced quotes"
    );
    assert!(split_args(r#"SET "k"x"#).is_err());
    assert!(split_args("GET 'k").is_err());
    assert!(split_args("   ").unwrap().is_empty());
    // A trailing `;` ends the command (SQL habit); quoted, it stays.
    assert_eq!(args("SCAN 0 COUNT 10;"), ["SCAN", "0", "COUNT", "10"]);
    assert_eq!(args("PING ;"), ["PING"]);
    assert_eq!(args("PING;"), ["PING"]);
    assert_eq!(args(r#"SET k "v;""#), ["SET", "k", "v;"]);
    assert_eq!(args("SET k a;b"), ["SET", "k", "a;b"]);
}

#[test]
fn quoting_round_trips() {
    for arg in [
        "plain",
        "a b",
        "",
        "quote\"d",
        "back\\slash",
        "new\nline",
        "tab\t",
        "it's",
        "ünï",
    ] {
        let line = format!("GET {}", quote_arg(arg));
        assert_eq!(args(&line), ["GET", arg], "{line}");
    }
    assert_eq!(quote_arg("user:1"), "user:1");
    assert_eq!(quote_arg("a b"), "\"a b\"");
}

#[test]
fn parses_commands_one_per_line() {
    let text = "# comment\nSET a 1\n\n  // also a comment\nGET a\n";
    let cmds = parse_commands(text).unwrap();
    assert_eq!(cmds.len(), 2);
    assert_eq!(cmds[1], vec![b"GET".to_vec(), b"a".to_vec()]);
    assert!(parse_commands("GET \"a").is_err());
    assert!(parse_commands("\n\n").unwrap().is_empty());
}

#[test]
fn database_names() {
    assert_eq!(db_name(3), "db3");
    assert_eq!(db_index("db3"), Some(3));
    assert_eq!(db_index("12"), Some(12));
    assert_eq!(db_index(" db0 "), Some(0));
    assert_eq!(db_index("postgres"), None);
    let mut c = config(1);
    c.database = String::new();
    assert_eq!(default_db(&c), "db0");
    c.database = "4".into();
    assert_eq!(default_db(&c), "db4");
}

#[test]
fn open_commands_per_type() {
    assert_eq!(open_command("k", "string"), "GET k");
    assert_eq!(open_command("k", "hash"), "HGETALL k");
    assert_eq!(open_command("k", "list"), "LRANGE k 0 -1");
    assert_eq!(open_command("k", "set"), "SMEMBERS k");
    assert_eq!(open_command("k", "zset"), "ZRANGE k 0 -1 WITHSCORES");
    assert_eq!(open_command("k", "stream"), "XRANGE k - +");
    assert_eq!(open_command("k", "ReJSON-RL"), "JSON.GET k");
    assert_eq!(open_command("my key", "string"), "GET \"my key\"");
    assert_eq!(open_command("k", "vectorset"), "TYPE k");
}

#[test]
fn command_under_the_caret_is_its_line() {
    let text = "GET a\n\n  HGETALL b  \n# note\n";
    assert_eq!(command_at(text, 2), Some(0..5));
    assert_eq!(command_at(text, 5), Some(0..5));
    assert_eq!(command_at(text, 6), None);
    let h = text.find("HGETALL").unwrap();
    assert_eq!(command_at(text, h + 3).map(|r| &text[r]), Some("HGETALL b"));
    assert_eq!(command_at(text, text.find("# note").unwrap() + 1), None);
    assert_eq!(command_at(text, text.len()), None);
    assert_eq!(command_at("PING", 4), Some(0..4));
}

#[test]
fn destructive_and_key_changing_commands() {
    assert_eq!(
        destructive_commands("get a\nflushdb\nDEL a b\ndel c"),
        ["FLUSHDB", "DEL"]
    );
    assert!(destructive_commands("GET del\n# FLUSHALL").is_empty());
    assert!(!changes_keys("GET a\nhgetall b\n# DEL x"));
    assert!(changes_keys("GET a\nSET a 1"));
    assert!(changes_keys("EXPIRE a 10"));
}

#[test]
fn highlighting_tokens() {
    let text =
        "# note\nSCAN 0 MATCH user:* COUNT 10\nHSET \"my key\" f 1.5\nCONFIG GET maxmemory\n";
    let tokens = tokenize(text);
    let kinds: Vec<(&str, TokenKind)> = tokens
        .iter()
        .filter(|t| t.kind != TokenKind::Plain)
        .map(|t| (&text[t.range()], t.kind))
        .collect();
    assert_eq!(
        kinds,
        [
            ("# note", TokenKind::Comment),
            ("SCAN", TokenKind::Keyword),
            ("0", TokenKind::Number),
            ("MATCH", TokenKind::Type),
            ("COUNT", TokenKind::Type),
            ("10", TokenKind::Number),
            ("HSET", TokenKind::Keyword),
            ("\"my key\"", TokenKind::Table),
            ("1.5", TokenKind::Number),
            ("CONFIG", TokenKind::Keyword),
            ("GET", TokenKind::Keyword),
        ]
    );
    // Contiguous from 0 to the end.
    assert_eq!(tokens[0].start, 0);
    assert!(tokens.windows(2).all(|w| w[0].end == w[1].start));
    assert_eq!(tokens.last().unwrap().end, text.len());
    assert!(tokenize("").is_empty());
    let t = tokenize("SCAN 0 COUNT 10;");
    let ten = t
        .iter()
        .find(|t| t.kind == TokenKind::Number && t.start > 6)
        .unwrap();
    assert_eq!(ten.range(), 13..15);
}

#[test]
fn key_folders() {
    assert_eq!(folder_ends("a:b::c"), [2, 5]);
    assert_eq!(folder_ends("cache::analyzed-image-1"), [7]);
    assert!(folder_ends("plain").is_empty());
    assert!(folder_ends("trailing:").is_empty());
    assert!(folder_ends(":lead").is_empty());
    assert_eq!(folder_ends("user:1"), [5]);
}

#[test]
fn guard_rail_risks() {
    let r = destructive_risks("GET a\nflushdb\nDEL a b", false);
    assert_eq!(r.len(), 1);
    assert_eq!(r[0].kind, RiskKind::Truncate);
    assert_eq!(r[0].statement, "flushdb");
    let r = destructive_risks("DEL a b\n# FLUSHALL", true);
    assert_eq!(r.len(), 1);
    assert_eq!(r[0].message, "DEL deletes 2 key(s)");
    assert!(destructive_risks("SET a 1", true).is_empty());
}

#[test]
fn scalar_replies_are_one_cell() {
    let cmd = |c: &str| args(c);
    let (cols, rows) = reply_grid(&cmd("GET a"), &s("hello"));
    assert_eq!(cols, vec![ResultColumn::new("value", "text")]);
    assert_eq!(rows, vec![vec![Some("hello".into())]]);
    let (cols, rows) = reply_grid(&cmd("GET a"), &Value::Nil);
    assert_eq!(cols.len(), 1);
    assert_eq!(rows, vec![vec![None]]);
    let (cols, rows) = reply_grid(&cmd("INCR a"), &Value::Int(7));
    assert_eq!(cols[0].ty, "int8");
    assert_eq!(rows[0][0].as_deref(), Some("7"));
    let (_, rows) = reply_grid(&cmd("SET a 1"), &Value::Okay);
    assert_eq!(rows[0][0].as_deref(), Some("OK"));
    let (_, rows) = reply_grid(&cmd("GET b"), &Value::BulkString(vec![0x61, 0xff, 0x00]));
    assert_eq!(rows[0][0].as_deref(), Some("a\\xff\u{0}"));
}

#[test]
fn lists_pairs_streams_and_info() {
    let (cols, rows) = reply_grid(
        &args("HGETALL h"),
        &Value::Array(vec![s("f1"), s("v1"), s("f2"), s("v2")]),
    );
    assert_eq!(
        cols.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(),
        ["field", "value"]
    );
    assert_eq!(rows[1], vec![Some("f2".into()), Some("v2".into())]);

    let (cols, rows) = reply_grid(
        &args("ZRANGE z 0 -1 withscores"),
        &Value::Array(vec![s("m"), s("1.5")]),
    );
    assert_eq!(cols[0].name, "member");
    assert_eq!(rows, vec![vec![Some("m".into()), Some("1.5".into())]]);

    // Without WITHSCORES a ZRANGE is a plain list.
    let (cols, rows) = reply_grid(&args("ZRANGE z 0 -1"), &Value::Array(vec![s("a"), s("b")]));
    assert_eq!(cols[0].name, "#");
    assert_eq!(rows[1], vec![Some("2".into()), Some("b".into())]);

    let (cols, rows) = reply_grid(
        &args("LRANGE l 0 -1"),
        &Value::Array(vec![
            s("x"),
            Value::Nil,
            Value::Array(vec![s("n"), Value::Int(1)]),
        ]),
    );
    assert_eq!(cols[1].name, "value");
    assert_eq!(rows[1][1], None);
    assert_eq!(rows[2][1].as_deref(), Some(r#"["n", 1]"#));

    let (cols, rows) = reply_grid(&args("HELLO"), &Value::Map(vec![(s("server"), s("redis"))]));
    assert_eq!(cols[0].name, "field");
    assert_eq!(rows[0][1].as_deref(), Some("redis"));

    let entry = Value::Array(vec![
        s("1-0"),
        Value::Array(vec![s("a"), s("1"), s("b"), s("2")]),
    ]);
    let (cols, rows) = reply_grid(&args("XRANGE st - +"), &Value::Array(vec![entry]));
    assert_eq!(
        cols.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(),
        ["id", "fields"]
    );
    assert_eq!(rows[0], vec![Some("1-0".into()), Some("a=1 b=2".into())]);

    let scan = Value::Array(vec![s("17"), Value::Array(vec![s("k1"), s("k2")])]);
    let (cols, rows) = reply_grid(&args("SCAN 0"), &scan);
    assert_eq!(cols[1].name, "value (next cursor: 17)");
    assert_eq!(rows.len(), 2);

    let info = s(
        "# Server\r\nredis_version:7.2.4\r\nos:Linux\r\n\r\n# Clients\r\nconnected_clients:1\r\n",
    );
    let (cols, rows) = reply_grid(&args("INFO"), &info);
    assert_eq!(cols.len(), 3);
    assert_eq!(
        rows[0],
        vec![
            Some("Server".into()),
            Some("redis_version".into()),
            Some("7.2.4".into())
        ]
    );
    assert_eq!(rows[2][0].as_deref(), Some("Clients"));
    assert_eq!(
        server_version("# Server\r\nredis_version:7.2.4\r\n"),
        "Redis 7.2.4"
    );
    assert_eq!(
        server_version("redis_version:7.2.4\nvalkey_version:8.0.1\n"),
        "Valkey 8.0.1"
    );
}

// ── Live ─────────────────────────────────────────────────────────────────

fn config(port: u16) -> ConnConfig {
    ConnConfig {
        id: format!("redis-test-{port}"),
        name: "redis test".into(),
        kind: DbKind::Redis,
        host: "localhost".into(),
        port,
        user: String::new(),
        database: "15".into(),
        ssl_mode: SslMode::Disable,
        read_only: false,
        environment: None,
        color: None,
        ssh: None,
    }
}

fn port(var: &str) -> Option<u16> {
    std::env::var(var).ok()?.parse().ok()
}

async fn run(r: &Redis, id: &str, text: &str) -> CmdResult<QueryResult> {
    r.query(id, "db15", text, 1000, 0, &CancelHandle::new())
        .await
}

fn cells(q: &QueryResult) -> Vec<Vec<Option<String>>> {
    q.rows.clone()
}

#[tokio::test(flavor = "multi_thread")]
async fn live_redis() {
    let Some(p) = port("BOA_REDIS_PORT") else {
        eprintln!("BOA_REDIS_PORT not set: skipping");
        return;
    };
    let r = Redis::default();
    let c = config(p);
    let id = c.id.clone();
    assert!(Redis::test(&c, None).await.unwrap().starts_with("Redis "));
    let info = r.connect(c.clone(), None).await.unwrap();
    assert!(info.server_version.starts_with("Redis "));
    assert!(info.databases.len() >= 16);
    assert_eq!(info.databases[15], "db15");
    assert!(r.is_open(&id));

    run(
        &r,
        &id,
        "DEL boa_test:s boa_test:h boa_test:l boa_test:z boa_test:set boa_test:st",
    )
    .await
    .unwrap();
    let setup = "SET boa_test:s hello\n\
                 HSET boa_test:h f1 v1 f2 v2\n\
                 RPUSH boa_test:l a b c\n\
                 ZADD boa_test:z 1 one 2 two\n\
                 SADD boa_test:set x\n\
                 XADD boa_test:st 1-0 a 1";
    let q = run(&r, &id, setup).await.unwrap();
    assert!(!q.pageable);

    let q = run(&r, &id, "GET boa_test:s").await.unwrap();
    assert_eq!(cells(&q), vec![vec![Some("hello".into())]]);
    assert!(q.pageable);
    let q = run(&r, &id, "HGETALL boa_test:h").await.unwrap();
    assert_eq!(q.columns[0].name, "field");
    assert_eq!(q.rows.len(), 2);
    let q = run(&r, &id, "ZRANGE boa_test:z 0 -1 WITHSCORES")
        .await
        .unwrap();
    assert_eq!(cells(&q)[1], vec![Some("two".into()), Some("2".into())]);
    let q = run(&r, &id, "XRANGE boa_test:st - +").await.unwrap();
    assert_eq!(cells(&q)[0], vec![Some("1-0".into()), Some("a=1".into())]);

    // Paging.
    let q = r
        .query(
            &id,
            "db15",
            "LRANGE boa_test:l 0 -1",
            2,
            1,
            &CancelHandle::new(),
        )
        .await
        .unwrap();
    assert_eq!(
        cells(&q),
        vec![
            vec![Some("2".into()), Some("b".into())],
            vec![Some("3".into()), Some("c".into())]
        ]
    );
    assert!(!q.truncated);
    let q = r
        .query(
            &id,
            "db15",
            "LRANGE boa_test:l 0 -1",
            1,
            0,
            &CancelHandle::new(),
        )
        .await
        .unwrap();
    assert!(q.truncated);

    // MULTI runs as one pipeline: the last reply is EXEC's.
    let q = run(&r, &id, "MULTI\nINCR boa_test:n\nINCR boa_test:n\nEXEC")
        .await
        .unwrap();
    assert_eq!(q.rows.len(), 2);
    assert_eq!(q.rows[1][1].as_deref(), Some("2"));

    // Server errors and refused commands.
    let e = run(&r, &id, "HGETALL boa_test:s").await.unwrap_err();
    assert!(e.message.starts_with("WRONGTYPE"), "{}", e.message);
    let e = run(&r, &id, "NOSUCHCOMMAND").await.unwrap_err();
    assert!(e.message.contains("unknown command"), "{}", e.message);
    assert!(run(&r, &id, "SELECT 1").await.is_err());

    // Keys for the explorer.
    let keys = r.keys(&id, "db15", KEY_LIMIT).await.unwrap();
    let mine: Vec<(&str, &str)> = keys
        .keys
        .iter()
        .filter(|k| k.name.starts_with("boa_test:"))
        .map(|k| (k.name.as_str(), k.ty.as_str()))
        .collect();
    assert_eq!(
        mine,
        [
            ("boa_test:h", "hash"),
            ("boa_test:l", "list"),
            ("boa_test:n", "string"),
            ("boa_test:s", "string"),
            ("boa_test:set", "set"),
            ("boa_test:st", "stream"),
            ("boa_test:z", "zset"),
        ]
    );
    assert!(keys.total >= 7);
    let two = r.keys(&id, "db15", 2).await.unwrap();
    assert_eq!(two.keys.len(), 2);
    assert!(two.truncated);

    r.ping(&id).await.unwrap();

    // Read-only: writes refused before they are sent, reads run.
    let mut ro = c.clone();
    ro.id = format!("{id}-ro");
    ro.read_only = true;
    r.connect(ro.clone(), None).await.unwrap();
    let e = run(&r, &ro.id, "SET boa_test:s changed").await.unwrap_err();
    assert_eq!(e.code.as_deref(), Some("read_only"));
    let e = run(&r, &ro.id, "GET boa_test:s\nDEL boa_test:s")
        .await
        .unwrap_err();
    assert_eq!(e.code.as_deref(), Some("read_only"));
    let e = run(&r, &ro.id, "CONFIG SET maxmemory 1").await.unwrap_err();
    assert_eq!(e.code.as_deref(), Some("read_only"));
    let q = run(&r, &ro.id, "GET boa_test:s").await.unwrap();
    assert_eq!(cells(&q), vec![vec![Some("hello".into())]]);
    run(&r, &ro.id, "CONFIG GET maxmemory").await.unwrap();
    r.disconnect(&ro.id).await;

    run(
        &r,
        &id,
        "DEL boa_test:s boa_test:h boa_test:l boa_test:z boa_test:set boa_test:st boa_test:n",
    )
    .await
    .unwrap();
    r.disconnect(&id).await;
    assert!(!r.is_open(&id));
    assert_eq!(
        r.ping(&id).await.unwrap_err().code.as_deref(),
        Some("not_connected")
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn live_redis_password() {
    let Some(p) = port("BOA_REDIS_AUTH_PORT") else {
        eprintln!("BOA_REDIS_AUTH_PORT not set: skipping");
        return;
    };
    let r = Redis::default();
    let c = config(p);
    let e = r.connect(c.clone(), None).await.unwrap_err();
    assert_eq!(
        e.code.as_deref(),
        Some("password_required"),
        "{}",
        e.message
    );
    let e = r
        .connect(c.clone(), Some("wrong".into()))
        .await
        .unwrap_err();
    assert_eq!(e.code.as_deref(), Some("28P01"), "{}", e.message);
    r.connect(c.clone(), Some("boa-secret".into()))
        .await
        .unwrap();
    // A second database's connection authenticates too.
    r.query(&c.id, "db3", "PING", 10, 0, &CancelHandle::new())
        .await
        .unwrap();
    r.disconnect(&c.id).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn unreachable_server() {
    let mut c = config(1);
    c.host = "127.0.0.1".into();
    let e = Redis::test(&c, None).await.unwrap_err();
    assert_eq!(e.code.as_deref(), Some(UNREACHABLE), "{}", e.message);
}
