//! Migration from a Tauri-era install, end to end: a synthetic Tauri config
//! dir (connections.json with passwords and an SSH tunnel,
//! history-<id>.jsonl) and a WebKit localStorage database holding
//! boa.prefs.v2 / boa.session.v1 / boa.csvExport as UTF-16LE blobs, in a temp
//! dir. The app starts on it as app::run does (the one-time import, then
//! AppState::init), and everything must appear: connections with their
//! passwords, history, prefs, the session (tabs, SQL, filters, panels) and
//! the CSV options. Then the app writes every file it owns, and each must
//! still read with the Tauri app's rules:
//! - connections.json and history-<id>.jsonl are read and written by
//!   boa_core::{connections, history}, which are the Tauri backend's
//!   crates/boa-desktop/src/{connections,history}.rs unchanged (only a
//!   Debug derive differs). The test also checks the bytes are what the
//!   Tauri app wrote: an unchanged save is byte-identical.
//! - prefs.json / session.json / csv-export.json hold what the Tauri app
//!   kept in localStorage. The TS rules (state/prefs.ts load,
//!   state/session.ts parseSession, state/csvExport.ts loadOptions) are
//!   checked here as Rust rules and, when BOA_TAURI_UI_SRC points at the
//!   source's crates/boa-desktop/ui/src and node is on PATH, by running
//!   those TS functions themselves (tauri_rules.mjs).

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{Duration, Instant};

use boa_core::connections as store;
use boa_core::error::CmdError;
use boa_core::grid::csv::{CsvDelimiter, CsvOptions};
use boa_core::history::Entry;
use boa_core::state::csv_export as csv_core;
use boa_core::state::history::OutputEntry;
use boa_core::state::local_storage::IMPORT_MARKER;
use boa_core::state::prefs::{self as prefs_core, RightTab, Scope};
use boa_core::state::session::{self as session_core, db_key, parse_session, table_key};
use gpui::{Task, TestAppContext};
use serde_json::{Value, json};

use crate::app_state::AppState;
use crate::models::catalogs::DbKey;
use crate::models::session::{SESSION_FILE, import_local_storage};
use crate::models::workspaces::TabKind;
use crate::tests::{TempDir, init, open_empty};

/// Saved connection ids, as the Tauri app made them (UUIDs).
const PROD: &str = "6f0c1d2e-0000-4000-8000-00000000a11a";
const LOCAL: &str = "6f0c1d2e-0000-4000-8000-00000000b22b";
/// A password that needs JSON escaping.
const PROD_PASSWORD: &str = "s3cr\"et ü/€";

/// connections.json exactly as the Tauri app writes it
/// (serde_json::to_vec_pretty of [{...config, password?}]).
fn tauri_connections_json() -> String {
    format!(
        r#"[
  {{
    "id": "{PROD}",
    "name": "Production",
    "host": "db.internal.example",
    "port": 5432,
    "user": "app",
    "database": "app",
    "sslMode": "verify-full",
    "readOnly": true,
    "environment": "production",
    "color": "red",
    "ssh": {{
      "enabled": true,
      "host": "bastion",
      "port": 2222,
      "user": "ops",
      "auth": "key-file",
      "identityFile": "~/.ssh/id_ed25519"
    }},
    "password": "s3cr\"et ü/€"
  }},
  {{
    "id": "{LOCAL}",
    "name": "Local",
    "host": "localhost",
    "port": 5433,
    "user": "me",
    "database": "dev",
    "sslMode": "prefer",
    "readOnly": false
  }}
]"#
    )
}

/// boa.prefs.v2 as an older Tauri build left it (no rightPanelWidth yet).
const PREFS: &str = r#"{"rowLimit":1000,"confirmDestructive":false,"recordHistory":true,"historyDays":90,"sidebarRowCounts":false,"sidebarViews":true,"recentCount":8,"connectionOrder":["6f0c1d2e-0000-4000-8000-00000000b22b","6f0c1d2e-0000-4000-8000-00000000a11a"],"treeExpanded":{"6f0c1d2e-0000-4000-8000-00000000a11a":true,"6f0c1d2e-0000-4000-8000-00000000a11a/app":true},"scopes":{"6f0c1d2e-0000-4000-8000-00000000a11a":["app","reporting"],"6f0c1d2e-0000-4000-8000-00000000a11a/app":"*"},"bottomHeight":320,"sidebarWidth":300,"rightTab":"history"}"#;

