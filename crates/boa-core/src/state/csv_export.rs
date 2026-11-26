//! Port of \`ui/src/state/csvExport.ts\` (pure parts; the export itself is
//! \`Backend::export_csv\` / \`pg::export\`, and the CSV writer \`grid::csv\`).
//!
//! CSV export and "Copy as CSV". Read-only (pageable) statements and tables
//! are exported whole by the backend, which streams a server-side COPY to the
//! chosen file; results of statements that change data (\`INSERT … RETURNING\`)
//! are never re-run: their loaded rows are written by \`grid::csv::to_csv\`.
//! The savepoint rules (own \`begin … commit\` outside a user transaction, a
//! savepoint that is always rolled back inside one) live with the COPY in
//! \`pg::export\`, already ported. Here: the persisted delimiter / header
//! options (\`boa.csvExport\` → \`<config dir>/csv-export.json\`, same JSON),
//! which route an [\`ExportSource\`] takes, the running-jobs list the status
//! bar and result toolbar show, and the Output-log / toast texts.

use crate::grid::csv::{CsvDelimiter, CsvOptions, csv_file_name};
use crate::grid::format::bytes;
use crate::model::Value;
use std::path::Path;

/// The options file in the config dir.
pub const FILE: &str = "csv-export.json";

/// The localStorage key the Tauri app used (imported once).
pub const LOCAL_STORAGE_KEY: &str = "boa.csvExport";

// ── Options ───────────────────────────────────────────────────────────────

/// Reads saved options (\`loadOptions\`): a delimiter other than \`,\` / tab /
/// \`;\`, a non-boolean header, or unreadable JSON fall back to the defaults
/// (comma, header) field by field.
pub fn parse_options(raw: Option<&str>) -> CsvOptions {
    let d = CsvOptions::default();
    let Some(raw) = raw.filter(|r| !r.is_empty()) else {
        return d;
    };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(raw) else {
        return d;
    };
    // JSON.parse("null").delimiter throws: defaults.
    if v.is_null() {
        return d;
    }
    let delimiter = match v.get("delimiter").and_then(serde_json::Value::as_str) {
        Some(",") => CsvDelimiter::Comma,
        Some("\t") => CsvDelimiter::Tab,
        Some(";") => CsvDelimiter::Semicolon,
        _ => d.delimiter,
    };
    let header = v
        .get("header")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(d.header);
    CsvOptions { delimiter, header }
}

/// The JSON saved for \`options\` (\`{"delimiter":",","header":true}\`, as
/// \`JSON.stringify\` wrote it).
pub fn options_json(options: CsvOptions) -> String {
    serde_json::json!({
        "delimiter": options.delimiter.as_char().to_string(),
        "header": options.header,
    })
    .to_string()
}

/// Reads \`<dir>/csv-export.json\`; missing or corrupt: the defaults.
pub fn load(dir: &Path) -> CsvOptions {
    parse_options(super::local_storage::read_config_file(dir, FILE).as_deref())
}

/// Writes \`<dir>/csv-export.json\` (temp file + rename).
pub fn save(dir: &Path, options: CsvOptions) -> std::io::Result<()> {
    super::local_storage::write_config_file(dir, FILE, &options_json(options))
}

// ── Sources ───────────────────────────────────────────────────────────────

/// What to export: a statement and, when there is one, the rows already loaded for it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ExportSource {
    pub connection_id: String,
    pub database: String,
    pub sql: String,
    /// Output log source and fallback file name (\`console_1\`).
    pub label: String,
    /// Read-only: the backend may re-run it for the whole result.
    pub pageable: bool,
    pub columns: Option<Vec<String>>,
    pub rows: Option<Vec<Vec<Value>>>,
    /// More rows exist than were loaded.
    pub truncated: bool,
}

/// How an [\`ExportSource\`] is written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportRoute {
    /// Re-run by the backend as a streamed COPY of the whole result (needs
    /// the connection open; shown as a running job).
    Stream,
    /// Changes data: never re-run; the loaded rows are written as they are.
    LoadedRows,
}

impl ExportSource {
    pub fn route(&self) -> ExportRoute {
        if self.pageable {
            ExportRoute::Stream
        } else {
            ExportRoute::LoadedRows
        }
    }

