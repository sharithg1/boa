//! One-time import of the Tauri app's WebView localStorage, and the small
//! file helpers for the JSON files that replace it.
//!
//! The Tauri app kept its preferences (\`boa.prefs.v2\`, state/prefs.ts), the
//! last session (\`boa.session.v1\`, state/session.ts) and the CSV options
//! (\`boa.csvExport\`, state/csvExport.ts) in WKWebView localStorage. WebKit
//! stores it in SQLite at
//! \`~/Library/WebKit/dev.boa.desktop/WebsiteData/Default/<salt>/<salt>/LocalStorage/localstorage.sqlite3\`
//! (\`ItemTable(key TEXT, value BLOB)\`, values UTF-16LE, verified on a real
//! install). The GPUI app keeps them as \`prefs.json\`, \`session.json\` and
//! \`csv-export.json\` in the config dir, with the raw JSON values unchanged,
//! so field names stay identical and the Tauri parsers' rules still apply.
//!
//! [\`import_if_needed\`] copies the database (+ \`-wal\`, \`-shm\`) to a temp
//! dir and reads the copy; the original is never opened. It writes each file
//! only when it does not exist yet, and never fails: anything that goes
//! wrong is reported as skipped.

use std::collections::HashMap;
use std::fmt;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// The preferences file in the config dir (state/prefs.ts, \`boa.prefs.v2\`).
pub const PREFS_FILE: &str = "prefs.json";

/// The localStorage key → config-dir file pairs that are imported.
pub const IMPORTS: [(&str, &str); 3] = [
    ("boa.prefs.v2", PREFS_FILE),
    (super::session::LOCAL_STORAGE_KEY, super::session::FILE),
    (
        super::csv_export::LOCAL_STORAGE_KEY,
        super::csv_export::FILE,
    ),
];

// ── Config files ──────────────────────────────────────────────────────────

/// \`<dir>/<name>\` as text; \`None\` when missing or unreadable.
pub fn read_config_file(dir: &Path, name: &str) -> Option<String> {
    std::fs::read_to_string(dir.join(name)).ok()
}

static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

/// A name no other writer in this process (or another one) uses.
fn unique(prefix: &str) -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.subsec_nanos());
    let n = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
    format!("{prefix}.{}-{nanos}-{n}", std::process::id())
}

/// Writes \`<dir>/<name>\` through a temp file and a rename, so a crash never
/// leaves half a file; owner-only (it can hold the user's SQL). Creates \`dir\`.
pub fn write_config_file(dir: &Path, name: &str, contents: &str) -> io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let tmp = dir.join(unique(&format!(".{name}")) + ".tmp");
    let res = (|| {
        std::fs::write(&tmp, contents)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600))?;
        }
        std::fs::rename(&tmp, dir.join(name))
    })();
    if res.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    res
}

// ── Reading localStorage ──────────────────────────────────────────────────

/// Why the localStorage database could not be read.
#[derive(Debug)]
pub enum LocalStorageError {
    Io(io::Error),
    Sqlite(rusqlite::Error),
}

impl fmt::Display for LocalStorageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(e) => write!(f, "{e}"),
            Self::Sqlite(e) => write!(f, "SQLite: {e}"),
        }
    }
}

impl std::error::Error for LocalStorageError {}

impl From<io::Error> for LocalStorageError {
    fn from(e: io::Error) -> Self {
        Self::Io(e)
    }
}

impl From<rusqlite::Error> for LocalStorageError {
    fn from(e: rusqlite::Error) -> Self {
        Self::Sqlite(e)
    }
}

/// The app's localStorage database under \`home\`, if there is one (the most
/// recently modified when WebKit left several salts).
pub fn find_local_storage_db(home: &Path) -> Option<PathBuf> {
    let root = home
        .join("Library/WebKit")
        .join(crate::config::APP_IDENTIFIER)
        .join("WebsiteData/Default");
    let mut found: Vec<(std::time::SystemTime, PathBuf)> = Vec::new();
    for outer in std::fs::read_dir(&root).ok()?.flatten() {
        let Ok(inner) = std::fs::read_dir(outer.path()) else {
            continue;
        };
        for inner in inner.flatten() {
            let db = inner.path().join("LocalStorage/localstorage.sqlite3");
            if let Ok(meta) = std::fs::metadata(&db)
                && meta.is_file()
            {
                let at = meta.modified().unwrap_or(std::time::UNIX_EPOCH);
                found.push((at, db));
            }
        }
    }
    found
        .into_iter()
        .max_by_key(|(at, _)| *at)
        .map(|(_, db)| db)
}