/// boa.session.v1 as JSON.stringify wrote it (U+0000 escaped).
const SESSION: &str = r#"{"active":{"connectionId":"6f0c1d2e-0000-4000-8000-00000000a11a","database":"app"},"workspaces":{"6f0c1d2e-0000-4000-8000-00000000a11a\u0000app":{"tabs":[{"id":"q1","kind":"query","title":"console_1","sql":"select 'héllo ✓' as greeting;\n-- a second line"},{"id":"t2","kind":"table","schema":"public","table":"orders","preview":false,"where":"total > 100","orderBy":"id desc"},{"id":"t3","kind":"table","schema":"public","table":"customers","preview":true}],"activeTab":"t2"},"6f0c1d2e-0000-4000-8000-00000000b22b\u0000dev":{"tabs":[{"id":"q4","kind":"query","title":"console_1","sql":""}],"activeTab":"q4"}},"mru":["6f0c1d2e-0000-4000-8000-00000000a11a\u0000app\u0000public\u0000orders"],"seen":{"6f0c1d2e-0000-4000-8000-00000000a11a\u0000app\u0000public\u0000orders":1795600000000},"wanted":{"6f0c1d2e-0000-4000-8000-00000000a11a":true},"sidebarOpen":true,"rightPanelOpen":false,"bottomOpen":true}"#;

/// boa.csvExport.
const CSV: &str = r#"{"delimiter":";","header":false}"#;

fn now_ms() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as f64
}

/// history-<PROD>.jsonl lines as the Tauri backend wrote them (same serde
/// code), oldest first, within the 90-day retention.
fn tauri_history_lines(now: f64) -> Vec<String> {
    let hour = 3_600_000.;
    let entries = [
        Entry {
            id: "h1".into(),
            sql: "select 1".into(),
            key: Some("select ?".into()),
            database: "app".into(),
            at: (now - 3. * hour).floor(),
            ms: 1.5,
            rows: Some(1.),
            truncated: None,
            affected: None,
            source: Some("console_1".into()),
            error: None,
            detail: None,
        },
        // Saved before entries had keys.
        Entry {
            id: "h2".into(),
            sql: "select * from orders where id = 42".into(),
            key: None,
            database: "app".into(),
            at: (now - 2. * hour).floor(),
            ms: 12.,
            rows: Some(1.),
            truncated: Some(false),
            affected: None,
            source: None,
            error: None,
            detail: None,
        },
        Entry {
            id: "h3".into(),
            sql: "delete from audit_log".into(),
            key: Some("delete from audit_log".into()),
            database: "app".into(),
            at: (now - hour).floor(),
            ms: 3.,
            rows: None,
            truncated: None,
            affected: None,
            source: Some("console_1".into()),
            error: Some("permission denied for table audit_log".into()),
            detail: Some("HINT:  ask the DBA".into()),
        },
    ];
    entries
        .iter()
        .map(|e| serde_json::to_string(e).unwrap())
        .collect()
}

fn history_file(dir: &Path, id: &str) -> PathBuf {
    dir.join(format!("history-{id}.jsonl"))
}

/// A WebKit localStorage database under home (UTF-16LE values; WAL mode,
/// as WebKit keeps it).
fn webkit_local_storage(home: &Path, items: &[(&str, &str)]) {
    let ls = home.join(
        "Library/WebKit/dev.boa.desktop/WebsiteData/Default/aBcD-salt/aBcD-salt/LocalStorage",
    );
    std::fs::create_dir_all(&ls).unwrap();
    let conn = rusqlite::Connection::open(ls.join("localstorage.sqlite3")).unwrap();
    conn.execute_batch(
        "pragma journal_mode = wal;
         create table ItemTable (key text unique on conflict replace, value blob not null on conflict fail);",
    )
    .unwrap();
    for (k, v) in items {
        let utf16: Vec<u8> = v.encode_utf16().flat_map(u16::to_le_bytes).collect();
        conn.execute(
            "insert into ItemTable (key, value) values (?1, ?2)",
            rusqlite::params![k, utf16],
        )
        .unwrap();
    }
}