    /// The connection must be connected first (\`ensureConnected\`), else the
    /// export fails with [\`not_connected\`].
    pub fn needs_connection(&self) -> bool {
        self.pageable
    }

    /// The save panel's suggested file name.
    pub fn file_name(&self, options: CsvOptions) -> String {
        csv_file_name(&self.sql, &self.label, options.delimiter)
    }
}

/// A whole table or view (explorer menu, table tab): \`select * from schema.table\`.
pub fn table_source(
    connection_id: &str,
    database: &str,
    schema: &str,
    table: &str,
) -> ExportSource {
    ExportSource {
        connection_id: connection_id.to_owned(),
        database: database.to_owned(),
        sql: format!(
            "select * from {}",
            crate::sql::tokenize::qualified(schema, table)
        ),
        label: table.to_owned(),
        pageable: true,
        ..ExportSource::default()
    }
}

/// Running-export owner of a table: shared by its explorer menu and its tab (\`tableKey\`).
pub fn table_owner(connection_id: &str, database: &str, schema: &str, table: &str) -> String {
    super::session::table_key(connection_id, database, schema, table)
}

// ── Running exports ───────────────────────────────────────────────────────

/// A running streamed export.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportJob {
    pub id: u64,
    /// Who started it: a result's tab id or a table key, to show it as running there.
    pub owner: String,
    pub connection_id: String,
    pub database: String,
    /// The file name (no directories).
    pub file: String,
    pub rows: u64,
    pub bytes: u64,
}

/// The running exports, in start order (\`useCsvExport().jobs\`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportJobs {
    next: u64,
    jobs: Vec<ExportJob>,
}

impl Default for ExportJobs {
    fn default() -> Self {
        Self {
            next: 1,
            jobs: Vec::new(),
        }
    }
}

impl ExportJobs {
    /// Adds a job for an export of \`src\` to \`path\`; returns its id.
    pub fn start(&mut self, owner: &str, src: &ExportSource, path: &str) -> u64 {
        let id = self.next;
        self.next += 1;
        self.jobs.push(ExportJob {
            id,
            owner: owner.to_owned(),
            connection_id: src.connection_id.clone(),
            database: src.database.clone(),
            file: file_name(path).to_owned(),
            rows: 0,
            bytes: 0,
        });
        id
    }

    /// A progress event; false when the job is gone.
    pub fn progress(&mut self, id: u64, rows: u64, bytes: u64) -> bool {
        match self.jobs.iter_mut().find(|j| j.id == id) {
            Some(j) => {
                j.rows = rows;
                j.bytes = bytes;
                true
            }
            None => false,
        }
    }

    /// The export ended (done, failed or cancelled).
    pub fn finish(&mut self, id: u64) -> Option<ExportJob> {
        let i = self.jobs.iter().position(|j| j.id == id)?;
        Some(self.jobs.remove(i))
    }

    pub fn get(&self, id: u64) -> Option<&ExportJob> {
        self.jobs.iter().find(|j| j.id == id)
    }

    pub fn all(&self) -> &[ExportJob] {
        &self.jobs
    }

    /// The jobs started by \`owner\` (a result's tab id or a table key).
    pub fn of_owner<'a>(&'a self, owner: &'a str) -> impl Iterator<Item = &'a ExportJob> + 'a {
        self.jobs.iter().filter(move |j| j.owner == owner)
    }

    pub fn is_empty(&self) -> bool {
        self.jobs.is_empty()
    }
}

// ── Texts ─────────────────────────────────────────────────────────────────

/// The last part of a path (\`/\` or \`\\\` separated).
pub fn file_name(path: &str) -> &str {
    path.rsplit(['/', '\\']).next().unwrap_or(path)
}

/// \`1234\` → \`"1,234"\` (\`toLocaleString\` in en-US).
pub fn grouped(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// \`"1 row"\`, \`"1,234 rows"\`.
pub fn plural(n: u64, word: &str) -> String {
    format!("{} {word}{}", grouped(n), if n == 1 { "" } else { "s" })
}

/// Title of every export failure toast.
pub const FAILED_TITLE: &str = "Export failed";
/// Note and toast title of a cancelled export.
pub const CANCELLED: &str = "Export cancelled";
/// Title of the toast when cancelling fails.
pub const CANCEL_FAILED_TITLE: &str = "Cancel failed";
/// The success toast's action.
pub const REVEAL_LABEL: &str = "Reveal in Finder";
/// How long the success toast stays (ms).
pub const DONE_TOAST_MS: u64 = 10_000;

/// Body of the failure toast when a pageable export finds its connection closed.
pub fn not_connected(connection_name: Option<&str>) -> String {
    format!(
        "{} is not connected",
        connection_name.unwrap_or("Connection")
    )
}

/// A toast's title and body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToastText {
    pub title: String,
    pub body: String,
}