/// A temp dir removed when dropped.
struct TempDir(PathBuf);

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A localStorage value: UTF-16LE in a BLOB (WebKit), or plain TEXT.
fn decode(v: rusqlite::types::ValueRef<'_>) -> Option<String> {
    use rusqlite::types::ValueRef;
    match v {
        ValueRef::Blob(b) if b.len().is_multiple_of(2) => {
            let units: Vec<u16> = b
                .as_chunks::<2>()
                .0
                .iter()
                .map(|c| u16::from_le_bytes(*c))
                .collect();
            String::from_utf16(&units).ok()
        }
        ValueRef::Text(t) => std::str::from_utf8(t).ok().map(str::to_owned),
        _ => None,
    }
}

/// Every key / value of the localStorage database \`db\`. Reads a copy (with
/// its \`-wal\` / \`-shm\`, so recent writes are included) in a temp dir, never
/// the file itself. Values that are not valid UTF-16 are left out.
pub fn read_items(db: &Path) -> Result<HashMap<String, String>, LocalStorageError> {
    let tmp = TempDir(std::env::temp_dir().join(unique("boa-localstorage")));
    std::fs::create_dir(&tmp.0)?;
    let copy = tmp.0.join("localstorage.sqlite3");
    std::fs::copy(db, &copy)?;
    for suffix in ["-wal", "-shm"] {
        let mut side = db.as_os_str().to_owned();
        side.push(suffix);
        let side = PathBuf::from(side);
        if side.is_file() {
            let mut dest = copy.as_os_str().to_owned();
            dest.push(suffix);
            std::fs::copy(&side, PathBuf::from(dest))?;
        }
    }
    let conn = rusqlite::Connection::open_with_flags(
        &copy,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    let mut items = HashMap::new();
    {
        let mut stmt = conn.prepare("select key, value from ItemTable")?;
        let mut rows = stmt.query([])?;
        while let Some(row) = rows.next()? {
            let Ok(key) = row.get::<_, String>(0) else {
                continue;
            };
            if let Some(value) = row.get_ref(1).ok().and_then(decode) {
                items.insert(key, value);
            }
        }
    }
    drop(conn);
    Ok(items)
}

// ── Import ────────────────────────────────────────────────────────────────

/// Written into the config dir once the import has run ([`import_once`]),
/// whatever it found: the import is a one-time migration, and
/// `csv-export.json` may legitimately never be written, so "a file is
/// missing" alone can't say whether it ran.
pub const IMPORT_MARKER: &str = ".localstorage-imported";

/// What [\`import_if_needed\`] did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ImportReport {
    /// The localStorage database read, if it got that far.
    pub source: Option<PathBuf>,
    /// Files written (\`prefs.json\`, …).
    pub imported: Vec<&'static str>,
    /// Files not written, and why (exists already, not in localStorage, …).
    pub skipped: Vec<(&'static str, String)>,
}

/// Imports \`boa.prefs.v2\` / \`boa.session.v1\` / \`boa.csvExport\` from the
/// Tauri app's localStorage under \`home\` into \`prefs.json\` / \`session.json\` /
/// \`csv-export.json\` in \`config_dir\`, each only when that file does not exist
/// yet and the value is a JSON object (written as is). Never fails or panics.
pub fn import_if_needed(config_dir: &Path, home: &Path) -> ImportReport {
    let mut report = ImportReport::default();
    let mut wanted = Vec::new();
    for (key, file) in IMPORTS {
        // An unreadable state counts as present: never overwrite.
        match config_dir.join(file).try_exists() {
            Ok(false) => wanted.push((key, file)),
            Ok(true) => report.skipped.push((file, "exists".into())),
            Err(e) => report.skipped.push((file, format!("cannot check: {e}"))),
        }
    }
    if wanted.is_empty() {
        return report;
    }
    let skip_all = |report: &mut ImportReport, why: &str| {
        for (_, file) in &wanted {
            report.skipped.push((file, why.to_owned()));
        }
    };
    let Some(db) = find_local_storage_db(home) else {
        skip_all(&mut report, "no localStorage");
        return report;
    };
    report.source = Some(db.clone());
    let items = match read_items(&db) {
        Ok(items) => items,
        Err(e) => {
            tracing::warn!("localStorage import: cannot read {}: {e}", db.display());
            skip_all(&mut report, &format!("cannot read localStorage: {e}"));
            return report;
        }
    };
    for (key, file) in &wanted {
        let Some(value) = items.get(*key) else {
            report.skipped.push((file, "not in localStorage".into()));
            continue;
        };
        if !matches!(
            serde_json::from_str::<serde_json::Value>(value),
            Ok(serde_json::Value::Object(_))
        ) {
            report.skipped.push((file, "not a JSON object".into()));
            continue;
        }
        match write_config_file(config_dir, file, value) {
            Ok(()) => report.imported.push(file),
            Err(e) => report.skipped.push((file, format!("cannot write: {e}"))),
        }
    }
    if !report.imported.is_empty() {
        tracing::info!("imported {:?} from localStorage", report.imported);
    }
    report
}

/// The app's startup import: [`import_if_needed`] at most once per config
/// dir. `None` (nothing done) when [`IMPORT_MARKER`] is there, or when
/// `prefs.json` and `session.json` both exist already (a config dir the
/// GPUI app has used; marked then too). Otherwise imports, writes the
/// marker (a small JSON note of what was imported) and returns the report.
/// Never fails: a marker that can't be written is logged (the import runs
/// again next launch, and writes only files that are still missing).
pub fn import_once(config_dir: &Path, home: &Path) -> Option<ImportReport> {
    let marker = config_dir.join(IMPORT_MARKER);
    // An unreadable state counts as done: never risk importing twice.
    if marker.try_exists().unwrap_or(true) {
        return None;
    }
    let has = |file: &str| config_dir.join(file).try_exists().unwrap_or(true);
    let report = if has(PREFS_FILE) && has(super::session::FILE) {
        None
    } else {
        Some(import_if_needed(config_dir, home))
    };
    let note = serde_json::json!({
        "at": std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_millis() as u64),
        "imported": report.as_ref().map(|r| r.imported.clone()).unwrap_or_default(),
    });
    if let Err(e) = write_config_file(config_dir, IMPORT_MARKER, &note.to_string()) {
        tracing::warn!(
            "localStorage import: cannot write {}: {e}",
            marker.display()
        );
    }
    report
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A fresh empty dir under the system temp dir.
    pub(crate) fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(unique(&format!("boa-core-test-{name}")));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    use rusqlite::types::Value as Sql;

    /// A value as WebKit stores it: UTF-16LE in a BLOB.
    fn utf16le(s: &str) -> Sql {
        Sql::Blob(s.encode_utf16().flat_map(u16::to_le_bytes).collect())
    }

    /// A fake home with a WebKit localStorage db holding \`items\` (UTF-16LE blobs).
    fn fake_home(name: &str, items: &[(&str, Sql)]) -> PathBuf {
        let home = temp_dir(name);
        let ls = home
            .join("Library/WebKit/dev.boa.desktop/WebsiteData/Default/salty/salty/LocalStorage");
        std::fs::create_dir_all(&ls).unwrap();
        std::fs::write(
            home.join("Library/WebKit/dev.boa.desktop/WebsiteData/Default/salt"),
            "x",
        )
        .unwrap();
        let conn = rusqlite::Connection::open(ls.join("localstorage.sqlite3")).unwrap();
        conn.execute_batch(
            "pragma journal_mode = wal;
             create table ItemTable (key text unique on conflict replace, value blob not null on conflict fail);",
        )
        .unwrap();
        for (k, v) in items {
            conn.execute(
                "insert into ItemTable (key, value) values (?1, ?2)",
                rusqlite::params![k, v],
            )
            .unwrap();
        }
        // Leave the rows in the -wal (no checkpoint): the import must see them.
        std::mem::forget(conn);
        home
    }

    const PREFS: &str = r#"{"theme":"dark","rowLimit":500,"treeExpanded":{"c1":true}}"#;
    const SESSION: &str = r#"{"active":null,"workspaces":{},"mru":[],"seen":{},"note":"é – 😀"}"#;

    #[test]
    fn finds_the_database_under_a_salt() {
        let home = fake_home("ls-find", &[]);
        let db = find_local_storage_db(&home).unwrap();
        assert!(db.ends_with("salty/salty/LocalStorage/localstorage.sqlite3"));
        assert_eq!(find_local_storage_db(&home.join("nowhere")), None);
        std::fs::remove_dir_all(&home).unwrap();
    }

    #[test]
    fn reads_utf16le_values_from_a_copy() {
        let home = fake_home(
            "ls-read",
            &[
                ("boa.session.v1", utf16le(SESSION)),
                ("odd", Sql::Blob(vec![0x41])),
                ("text", Sql::Text("plain".into())),
            ],
        );
        let db = find_local_storage_db(&home).unwrap();
        let before = std::fs::read(&db).unwrap();
        let items = read_items(&db).unwrap();
        assert_eq!(
            items.get("boa.session.v1").map(String::as_str),
            Some(SESSION)
        );
        assert_eq!(items.get("odd"), None);
        assert_eq!(items.get("text").map(String::as_str), Some("plain"));
        assert_eq!(
            std::fs::read(&db).unwrap(),
            before,
            "the original is untouched"
        );
        std::fs::remove_dir_all(&home).unwrap();
    }

    #[test]
    fn imports_missing_files_only_and_keeps_the_raw_json() {
        let home = fake_home(
            "ls-import",
            &[
                ("boa.prefs.v2", utf16le(PREFS)),
                ("boa.session.v1", utf16le(SESSION)),
                (
                    "boa.csvExport",
                    utf16le(r#"{"delimiter":";","header":false}"#),
                ),
            ],
        );
        let config = temp_dir("ls-import-config").join("dev.boa.desktop");
        // An existing session.json is never replaced.
        std::fs::create_dir_all(&config).unwrap();
        std::fs::write(config.join("session.json"), "{}").unwrap();

        let r = import_if_needed(&config, &home);
        assert_eq!(r.imported, ["prefs.json", "csv-export.json"]);
        assert_eq!(r.skipped, [("session.json", "exists".to_owned())]);
        assert!(r.source.is_some());
        assert_eq!(
            std::fs::read_to_string(config.join("prefs.json")).unwrap(),
            PREFS
        );
        assert_eq!(
            std::fs::read_to_string(config.join("session.json")).unwrap(),
            "{}"
        );
        assert_eq!(
            super::super::csv_export::load(&config),
            crate::grid::csv::CsvOptions {
                delimiter: crate::grid::csv::CsvDelimiter::Semicolon,
                header: false
            }
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(config.join("prefs.json"))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }

        // Second run: everything exists, the db is not even looked for.
        let again = import_if_needed(&config, &home.join("nowhere"));
        assert!(again.imported.is_empty());
        assert_eq!(again.source, None);
        assert_eq!(again.skipped.len(), 3);
        std::fs::remove_dir_all(&home).unwrap();
        std::fs::remove_dir_all(config.parent().unwrap()).unwrap();
    }

    #[test]
    fn skips_values_that_are_missing_or_not_json_objects() {
        let home = fake_home(
            "ls-bad",
            &[
                ("boa.prefs.v2", utf16le("{nope")),
                ("boa.session.v1", utf16le("[1,2]")),
            ],
        );
        let config = temp_dir("ls-bad-config");
        let r = import_if_needed(&config, &home);
        assert!(r.imported.is_empty());
        assert_eq!(
            r.skipped,
            [
                ("prefs.json", "not a JSON object".to_owned()),
                ("session.json", "not a JSON object".to_owned()),
                ("csv-export.json", "not in localStorage".to_owned()),
            ]
        );
        assert_eq!(std::fs::read_dir(&config).unwrap().count(), 0);
        std::fs::remove_dir_all(&home).unwrap();
        std::fs::remove_dir_all(&config).unwrap();
    }

    #[test]
    fn never_fails_without_or_with_a_broken_database() {
        let config = temp_dir("ls-none-config");
        let home = temp_dir("ls-none-home");
        let r = import_if_needed(&config, &home);
        assert!(r.imported.is_empty());
        assert!(r.skipped.iter().all(|(_, why)| why == "no localStorage"));

        let ls = home.join("Library/WebKit/dev.boa.desktop/WebsiteData/Default/s/s/LocalStorage");
        std::fs::create_dir_all(&ls).unwrap();
        std::fs::write(
            ls.join("localstorage.sqlite3"),
            "not a database at all, not even close",
        )
        .unwrap();
        let r = import_if_needed(&config, &home);
        assert!(r.imported.is_empty());
        assert_eq!(r.skipped.len(), 3);
        assert!(
            r.skipped[0].1.starts_with("cannot read localStorage"),
            "{:?}",
            r.skipped
        );
        std::fs::remove_dir_all(&home).unwrap();
        std::fs::remove_dir_all(&config).unwrap();
    }

    #[test]
    fn import_once_runs_once_per_config_dir() {
        let home = fake_home("ls-once", &[("boa.prefs.v2", utf16le(PREFS))]);
        let config = temp_dir("ls-once-config").join("dev.boa.desktop");
        let r = import_once(&config, &home).unwrap();
        assert_eq!(r.imported, ["prefs.json"]);
        let marker = std::fs::read_to_string(config.join(IMPORT_MARKER)).unwrap();
        assert!(marker.contains(r#""imported":["prefs.json"]"#), "{marker}");
        // csv-export.json and session.json are still missing: marked, not again.
        std::fs::remove_file(config.join("prefs.json")).unwrap();
        assert_eq!(import_once(&config, &home), None);
        assert!(!config.join("prefs.json").exists());
        std::fs::remove_dir_all(&home).unwrap();
        std::fs::remove_dir_all(config.parent().unwrap()).unwrap();
    }

    #[test]
    fn import_once_skips_a_config_dir_already_in_use() {
        let home = fake_home(
            "ls-used",
            &[("boa.csvExport", utf16le(r#"{"header":false}"#))],
        );
        let config = temp_dir("ls-used-config");
        std::fs::write(config.join("prefs.json"), "{}").unwrap();
        std::fs::write(config.join("session.json"), "{}").unwrap();
        assert_eq!(import_once(&config, &home), None);
        assert!(config.join(IMPORT_MARKER).exists());
        assert!(!config.join("csv-export.json").exists());
        std::fs::remove_dir_all(&home).unwrap();
        std::fs::remove_dir_all(&config).unwrap();
    }

    #[test]
    fn import_once_without_local_storage_marks_and_never_fails() {
        let config = temp_dir("ls-once-none-config");
        let home = temp_dir("ls-once-none-home");
        let r = import_once(&config, &home).unwrap();
        assert!(r.imported.is_empty());
        assert!(config.join(IMPORT_MARKER).exists());
        assert_eq!(import_once(&config, &home), None);
        std::fs::remove_dir_all(&home).unwrap();
        std::fs::remove_dir_all(&config).unwrap();
    }

    #[test]
    fn config_files_are_written_atomically() {
        let dir = temp_dir("ls-write").join("nested");
        write_config_file(&dir, "x.json", "{\"a\":1}").unwrap();
        write_config_file(&dir, "x.json", "{\"a\":2}").unwrap();
        assert_eq!(
            read_config_file(&dir, "x.json").as_deref(),
            Some("{\"a\":2}")
        );
        assert_eq!(read_config_file(&dir, "missing.json"), None);
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
        std::fs::remove_dir_all(dir.parent().unwrap()).unwrap();
    }

    /// Reads a copy of the real localStorage of this machine's Tauri install
    /// and prints its keys and value sizes (never the values).
    /// \`cargo test -p boa-core -- --ignored real_local_storage --nocapture\`
    #[test]
    #[ignore]
    fn real_local_storage_keys() {
        let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else {
            return;
        };
        let Some(db) = find_local_storage_db(&home) else {
            println!("no localStorage database under {}", home.display());
            return;
        };
        println!("database: {}", db.display());
        let items = read_items(&db).unwrap();
        let mut keys: Vec<_> = items.iter().collect();
        keys.sort();
        for (k, v) in keys {
            let json = serde_json::from_str::<serde_json::Value>(v).is_ok();
            println!(
                "{k}: {} chars, {} UTF-8 bytes, valid JSON: {json}",
                v.chars().count(),
                v.len()
            );
        }
        // The session parses with the Rust rules (counts only).
        if let Some(raw) = items.get(super::super::session::LOCAL_STORAGE_KEY) {
            let v: serde_json::Value = serde_json::from_str(raw).unwrap_or_default();
            let ids: Vec<String> = v["workspaces"]
                .as_object()
                .into_iter()
                .flatten()
                .map(|(k, _)| super::super::session::connection_of(k).to_owned())
                .chain(v["active"]["connectionId"].as_str().map(str::to_owned))
                .collect();
            let s = super::super::session::parse_session(Some(raw), ids.iter().map(String::as_str))
                .unwrap();
            let tabs: usize = s.workspaces.values().map(|w| w.tabs.len()).sum();
            println!(
                "session: {} workspaces, {tabs} tabs, {} mru, active: {}, wanted: {:?}",
                s.workspaces.len(),
                s.mru.len(),
                s.active.is_some(),
                s.wanted.as_ref().map(|w| w.len())
            );
        }
    }
}