/// Waits (really) until done: the backend answers on its own threads.
fn wait_until(
    cx: &mut TestAppContext,
    what: &str,
    mut done: impl FnMut(&mut TestAppContext) -> bool,
) {
    let start = Instant::now();
    loop {
        cx.run_until_parked();
        if done(cx) {
            return;
        }
        assert!(
            start.elapsed() < Duration::from_secs(30),
            "timed out waiting for {what}"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// Runs a model task to its end.
fn finish(cx: &mut TestAppContext, what: &str, task: Task<Result<(), CmdError>>) {
    let out = Rc::new(RefCell::new(None));
    cx.update(|cx| {
        let out = out.clone();
        cx.spawn(async move |_| {
            *out.borrow_mut() = Some(task.await);
        })
        .detach();
    });
    wait_until(cx, what, |_| out.borrow().is_some());
    if let Some(Err(e)) = out.borrow_mut().take() {
        panic!("{what}: {}", e.message);
    }
}

fn read(dir: &Path, name: &str) -> String {
    std::fs::read_to_string(dir.join(name)).unwrap_or_else(|e| panic!("{name}: {e}"))
}

#[cfg(unix)]
fn mode(path: &Path) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path).unwrap().permissions().mode() & 0o777
}

// ── The Tauri app's rules for the files that replaced localStorage ────────

/// state/prefs.ts load(): {...DEFAULT_PREFS, ...JSON.parse(raw)}, checking
/// only rightTab and historyDays, so every field in the file must already
/// have the type the TS code expects (anything else reaches the Tauri UI
/// as is).
fn assert_tauri_prefs(raw: &str) {
    let v: Value = serde_json::from_str(raw).expect("prefs.json is JSON");
    let obj = v.as_object().expect("prefs.json is an object");
    for k in [
        "rowLimit",
        "historyDays",
        "recentCount",
        "bottomHeight",
        "sidebarWidth",
        "rightPanelWidth",
    ] {
        assert!(
            obj[k].as_f64().is_some_and(f64::is_finite),
            "{k} is not a number: {raw}"
        );
    }
    for k in [
        "confirmDestructive",
        "recordHistory",
        "sidebarRowCounts",
        "sidebarViews",
    ] {
        assert!(obj[k].is_boolean(), "{k} is not a boolean: {raw}");
    }
    assert!(obj["historyDays"].as_f64().unwrap() >= 0.);
    let order = obj["connectionOrder"].as_array().unwrap();
    assert!(order.iter().all(Value::is_string));
    let expanded = obj["treeExpanded"].as_object().unwrap();
    assert!(expanded.values().all(|v| v == &Value::Bool(true)));
    let scopes = obj["scopes"].as_object().unwrap();
    assert!(
        scopes
            .values()
            .all(|s| s == "*" || s.as_array().is_some_and(|a| a.iter().all(Value::is_string)))
    );
    assert!(["structure", "info", "history"].contains(&obj["rightTab"].as_str().unwrap()));
    // Every field of the Tauri app's Prefs is there.
    for k in prefs_core::FIELDS {
        assert!(obj.contains_key(k), "{k} missing: {raw}");
    }
}

/// state/csvExport.ts loadOptions(): a delimiter of , \t or ; and a boolean
/// header (else that field's default). The file must hold exactly those.
fn assert_tauri_csv(raw: &str) -> CsvOptions {
    let v: Value = serde_json::from_str(raw).unwrap();
    let obj = v.as_object().unwrap();
    assert_eq!(obj.len(), 2, "{raw}");
    assert!([",", "\t", ";"].contains(&obj["delimiter"].as_str().unwrap()));
    assert!(obj["header"].is_boolean());
    csv_core::parse_options(Some(raw))
}

/// session.ts pick(): the top-level keys, in this order.
fn assert_tauri_session_keys(raw: &str) {
    let keys = [
        "active",
        "workspaces",
        "mru",
        "seen",
        "wanted",
        "sidebarOpen",
        "rightPanelOpen",
        "bottomOpen",
    ];
    let at: Vec<usize> = keys
        .iter()
        .map(|k| {
            raw.find(&format!("\"{k}\":"))
                .unwrap_or_else(|| panic!("{k} missing: {raw}"))
        })
        .collect();
    assert_eq!(at[0], 1, "{raw}");
    assert!(at.windows(2).all(|w| w[0] < w[1]), "{raw}");
    let v: Value = serde_json::from_str(raw).unwrap();
    assert_eq!(v.as_object().unwrap().len(), keys.len(), "{raw}");
}

/// With BOA_TAURI_UI_SRC set: runs the Tauri app's own TS parsers
/// (state/prefs.ts, state/session.ts parseSession, state/csvExport.ts) on
/// the files in dir with node, and returns what they read.
fn tauri_ui_check(dir: &Path, connection_ids: &[&str]) -> Option<Value> {
    let src = std::env::var_os("BOA_TAURI_UI_SRC").filter(|s| !s.is_empty())?;
    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/tests/tauri_rules.mjs");
    let out = std::process::Command::new("node")
        .arg(&script)
        .arg(dir)
        .arg(serde_json::to_string(connection_ids).unwrap())
        .env("BOA_TAURI_UI_SRC", src)
        .output()
        .expect("BOA_TAURI_UI_SRC is set but node could not be run");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "tauri_rules.mjs failed: {stdout}{}",
        String::from_utf8_lossy(&out.stderr)
    );
    Some(serde_json::from_str(stdout.trim()).expect("tauri_rules.mjs prints JSON"))
}

