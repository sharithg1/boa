//! Query history: the queries run from the client, one JSON object per line
//! in `<app config dir>/history-<connection id>.jsonl` (owner-only on Unix,
//! like `connections.json`). An append replaces the earlier run of the same
//! query (same `key`: the UI's `queryKey`, the SQL without its values) and
//! drops runs past the retention period (a setting: 30 days by default) and
//! the oldest past `CAP`, rewriting the file. Lines that do not parse (a
//! crash mid-write) are skipped.

use crate::error::CmdResult;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// Distinct queries kept per connection (the oldest are dropped).
pub const CAP: usize = 5000;

/// One executed statement, as the UI records it (`ui/src/lib/history.ts`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Entry {
    pub id: String,
    pub sql: String,
    /// What makes two runs the same query (`queryKey` in the UI). Entries
    /// saved before keys were have none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    pub database: String,
    /// When it ran (ms since the epoch).
    pub at: f64,
    /// Duration in ms.
    pub ms: f64,
    /// Rows fetched, for row-returning statements.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rows: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub truncated: Option<bool>,
    /// Affected rows, for statements that return none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub affected: Option<f64>,
    /// Editor tab it ran from (`console_1`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Server detail / hint lines of an error.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

impl Entry {
    /// The dedupe key: the UI's, or the SQL itself for an entry without one.
    fn key(&self) -> &str {
        self.key.as_deref().unwrap_or_else(|| self.sql.trim())
    }
}

/// Serialises reads and writes of the history files.
#[derive(Default)]
pub struct History {
    lock: Mutex<()>,
}

/// File name for a connection id: safe characters kept, others hex-escaped,
/// so an id can never point outside the config dir.
fn file(dir: &Path, id: &str) -> PathBuf {
    let mut name = String::from("history-");
    for b in id.bytes() {
        if b.is_ascii_alphanumeric() || b == b'-' || b == b'_' {
            name.push(char::from(b));
        } else {
            name.push_str(&format!("%{b:02x}"));
        }
    }
    name.push_str(".jsonl");
    dir.join(name)
}

fn open_options() -> OpenOptions {
    let mut o = OpenOptions::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        o.mode(0o600);
    }
    o
}

/// Every readable entry, oldest first.
fn read(path: &Path) -> CmdResult<Vec<Entry>> {
    let f = match File::open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e.into()),
    };
    let mut out = Vec::new();
    for line in BufReader::new(f).lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        if let Ok(e) = serde_json::from_str::<Entry>(&line) {
            out.push(e);
        }
    }
    Ok(out)
}

/// Replaces the file with `entries` (write then rename).
fn write_all(path: &Path, entries: &[Entry]) -> CmdResult<()> {
    let tmp = path.with_extension("jsonl.tmp");
    {
        let mut f = open_options()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&tmp)?;
        let mut buf = Vec::new();
        for e in entries {
            serde_json::to_writer(&mut buf, e)?;
            buf.push(b'\n');
        }
        f.write_all(&buf)?;
    }
    std::fs::rename(tmp, path)?;
    Ok(())
}

impl History {
    fn lock(&self) -> std::sync::MutexGuard<'_, ()> {
        self.lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// The connection's entries run at or after `cutoff` (ms since the
    /// epoch; `None`: all), oldest first. Older ones stay in the file until
    /// the next append.
    pub fn list(&self, dir: &Path, id: &str, cutoff: Option<f64>) -> CmdResult<Vec<Entry>> {
        let _guard = self.lock();
        let mut entries = read(&file(dir, id))?;
        if let Some(cutoff) = cutoff {
            entries.retain(|e| e.at >= cutoff);
        }
        Ok(entries)
    }

    /// Adds `entry` as the newest, in place of any earlier run of the same
    /// query; drops runs before `cutoff` and keeps the newest `CAP`.
    pub fn append(
        &self,
        dir: &Path,
        id: &str,
        entry: &Entry,
        cutoff: Option<f64>,
    ) -> CmdResult<()> {
        let _guard = self.lock();
        std::fs::create_dir_all(dir)?;
        let path = file(dir, id);
        let mut entries = read(&path)?;
        entries.retain(|e| e.key() != entry.key() && cutoff.is_none_or(|c| e.at >= c));
        entries.push(entry.clone());
        write_all(&path, &entries[entries.len().saturating_sub(CAP)..])
    }

    /// Removes the entries with these ids.
    pub fn delete(&self, dir: &Path, id: &str, ids: &[String]) -> CmdResult<()> {
        let _guard = self.lock();
        let path = file(dir, id);
        let drop: HashSet<&str> = ids.iter().map(String::as_str).collect();
        let mut entries = read(&path)?;
        let before = entries.len();
        entries.retain(|e| !drop.contains(e.id.as_str()));
        if entries.len() != before {
            write_all(&path, &entries)?;
        }
        Ok(())
    }

