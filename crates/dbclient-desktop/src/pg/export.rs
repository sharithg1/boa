//! CSV export: streams a read-only statement's whole result to a file with
//! `COPY (<statement>) TO STDOUT WITH (FORMAT csv)`, so nothing but one
//! row at a time is held in memory. Postgres's CSV output is RFC 4180: NULL
//! is an empty unquoted field and an empty string is `""`.
//!
//! The statement runs on the database's shared session:
//! - outside a user transaction, in its own `begin … commit`;
//! - inside one (`Session.in_tx`), under a savepoint that is always rolled
//!   back, so an error or a cancel (`cancel_query`) leaves the user's
//!   transaction as it was, and uncommitted rows are exported as shown.
//!
//! Only what can be paged can be exported: a `declare … cursor` of the
//! statement (planned, never executed) rejects anything that writes,
//! including data-modifying `WITH`s, which `COPY (…)` would otherwise run.
//! The file is written next to the destination and renamed over it on
//! success; on failure it is removed.

use super::{Pg, Session};
use crate::error::{CmdError, CmdResult};
use futures_util::{StreamExt, pin_mut};
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CsvOptions {
    /// `,`, `\t` or `;`.
    pub delimiter: char,
    pub header: bool,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportProgress {
    pub rows: u64,
    pub bytes: u64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportResult {
    pub rows: u64,
    pub bytes: u64,
    pub ms: u64,
}

/// The `COPY` statement for `sql` (already trimmed of its `;`).
fn copy_sql(sql: &str, opts: CsvOptions) -> CmdResult<String> {
    let delimiter = match opts.delimiter {
        ',' => "','",
        ';' => "';'",
        '\t' => "E'\\t'",
        _ => return Err(CmdError::msg("Unsupported delimiter")),
    };
    // A newline before `)` so a trailing `-- comment` can't swallow it.
    Ok(format!(
        "copy (\n{sql}\n) to stdout with (format csv, header {}, delimiter {delimiter})",
        opts.header
    ))
}

/// `dir/.name.<pid>-<nanos>.part`, next to the destination so the rename stays on one volume.
fn temp_path(dest: &Path) -> CmdResult<PathBuf> {
    let name = dest
        .file_name()
        .ok_or_else(|| CmdError::msg("Not a file path"))?
        .to_string_lossy();
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.subsec_nanos());
    Ok(dest.with_file_name(format!(".{name}.{}-{nanos}.part", std::process::id())))
}

/// Writes `contents` to `dest` through a temporary file and a rename.
pub fn write_file_atomic(dest: &Path, contents: &[u8]) -> CmdResult<()> {
    let tmp = temp_path(dest)?;
    let res = std::fs::write(&tmp, contents).and_then(|()| std::fs::rename(&tmp, dest));
    if res.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    Ok(res?)
}

impl Pg {
    /// Streams the whole result of `sql` as CSV into `dest`. `progress` is
    /// called every few hundred milliseconds while rows arrive.
    pub async fn export_csv(
        &self,
        id: &str,
        database: &str,
        sql: &str,
        dest: &Path,
        opts: CsvOptions,
        mut progress: impl FnMut(ExportProgress) + Send,
    ) -> CmdResult<ExportResult> {
        let sql = sql.trim().trim_end_matches(';').trim_end();
        if sql.is_empty() {
            return Err(CmdError::msg("Empty statement"));
        }
        let copy = copy_sql(sql, opts)?;
        let s = self.session(id, database).await?;
        let started = Instant::now();
        let tmp = temp_path(dest)?;
        let file = std::fs::File::create(&tmp)?;
        let mut out = std::io::BufWriter::with_capacity(256 * 1024, file);

        let user_tx = s.in_tx.load(Ordering::Relaxed);
        let (open, ok, err) = if user_tx {
            (
                "savepoint __dbclient_export",
                "rollback to savepoint __dbclient_export; release savepoint __dbclient_export",
                "rollback to savepoint __dbclient_export; release savepoint __dbclient_export",
            )
        } else {
            ("begin", "commit", "rollback")
        };
        let streamed = async {
            s.client.batch_execute(open).await?;
            let run = async {
                probe_read_only(&s, sql).await?;
                stream_copy(&s, &copy, opts.header, &mut out, &mut progress).await
            };
            match run.await {
                Ok(p) => {
                    s.client.batch_execute(ok).await?;
                    Ok(p)
                }
                Err(e) => {
                    let _ = s.client.batch_execute(err).await;
                    Err(e)
                }
            }
        };
        let res = streamed.await.and_then(|p| {
            out.flush()?;
            out.get_ref().sync_all()?;
            drop(out);
            std::fs::rename(&tmp, dest)?;
            Ok(p)
        });
        match res {
            Ok(p) => Ok(ExportResult {
                rows: p.rows,
                bytes: p.bytes,
                ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
            }),
            Err(e) => {
                let _ = std::fs::remove_file(&tmp);
                Err(e)
            }
        }
    }
}

/// Fails (0A000, like paging) unless `sql` is a plain query: a cursor can
/// only be declared for one, and declaring it plans without running.
async fn probe_read_only(s: &Session, sql: &str) -> CmdResult<()> {
    s.client
        .batch_execute(&format!(
            "declare __dbclient_export no scroll cursor for {sql}\n;\nclose __dbclient_export"
        ))
        .await
        .map_err(|e| {
            let e = CmdError::from(e);
            if e.code.as_deref() == Some("0A000") {
                CmdError::msg("Only queries can be exported; this statement modifies data")
            } else {
                e
            }
        })
}

async fn stream_copy(
    s: &Session,
    copy: &str,
    header: bool,
    out: &mut impl Write,
    progress: &mut impl FnMut(ExportProgress),
) -> CmdResult<ExportProgress> {
    let stream = s.client.copy_out(copy).await?;
    pin_mut!(stream);
    // The server sends one CopyData message per row (the header is one too).
    let mut messages = 0u64;
    let mut bytes = 0u64;
    let mut last = Instant::now();
    let rows = |messages: u64| messages.saturating_sub(u64::from(header));
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        out.write_all(&chunk)?;
        messages += 1;
        bytes += chunk.len() as u64;
        if last.elapsed() >= Duration::from_millis(250) {
            last = Instant::now();
            progress(ExportProgress {
                rows: rows(messages),
                bytes,
            });
        }
    }
    Ok(ExportProgress {
        rows: rows(messages),
        bytes,
    })
}