/// Output-log note after writing \`rows\` loaded rows to \`path\`.
pub fn loaded_rows_note(rows: u64, path: &str) -> String {
    format!(
        "{} exported to {path} (not re-run)",
        plural(rows, "loaded row")
    )
}

/// Success toast after writing loaded rows (\`truncated\`: more existed).
pub fn loaded_rows_toast(rows: u64, truncated: bool, path: &str) -> ToastText {
    ToastText {
        title: format!("Exported {}", plural(rows, "row")),
        body: if truncated {
            format!(
                "Only the {}: the statement changes data, so it was not re-run for the rest.\n{path}",
                plural(rows, "loaded row")
            )
        } else {
            format!("The loaded result (the statement changes data, so it was not re-run).\n{path}")
        },
    }
}

/// Output-log note after a streamed export.
pub fn streamed_note(rows: u64, size: u64, ms: u64, path: &str) -> String {
    format!(
        "{} exported to {path} ({}) in {ms} ms",
        plural(rows, "row"),
        bytes(size)
    )
}

/// Success toast after a streamed export.
pub fn streamed_toast(rows: u64, size: u64, path: &str) -> ToastText {
    ToastText {
        title: format!("Exported {}", plural(rows, "row")),
        body: format!("{path} · {}", bytes(size)),
    }
}

/// A failed export was cancelled by the user (SQLSTATE 57014): logged as
/// [\`CANCELLED\`] with an info toast, not as an error.
pub fn is_cancelled(code: Option<&str>) -> bool {
    code == Some("57014")
}

/// Output-log error of a failed export.
pub fn failed_note(error: &str) -> String {
    format!("Export failed: {error}")
}