    /// Deletes the connection's history file.
    pub fn clear(&self, dir: &Path, id: &str) -> CmdResult<()> {
        let _guard = self.lock();
        match std::fs::remove_file(file(dir, id)) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.into()),
            _ => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{CAP, Entry, History, file};
    use std::path::PathBuf;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "boa-history-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn entry(n: usize) -> Entry {
        Entry {
            id: format!("e{n}"),
            sql: format!("select {n}"),
            key: Some(format!("select ? from t{n}")),
            database: "postgres".into(),
            at: 1_700_000_000_000.0 + n as f64,
            ms: 3.0,
            rows: Some(1.0),
            truncated: None,
            affected: None,
            source: Some("console_1".into()),
            error: None,
            detail: None,
        }
    }

    #[test]
    fn file_names_stay_in_dir() {
        let dir = PathBuf::from("/cfg");
        assert_eq!(file(&dir, "abc-1_x"), dir.join("history-abc-1_x.jsonl"));
        assert_eq!(
            file(&dir, "../etc/x"),
            dir.join("history-%2e%2e%2fetc%2fx.jsonl")
        );
    }

    #[test]
    fn append_list_delete_clear() {
        let dir = temp_dir("basic");
        let h = History::default();
        assert!(h.list(&dir, "c1", None).unwrap().is_empty());
        for n in 0..3 {
            h.append(&dir, "c1", &entry(n), None).unwrap();
        }
        let mut failed = entry(9);
        failed.error = Some("[42P01] relation \"x\" does not exist".into());
        failed.rows = None;
        h.append(&dir, "c1", &failed, None).unwrap();
        h.append(&dir, "c2", &entry(5), None).unwrap();

        let list = h.list(&dir, "c1", None).unwrap();
        assert_eq!(list.len(), 4);
        assert_eq!(list[0], entry(0));
        assert_eq!(list[3], failed);

        h.delete(&dir, "c1", &["e1".into()]).unwrap();
        let ids: Vec<_> = h
            .list(&dir, "c1", None)
            .unwrap()
            .into_iter()
            .map(|e| e.id)
            .collect();
        assert_eq!(ids, ["e0", "e2", "e9"]);

        h.clear(&dir, "c1").unwrap();
        h.clear(&dir, "c1").unwrap(); // already gone: fine
        assert!(h.list(&dir, "c1", None).unwrap().is_empty());
        assert_eq!(h.list(&dir, "c2", None).unwrap().len(), 1);

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(file(&dir, "c2"))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn skips_broken_lines() {
        let dir = temp_dir("broken");
        let h = History::default();
        h.append(&dir, "c", &entry(1), None).unwrap();
        let path = file(&dir, "c");
        let mut text = std::fs::read_to_string(&path).unwrap();
        text.push_str("{\"id\":\"half\",\"sq\n");
        std::fs::write(&path, text).unwrap();
        // A fresh instance, as after a restart.
        let h = History::default();
        h.append(&dir, "c", &entry(2), None).unwrap();
        let ids: Vec<_> = h
            .list(&dir, "c", None)
            .unwrap()
            .into_iter()
            .map(|e| e.id)
            .collect();
        assert_eq!(ids, ["e1", "e2"]);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn caps_to_newest() {
        let dir = temp_dir("cap");
        let h = History::default();
        let total = CAP + 3;
        // Written directly: an append rewrites the file.
        let lines: String = (0..total - 1)
            .map(|n| serde_json::to_string(&entry(n)).unwrap() + "\n")
            .collect();
        std::fs::write(file(&dir, "c"), lines).unwrap();
        h.append(&dir, "c", &entry(total - 1), None).unwrap();
        let list = h.list(&dir, "c", None).unwrap();
        assert_eq!(list.len(), CAP);
        assert_eq!(list[0].id, format!("e{}", total - CAP));
        assert_eq!(list[CAP - 1].id, format!("e{}", total - 1));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn rerun_replaces_the_earlier_entry() {
        let dir = temp_dir("dedupe");
        let h = History::default();
        for n in 0..3 {
            h.append(&dir, "c", &entry(n), None).unwrap();
        }
        // The same query with another value: same key, new SQL.
        let mut again = entry(0);
        again.id = "e0b".into();
        again.sql = "select 42".into();
        h.append(&dir, "c", &again, None).unwrap();
        let list = h.list(&dir, "c", None).unwrap();
        let ids: Vec<_> = list.iter().map(|e| e.id.as_str()).collect();
        assert_eq!(ids, ["e1", "e2", "e0b"]);
        assert_eq!(list[2].sql, "select 42");

        // Entries without a key are the same query when their SQL is.
        let mut old = entry(7);
        old.key = None;
        h.append(&dir, "c", &old, None).unwrap();
        let mut old_again = old.clone();
        old_again.id = "e7b".into();
        h.append(&dir, "c", &old_again, None).unwrap();
        let ids: Vec<_> = h
            .list(&dir, "c", None)
            .unwrap()
            .into_iter()
            .map(|e| e.id)
            .collect();
        assert_eq!(ids, ["e1", "e2", "e0b", "e7b"]);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn drops_runs_past_the_retention_period() {
        let dir = temp_dir("age");
        let h = History::default();
        for n in 0..4 {
            h.append(&dir, "c", &entry(n), None).unwrap();
        }
        let at = |n: usize| Some(entry(n).at);
        let ids = |cutoff| -> Vec<String> {
            h.list(&dir, "c", cutoff)
                .unwrap()
                .into_iter()
                .map(|e| e.id)
                .collect()
        };
        // Listing hides older runs without removing them.
        assert_eq!(ids(at(2)), ["e2", "e3"]);
        assert_eq!(ids(None), ["e0", "e1", "e2", "e3"]);
        // An append removes them.
        h.append(&dir, "c", &entry(4), at(2)).unwrap();
        assert_eq!(ids(None), ["e2", "e3", "e4"]);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
