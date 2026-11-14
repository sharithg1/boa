//! Query history: every statement run from the client, one JSON object per
//! line in `<app config dir>/history-<connection id>.jsonl` (owner-only on
//! Unix, like `connections.json`). Appends are cheap; once a file holds
//! `CAP + SLACK` entries it is rewritten with the newest `CAP`. Lines that do
//! not parse (a crash mid-write) are skipped.

use crate::error::CmdResult;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// Entries kept per connection (the oldest are dropped).
pub const CAP: usize = 5000;
/// How far past `CAP` a file may grow before it is compacted.
const SLACK: usize = 500;

/// One executed statement, as the UI records it (`ui/src/lib/history.ts`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Entry {
    pub id: String,
    pub sql: String,
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

/// Serialises writes and remembers each file's line count, so an append
/// does not have to read the file.
#[derive(Default)]
pub struct History {
    counts: Mutex<HashMap<String, usize>>,
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
    fn counts(&self) -> std::sync::MutexGuard<'_, HashMap<String, usize>> {
        self.counts
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// The connection's entries, oldest first.
    pub fn list(&self, dir: &Path, id: &str) -> CmdResult<Vec<Entry>> {
        let mut counts = self.counts();
        let entries = read(&file(dir, id))?;
        counts.insert(id.to_owned(), entries.len());
        Ok(entries)
    }

    pub fn append(&self, dir: &Path, id: &str, entry: &Entry) -> CmdResult<()> {
        let mut counts = self.counts();
        std::fs::create_dir_all(dir)?;
        let path = file(dir, id);
        let count = match counts.get(id) {
            Some(n) => *n,
            None => read(&path)?.len(),
        };
        let mut line = serde_json::to_vec(entry)?;
        line.push(b'\n');
        open_options()
            .append(true)
            .create(true)
            .open(&path)?
            .write_all(&line)?;
        let mut count = count + 1;
        if count > CAP + SLACK {
            let entries = read(&path)?;
            let keep = &entries[entries.len().saturating_sub(CAP)..];
            write_all(&path, keep)?;
            count = keep.len();
        }
        counts.insert(id.to_owned(), count);
        Ok(())
    }

    /// Removes the entries with these ids.
    pub fn delete(&self, dir: &Path, id: &str, ids: &[String]) -> CmdResult<()> {
        let mut counts = self.counts();
        let path = file(dir, id);
        let drop: HashSet<&str> = ids.iter().map(String::as_str).collect();
        let mut entries = read(&path)?;
        let before = entries.len();
        entries.retain(|e| !drop.contains(e.id.as_str()));
        if entries.len() != before {
            write_all(&path, &entries)?;
        }
        counts.insert(id.to_owned(), entries.len());
        Ok(())
    }

    /// Deletes the connection's history file.
    pub fn clear(&self, dir: &Path, id: &str) -> CmdResult<()> {
        let mut counts = self.counts();
        counts.remove(id);
        match std::fs::remove_file(file(dir, id)) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.into()),
            _ => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{CAP, Entry, History, SLACK, file};
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
        assert!(h.list(&dir, "c1").unwrap().is_empty());
        for n in 0..3 {
            h.append(&dir, "c1", &entry(n)).unwrap();
        }
        let mut failed = entry(9);
        failed.error = Some("[42P01] relation \"x\" does not exist".into());
        failed.rows = None;
        h.append(&dir, "c1", &failed).unwrap();
        h.append(&dir, "c2", &entry(5)).unwrap();

        let list = h.list(&dir, "c1").unwrap();
        assert_eq!(list.len(), 4);
        assert_eq!(list[0], entry(0));
        assert_eq!(list[3], failed);

        h.delete(&dir, "c1", &["e1".into()]).unwrap();
        let ids: Vec<_> = h
            .list(&dir, "c1")
            .unwrap()
            .into_iter()
            .map(|e| e.id)
            .collect();
        assert_eq!(ids, ["e0", "e2", "e9"]);

        h.clear(&dir, "c1").unwrap();
        h.clear(&dir, "c1").unwrap(); // already gone: fine
        assert!(h.list(&dir, "c1").unwrap().is_empty());
        assert_eq!(h.list(&dir, "c2").unwrap().len(), 1);

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
        h.append(&dir, "c", &entry(1)).unwrap();
        let path = file(&dir, "c");
        let mut text = std::fs::read_to_string(&path).unwrap();
        text.push_str("{\"id\":\"half\",\"sq\n");
        std::fs::write(&path, text).unwrap();
        // A fresh instance, as after a restart.
        let h = History::default();
        h.append(&dir, "c", &entry(2)).unwrap();
        let ids: Vec<_> = h
            .list(&dir, "c")
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
        let total = CAP + SLACK + 1;
        for n in 0..total {
            h.append(&dir, "c", &entry(n)).unwrap();
        }
        let list = h.list(&dir, "c").unwrap();
        assert_eq!(list.len(), CAP);
        assert_eq!(list[0].id, format!("e{}", total - CAP));
        assert_eq!(list[CAP - 1].id, format!("e{}", total - 1));
        std::fs::remove_dir_all(dir).unwrap();
    }
}