/// Toast after "Copy as CSV" of the loaded page.
pub fn copied_title(rows: u64) -> String {
    format!("Copied {} as CSV", plural(rows, "row"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn options_default_and_validate_field_by_field() {
        let d = CsvOptions::default();
        assert_eq!(
            d,
            CsvOptions {
                delimiter: CsvDelimiter::Comma,
                header: true
            }
        );
        assert_eq!(parse_options(None), d);
        assert_eq!(parse_options(Some("{nope")), d);
        assert_eq!(parse_options(Some("null")), d);
        assert_eq!(parse_options(Some("[]")), d);
        assert_eq!(
            parse_options(Some(r#"{"delimiter":"\t","header":false}"#)),
            CsvOptions {
                delimiter: CsvDelimiter::Tab,
                header: false
            }
        );
        assert_eq!(
            parse_options(Some(r#"{"delimiter":"|","header":false}"#)),
            CsvOptions {
                delimiter: CsvDelimiter::Comma,
                header: false
            }
        );
        assert_eq!(
            parse_options(Some(r#"{"delimiter":";","header":"no"}"#)),
            CsvOptions {
                delimiter: CsvDelimiter::Semicolon,
                header: true
            }
        );
    }

    #[test]
    fn options_json_is_what_the_tauri_app_wrote() {
        assert_eq!(
            options_json(CsvOptions::default()),
            r#"{"delimiter":",","header":true}"#
        );
        let tab = CsvOptions {
            delimiter: CsvDelimiter::Tab,
            header: false,
        };
        assert_eq!(options_json(tab), r#"{"delimiter":"\t","header":false}"#);
        for o in CsvDelimiter::ALL.map(|delimiter| CsvOptions {
            delimiter,
            header: false,
        }) {
            assert_eq!(parse_options(Some(&options_json(o))), o);
        }
    }

    #[test]
    fn load_and_save_csv_export_json() {
        let dir = super::super::local_storage::tests::temp_dir("csv-export");
        assert_eq!(load(&dir), CsvOptions::default());
        let o = CsvOptions {
            delimiter: CsvDelimiter::Semicolon,
            header: false,
        };
        save(&dir, o).unwrap();
        assert_eq!(load(&dir), o);
        assert_eq!(
            std::fs::read_to_string(dir.join(FILE)).unwrap(),
            r#"{"delimiter":";","header":false}"#
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn read_only_statements_stream_and_changes_write_the_loaded_rows() {
        let t = table_source("c1", "app", "public", "Users");
        assert_eq!(t.sql, r#"select * from public."Users""#);
        assert_eq!(t.label, "Users");
        assert_eq!(t.route(), ExportRoute::Stream);
        assert!(t.needs_connection());
        assert_eq!(
            table_owner("c1", "app", "public", "Users"),
            "c1\u{0}app\u{0}public\u{0}Users"
        );
        let r = ExportSource {
            sql: "insert into t values (1) returning *".into(),
            label: "console_1".into(),
            pageable: false,
            ..ExportSource::default()
        };
        assert_eq!(r.route(), ExportRoute::LoadedRows);
        assert!(!r.needs_connection());
        assert_eq!(
            r.file_name(CsvOptions::default()),
            csv_file_name(&r.sql, "console_1", CsvDelimiter::Comma)
        );
    }

    #[test]
    fn jobs_are_listed_until_they_finish() {
        let mut jobs = ExportJobs::default();
        let src = table_source("c1", "app", "public", "users");
        let a = jobs.start("tab1", &src, "/Users/me/Desktop/users.csv");
        let b = jobs.start("tab2", &src, "C:\\out\\x.csv");
        assert_eq!((a, b), (1, 2));
        assert_eq!(jobs.get(a).unwrap().file, "users.csv");
        assert_eq!(jobs.get(b).unwrap().file, "x.csv");
        assert!(jobs.progress(a, 10, 2048));
        assert_eq!(
            (jobs.get(a).unwrap().rows, jobs.get(a).unwrap().bytes),
            (10, 2048)
        );
        assert_eq!(jobs.of_owner("tab1").count(), 1);
        assert_eq!(jobs.finish(a).unwrap().id, a);
        assert!(!jobs.progress(a, 1, 1));
        assert_eq!(jobs.all().len(), 1);
        jobs.finish(b);
        assert!(jobs.is_empty());
        assert_eq!(jobs.start("t", &src, "y.csv"), 3);
    }

    #[test]
    fn texts() {
        assert_eq!(plural(1, "row"), "1 row");
        assert_eq!(plural(0, "row"), "0 rows");
        assert_eq!(plural(1234567, "loaded row"), "1,234,567 loaded rows");
        assert_eq!(grouped(999), "999");
        assert_eq!(grouped(1000), "1,000");
        assert_eq!(file_name("/a/b/c.csv"), "c.csv");
        assert_eq!(file_name("c.csv"), "c.csv");
        assert_eq!(not_connected(Some("Prod")), "Prod is not connected");
        assert_eq!(not_connected(None), "Connection is not connected");
        assert_eq!(
            loaded_rows_note(2, "/x.csv"),
            "2 loaded rows exported to /x.csv (not re-run)"
        );
        assert_eq!(
            loaded_rows_toast(1, true, "/x.csv"),
            ToastText {
                title: "Exported 1 row".into(),
                body: "Only the 1 loaded row: the statement changes data, so it was not re-run for the rest.\n/x.csv".into()
            }
        );
        assert_eq!(
            loaded_rows_toast(3, false, "/x.csv").body,
            "The loaded result (the statement changes data, so it was not re-run).\n/x.csv"
        );
        assert_eq!(
            streamed_note(1500, 2048, 12, "/x.csv"),
            "1,500 rows exported to /x.csv (2.0 KB) in 12 ms"
        );
        assert_eq!(
            streamed_toast(1500, 2048, "/x.csv"),
            ToastText {
                title: "Exported 1,500 rows".into(),
                body: "/x.csv · 2.0 KB".into()
            }
        );
        assert!(is_cancelled(Some("57014")));
        assert!(!is_cancelled(Some("42P01")) && !is_cancelled(None));
        assert_eq!(failed_note("boom"), "Export failed: boom");
        assert_eq!(copied_title(2), "Copied 2 rows as CSV");
    }
}