#[cfg(test)]
mod tests {
    use super::{CsvOptions, copy_sql, temp_path, write_file_atomic};
    use std::path::Path;

    #[test]
    fn copy_statement_options() {
        let o = |delimiter, header| CsvOptions { delimiter, header };
        assert_eq!(
            copy_sql("select 1 -- c", o(',', true)).unwrap(),
            "copy (\nselect 1 -- c\n) to stdout with (format csv, header true, delimiter ',')"
        );
        assert!(
            copy_sql("select 1", o('\t', false))
                .unwrap()
                .ends_with("header false, delimiter E'\\t')")
        );
        assert!(copy_sql("select 1", o(';', true)).unwrap().contains("';'"));
        assert!(copy_sql("select 1", o('|', true)).is_err());
    }

    #[test]
    fn temp_file_is_a_hidden_sibling() {
        let t = temp_path(Path::new("/a/b/out.csv")).unwrap();
        assert_eq!(t.parent(), Some(Path::new("/a/b")));
        let name = t.file_name().unwrap().to_string_lossy();
        assert!(name.starts_with(".out.csv.") && name.ends_with(".part"));
        assert!(temp_path(Path::new("/")).is_err());
    }

    #[test]
    fn atomic_write_replaces_and_leaves_no_temp() {
        let dir = std::env::temp_dir().join(format!("dbclient-export-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let dest = dir.join("x.csv");
        std::fs::write(&dest, "old").unwrap();
        write_file_atomic(&dest, b"a,b\r\n").unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), b"a,b\r\n");
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
        // A missing directory fails without leaving anything behind.
        assert!(write_file_atomic(&dir.join("nope/x.csv"), b"").is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