// ── The test ──────────────────────────────────────────────────────────────

#[gpui::test]
fn a_tauri_install_carries_over_and_its_files_stay_readable_by_the_tauri_app(
    cx: &mut TestAppContext,
) {
    cx.executor().allow_parking();
    let root = TempDir::new("migration");
    let (config, home) = (root.path().join("config"), root.path().join("home"));
    std::fs::create_dir_all(&config).unwrap();

    // ── A Tauri-era install ──
    let connections_json = tauri_connections_json();
    std::fs::write(config.join("connections.json"), &connections_json).unwrap();
    let now = now_ms();
    let history_lines = tauri_history_lines(now);
    let history_path = history_file(&config, PROD);
    std::fs::write(
        &history_path,
        history_lines
            .iter()
            .map(|l| format!("{l}\n"))
            .collect::<String>(),
    )
    .unwrap();
    webkit_local_storage(
        &home,
        &[
            ("boa.prefs.v2", PREFS),
            ("boa.session.v1", SESSION),
            ("boa.csvExport", CSV),
            ("some-other-key", "not ours"),
        ],
    );

    // ── Launch as app::run does: the import, then the models ──
    let report = import_local_storage(&config, &home).expect("the import runs on a new dir");
    assert!(report.source.is_some());
    assert_eq!(
        report.imported,
        ["prefs.json", "session.json", "csv-export.json"]
    );
    // The values land unchanged.
    assert_eq!(read(&config, "prefs.json"), PREFS);
    assert_eq!(read(&config, SESSION_FILE), SESSION);
    assert_eq!(read(&config, csv_core::FILE), CSV);
    assert!(config.join(IMPORT_MARKER).exists());
    init(cx, &config);
    let s = cx.update(|cx| AppState::global(cx).clone());
    // Nothing connects here: there are no servers behind these connections.
    cx.update(|cx| {
        s.catalogs.update(cx, |c, _| {
            c.offline.extend([PROD.to_owned(), LOCAL.to_owned()]);
        })
    });
    wait_until(cx, "the saved connections", |cx| {
        cx.read(|cx| s.connections.read(cx).is_loaded())
    });

    // Connections in the explorer's order (prefs connectionOrder), with the
    // stored password where there was one.
    cx.read(|cx| {
        let c = s.connections.read(cx);
        let ids: Vec<&str> = c.saved.iter().map(|c| c.config.id.as_str()).collect();
        assert_eq!(ids, [LOCAL, PROD]);
        let prod = c.get(PROD).unwrap();
        assert!(prod.has_password);
        assert!(prod.config.read_only);
        assert_eq!(prod.config.ssl_mode, store::SslMode::VerifyFull);
        assert_eq!(
            prod.config.environment,
            Some(store::Environment::Production)
        );
        let ssh = prod.config.tunnel().expect("the SSH tunnel is kept");
        assert_eq!((ssh.host.as_str(), ssh.port), ("bastion", Some(2222)));
        assert_eq!(ssh.auth, store::SshAuth::KeyFile);
        assert!(!c.get(LOCAL).unwrap().has_password);
    });
    assert_eq!(
        store::password(&config, PROD).unwrap().as_deref(),
        Some(PROD_PASSWORD)
    );
    assert_eq!(store::password(&config, LOCAL).unwrap(), None);

    // Query history.
    cx.update(|cx| s.history.update(cx, |h, cx| h.load(PROD, false, cx)));
    wait_until(cx, "the history", |cx| {
        cx.read(|cx| s.history.read(cx).list(PROD).is_some())
    });
    cx.read(|cx| {
        let list = s.history.read(cx).list(PROD).unwrap();
        let ids: Vec<&str> = list.iter().map(|e| e.id.as_str()).collect();
        assert_eq!(ids, ["h3", "h2", "h1"], "newest first");
        assert_eq!(
            list[0].error.as_deref(),
            Some("permission denied for table audit_log")
        );
    });

    // Prefs (rightPanelWidth was not saved yet: the default).
    cx.read(|cx| {
        let p = s.prefs.read(cx).get();
        assert_eq!(p.row_limit, 1000);
        assert!(!p.confirm_destructive);
        assert_eq!(p.history_days, 90);
        assert!(!p.sidebar_row_counts);
        assert_eq!(p.recent_count, 8);
        assert_eq!(p.connection_order, [LOCAL, PROD]);
        assert!(p.is_expanded(&format!("{PROD}/app")));
        assert_eq!(p.scopes[&format!("{PROD}/app")], Scope::All);
        assert_eq!(
            p.scopes[PROD],
            Scope::Some(vec!["app".into(), "reporting".into()])
        );
        assert_eq!((p.bottom_height, p.sidebar_width), (320, 300));
        assert_eq!(p.right_panel_width, 340);
        assert_eq!(p.right_tab, RightTab::History);
    });

    // The session: tabs with their SQL, filters and preview flags, the
    // active tab and database, MRU / seen and the panels. Nothing ran.
    let prod_app = DbKey::new(PROD, "app");
    let local_dev = DbKey::new(LOCAL, "dev");
    cx.read(|cx| {
        let w = s.workspaces.read(cx);
        assert_eq!(w.active.as_ref(), Some(&prod_app));
        let ws = w.workspace(&prod_app).unwrap();
        let labels: Vec<String> = ws.tabs.iter().map(|t| t.label().to_string()).collect();
        assert_eq!(labels, ["console_1", "orders", "customers"]);
        assert_eq!(ws.active_tab.as_deref(), Some("t2"));
        let console = ws.tabs[0].console().unwrap().read(cx);
        assert_eq!(
            console.sql,
            "select 'héllo ✓' as greeting;\n-- a second line"
        );
        assert!(console.result.is_none() && console.running.is_none());
        let orders = ws.tabs[1].table().unwrap().read(cx);
        assert_eq!(
            (orders.where_clause.as_str(), orders.order_by.as_str()),
            ("total > 100", "id desc")
        );
        assert!(orders.result.is_none());
        assert!(matches!(
            ws.tabs[2].kind,
            TabKind::Table { preview: true, .. }
        ));
        assert_eq!(w.workspace(&local_dev).unwrap().tabs.len(), 1);
        let orders_key = table_key(PROD, "app", "public", "orders");
        assert_eq!(w.mru, std::slice::from_ref(&orders_key));
        assert_eq!(w.seen.get(&orders_key), Some(&1_795_600_000_000));
        let l = s.layout.read(cx);
        assert!(l.sidebar_open() && !l.right_panel_open() && l.bottom_open());
        let restored = s.session.read(cx).restored().expect("a restored session");
        assert_eq!(session_core::connections_to_restore(restored), [PROD]);
    });

    // CSV options.
    assert_eq!(
        cx.read(|cx| s.exports.read(cx).options()),
        CsvOptions {
            delimiter: CsvDelimiter::Semicolon,
            header: false
        }
    );

    // The main window draws the restored state.
    let (_root, vcx) = open_empty(cx);
    vcx.run_until_parked();
    drop(vcx);

    // ── The GPUI app writes every file it owns ──
    // An unchanged save first: connections.json comes out byte for byte.
    let (prod_config, mut local_config) = cx.read(|cx| {
        let c = s.connections.read(cx);
        (
            c.get(PROD).unwrap().config.clone(),
            c.get(LOCAL).unwrap().config.clone(),
        )
    });
    let task = cx.update(|cx| {
        s.connections
            .update(cx, |c, cx| c.save(prod_config, None, true, cx))
    });
    finish(cx, "the unchanged save", task);
    assert_eq!(read(&config, "connections.json"), connections_json);
    local_config.name = "Local (edited)".into();
    let task = cx.update(|cx| {
        s.connections
            .update(cx, |c, cx| c.save(local_config, None, true, cx))
    });
    finish(cx, "the edit", task);
    cx.update(|cx| {
        s.prefs.update(cx, |p, cx| {
            p.update(cx, |p| {
                p.row_limit = 5000;
                p.set_expanded(&format!("{LOCAL}/dev"), true);
            })
        });
        s.exports.update(cx, |e, cx| {
            e.set_options(
                CsvOptions {
                    delimiter: CsvDelimiter::Tab,
                    header: true,
                },
                cx,
            )
        });
        s.workspaces.update(cx, |w, cx| {
            w.new_query_tab_with(local_dev.clone(), "select now();".into(), cx)
        });
        s.history.update(cx, |h, cx| {
            h.record(
                PROD,
                &OutputEntry {
                    sql: "select count(*) from orders".into(),
                    database: "app".into(),
                    at: now,
                    ms: 2.,
                    rows: Some(1.),
                    source: Some("console_1".into()),
                    ..OutputEntry::default()
                },
                cx,
            )
        });
    });
    cx.executor().advance_clock(Duration::from_secs(3));
    wait_until(cx, "the history append", |_| {
        std::fs::read_to_string(&history_path).is_ok_and(|t| t.lines().count() == 4)
    });
    cx.executor().advance_clock(Duration::from_secs(3));
    cx.run_until_parked();

    // ── What the Tauri app reads after a downgrade ──
    // connections.json: read by the Tauri backend's own code; the bytes are
    // what it writes itself (pretty JSON, camelCase, password last).
    assert_eq!(
        read(&config, "connections.json"),
        connections_json.replace(r#""name": "Local","#, r#""name": "Local (edited)","#)
    );
    assert_eq!(store::list(&config).unwrap().len(), 2);
    assert_eq!(
        store::password(&config, PROD).unwrap().as_deref(),
        Some(PROD_PASSWORD)
    );

    // history-<id>.jsonl: one Entry per line; the Tauri lines unchanged.
    let text = std::fs::read_to_string(&history_path).unwrap();
    let lines: Vec<&str> = text.lines().collect();
    for l in &history_lines {
        assert!(lines.contains(&l.as_str()), "{l} changed:\n{text}");
    }
    let entries: Vec<Entry> = lines
        .iter()
        .map(|l| serde_json::from_str(l).expect("an Entry"))
        .collect();
    assert!(
        entries
            .iter()
            .any(|e| e.sql == "select count(*) from orders" && e.key.is_some())
    );
    assert!(text.ends_with('\n'));

    // prefs.json: state/prefs.ts load() rules.
    let prefs_raw = read(&config, "prefs.json");
    assert_tauri_prefs(&prefs_raw);
    let reread = prefs_core::Prefs::from_json(&prefs_raw);
    assert_eq!(reread, cx.read(|cx| s.prefs.read(cx).get().clone()));
    assert_eq!(reread.row_limit, 5000);

    // session.json: state/session.ts parseSession (ported with its test
    // cases) keeps all of it, and the keys are in pick()'s order.
    let session_raw = read(&config, SESSION_FILE);
    let parsed = parse_session(Some(&session_raw), [PROD, LOCAL]).expect("parses");
    assert_eq!(parsed.to_json(), session_raw);
    assert_tauri_session_keys(&session_raw);
    let local_tabs = &parsed.workspaces[&db_key(LOCAL, "dev")].tabs;
    assert_eq!(local_tabs.len(), 2);
    assert!(matches!(&local_tabs[1], session_core::Tab::Query(q) if q.sql == "select now();"));
    assert_eq!(parsed.workspaces[&db_key(PROD, "app")].tabs.len(), 3);

    // csv-export.json: state/csvExport.ts loadOptions rules.
    let csv_raw = read(&config, csv_core::FILE);
    assert_eq!(
        assert_tauri_csv(&csv_raw),
        CsvOptions {
            delimiter: CsvDelimiter::Tab,
            header: true
        }
    );
    assert_eq!(csv_raw, r#"{"delimiter":"\t","header":true}"#);

    // Owner-only, like the Tauri app's own files.
    #[cfg(unix)]
    for f in [
        config.join("connections.json"),
        history_path.clone(),
        config.join(SESSION_FILE),
        config.join(csv_core::FILE),
    ] {
        assert_eq!(mode(&f), 0o600, "{}", f.display());
    }

    // The import never runs again: the app's changes are not overwritten.
    assert_eq!(import_local_storage(&config, &home), None);
    assert_eq!(read(&config, "prefs.json"), prefs_raw);

    // The Tauri app's own TS parsers, when the source is at hand.
    match tauri_ui_check(&config, &[PROD, LOCAL]) {
        None => println!("BOA_TAURI_UI_SRC not set: the TS parsers were not run"),
        Some(v) => {
            assert_eq!(v["prefsLossless"], json!(true), "{v}");
            assert_eq!(v["sessionLossless"], json!(true), "{v}");
            assert_eq!(v["csvLossless"], json!(true), "{v}");
            assert_eq!(v["prefs"]["rowLimit"], json!(5000));
            assert_eq!(v["csv"], json!({"delimiter": "\t", "header": true}));
            let tabs = &v["session"]["workspaces"][db_key(LOCAL, "dev")]["tabs"];
            assert_eq!(tabs.as_array().map(Vec::len), Some(2), "{v}");
            println!("the Tauri app's TS parsers read every file losslessly");
        }
    }
}

/// Rehearsal on a COPY of a real Tauri install (never the originals):
/// BOA_MIGRATION_DIR=<dir> holding config/ (a copy of the config dir, without
/// the files only the GPUI app writes: prefs.json, session.json,
/// csv-export.json, .localstorage-imported) and
/// home/Library/WebKit/dev.boa.desktop (a copy of WebKit's data). Imports,
/// starts the app models (nothing connects), reads every history file,
/// then has the app write the files it owns and checks them with the
/// Tauri app's rules (and its TS parsers with BOA_TAURI_UI_SRC). Prints
/// counts and yes/no answers only: no SQL, names or hosts.
/// BOA_MIGRATION_DIR=… cargo test -p boa-app -- --ignored real_install_copy --nocapture
#[gpui::test]
#[ignore]
fn real_install_copy(cx: &mut TestAppContext) {
    let Some(root) = std::env::var_os("BOA_MIGRATION_DIR").map(PathBuf::from) else {
        println!("BOA_MIGRATION_DIR not set");
        return;
    };
    cx.executor().allow_parking();
    let (config, home) = (root.join("config"), root.join("home"));
    let connections_before = read(&config, "connections.json");
    let report = import_local_storage(&config, &home);
    match &report {
        Some(r) => println!(
            "import: localStorage found: {}, imported {:?}, skipped {:?}",
            r.source.is_some(),
            r.imported,
            r.skipped
        ),
        None => println!("import: not run (already imported)"),
    }
    init(cx, &config);
    let s = cx.update(|cx| AppState::global(cx).clone());
    wait_until(cx, "the saved connections", |cx| {
        cx.read(|cx| s.connections.read(cx).is_loaded())
    });
    let ids: Vec<String> = cx.read(|cx| {
        s.connections
            .read(cx)
            .saved
            .iter()
            .map(|c| c.config.id.clone())
            .collect()
    });
    cx.update(|cx| s.catalogs.update(cx, |c, _| c.offline.extend(ids.clone())));
    let with_password = ids
        .iter()
        .filter(|id| store::password(&config, id).ok().flatten().is_some())
        .count();
    let with_ssh = cx.read(|cx| {
        s.connections
            .read(cx)
            .saved
            .iter()
            .filter(|c| c.config.tunnel().is_some())
            .count()
    });
    println!(
        "connections: {} saved, {with_password} with a stored password, {with_ssh} through SSH",
        ids.len()
    );

    // History, through the app's model (retention applies, as in the app).
    for id in &ids {
        cx.update(|cx| s.history.update(cx, |h, cx| h.load(id, false, cx)));
    }
    wait_until(cx, "the histories", |cx| {
        cx.read(|cx| ids.iter().all(|id| s.history.read(cx).list(id).is_some()))
    });
    let mut on_disk = 0;
    let mut unreadable = 0;
    for id in &ids {
        if let Ok(text) = std::fs::read_to_string(history_file(&config, id)) {
            for l in text.lines().filter(|l| !l.trim().is_empty()) {
                on_disk += 1;
                if serde_json::from_str::<Entry>(l).is_err() {
                    unreadable += 1;
                }
            }
        }
    }
    let shown: usize = cx.read(|cx| {
        ids.iter()
            .map(|id| s.history.read(cx).list(id).map_or(0, <[Entry]>::len))
            .sum()
    });
    println!(
        "history: {on_disk} lines on disk ({unreadable} unreadable), {shown} shown (within historyDays)"
    );

    cx.read(|cx| {
        let w = s.workspaces.read(cx);
        let tabs: Vec<_> = w.workspaces.values().flat_map(|ws| &ws.tabs).collect();
        let tables = tabs.iter().filter(|t| t.table().is_some()).count();
        let consoles_with_sql = tabs
            .iter()
            .filter_map(|t| t.console())
            .filter(|c| !c.read(cx).sql.trim().is_empty())
            .count();
        let l = s.layout.read(cx);
        println!(
            "session: {} databases, {} tabs ({tables} table tabs, {consoles_with_sql} consoles with SQL), active database: {}, mru {}, panels sidebar/right/bottom {}/{}/{}, to reconnect: {:?}",
            w.workspaces.len(),
            tabs.len(),
            w.active.is_some(),
            w.mru.len(),
            l.sidebar_open(),
            l.right_panel_open(),
            l.bottom_open(),
            s.session
                .read(cx)
                .restored()
                .map(|r| session_core::connections_to_restore(r).len())
        );
        let p = s.prefs.read(cx).get();
        println!(
            "prefs: rowLimit {}, historyDays {}, connectionOrder {}, scopes {}, expanded {}",
            p.row_limit,
            p.history_days,
            p.connection_order.len(),
            p.scopes.len(),
            p.tree_expanded.len()
        );
        println!("csv options: {:?}", s.exports.read(cx).options());
    });

    // The app writes what it owns: every connection saved unchanged, prefs
    // and CSV options changed and changed back, the session.
    let configs: Vec<_> = cx.read(|cx| {
        s.connections
            .read(cx)
            .saved
            .iter()
            .map(|c| c.config.clone())
            .collect()
    });
    for c in configs {
        let task = cx.update(|cx| s.connections.update(cx, |m, cx| m.save(c, None, true, cx)));
        finish(cx, "an unchanged save", task);
    }
    let (recent, options) = cx.read(|cx| {
        (
            s.prefs.read(cx).get().recent_count,
            s.exports.read(cx).options(),
        )
    });
    cx.update(|cx| {
        s.prefs
            .update(cx, |p, cx| p.update(cx, |p| p.recent_count = recent + 1));
        s.prefs
            .update(cx, |p, cx| p.update(cx, |p| p.recent_count = recent));
        let other = CsvOptions {
            header: !options.header,
            ..options
        };
        s.exports.update(cx, |e, cx| e.set_options(other, cx));
        s.exports.update(cx, |e, cx| e.set_options(options, cx));
        s.session.update(cx, |s, cx| s.write(cx));
    });
    cx.executor().advance_clock(Duration::from_secs(3));
    cx.run_until_parked();

    println!(
        "connections.json byte-identical after saving every connection: {}",
        read(&config, "connections.json") == connections_before
    );
    let prefs_raw = read(&config, "prefs.json");
    assert_tauri_prefs(&prefs_raw);
    println!("prefs.json follows the prefs.ts rules: true");
    let session_raw = read(&config, SESSION_FILE);
    let parsed = parse_session(Some(&session_raw), ids.iter().map(String::as_str));
    println!(
        "session.json parses with the session.ts rules, losslessly: {}",
        parsed.is_some_and(|p| p.to_json() == session_raw)
    );
    assert_tauri_session_keys(&session_raw);
    assert_tauri_csv(&read(&config, csv_core::FILE));
    println!("csv-export.json follows the csvExport.ts rules: true");
    let id_refs: Vec<&str> = ids.iter().map(String::as_str).collect();
    match tauri_ui_check(&config, &id_refs) {
        None => println!("BOA_TAURI_UI_SRC not set: the TS parsers were not run"),
        Some(v) => println!(
            "the Tauri app's TS parsers, lossless: prefs {}, session {}, csv {}",
            v["prefsLossless"], v["sessionLossless"], v["csvLossless"]
        ),
    }
}
