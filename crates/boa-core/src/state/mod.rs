//! UI state logic ported from the Tauri app's `ui/src/state/*.ts` and
//! `ui/src/lib/{history,updates}.ts`: pure functions and data types (plus small
//! JSON-file helpers for the files that replace WebView localStorage). No GPUI.
//! The GPUI entities in boa-app are built on these.

/// Port of postgresql:// URL paste parsing from components/ConnectionDialog.tsx (+ its .test.ts).
pub mod conn_url;
/// Port of state/csvExport.ts.
pub mod csv_export;
/// Port of state/ddl.ts.
pub mod ddl;
/// Port of state/guards.ts.
pub mod guards;
/// Port of lib/history.ts + state/history.ts.
pub mod history;
/// Port of one-time import of the WebKit localStorage (boa.prefs.v2, boa.session.v1, boa.csvExport).
pub mod local_storage;
/// Port of state/prefs.ts.
pub mod prefs;
/// Port of pure reconnect rules of state/store.ts + state/links.ts (backoff schedule, which errors mean lost, retry-safe decisions) + the pure cases of state/reconnect.test.ts.
pub mod reconnect;
/// Port of state/session.ts.
pub mod session;
/// Port of state/tx.ts.
pub mod tx;
/// Port of lib/updates.ts.
pub mod updates;
