//! The app's state as GPUI entities (no views): what `state/store.ts` (zustand)
//! was in the Tauri app, split per HANDOFF-PARITY.md §7. Views reach the
//! app-wide ones through [`crate::app_state::AppState`] and `cx.observe` /
//! `cx.subscribe` them.
//!
//! | Module | Entity | Status |
//! |---|---|---|
//! | [`prefs`] | [`prefs::Prefs`]: prefs.json, debounced save, change events | Phase 1 |
//! | [`layout`] | [`layout::Layout`]: which panels / overlays show | Phase 1 |
//! | [`toasts`] | [`toasts::Toasts`]: keyed toasts, TTL, actions | Phase 1 |
//! | [`connections`] | [`connections::Connections`]: saved + live connections, password prompt, reconnect | Phase 2 |
//! | [`catalogs`] | [`catalogs::Catalogs`]: introspection per database, scopes, refresh | Phase 3 |
//! | [`workspaces`] | [`workspaces::Workspaces`]: tabs per database, MRU | Phase 4 |
//! | [`console`] | [`console::Console`]: one console tab | Phase 4 |
//! | [`table_tab`] | [`table_tab::TableTab`]: one table tab | Phase 5 |
//! | [`transactions`] | [`transactions::Transactions`]: tx mode / state | Phase 6 |
//! | [`history`] | [`history::History`]: query history | Phase 7 |
//! | [`exports`] | [`exports::Exports`]: CSV export options (csv-export.json), running exports, cancel | Phase 7 |
//! | [`session`] | [`session::Session`]: session.json (restore, debounced save, reconnect at launch), localStorage import | Phase 7 |
//! | [`updates`] | [`updates::Updates`]: update checks, download, restart to update | Phase 8 |

pub mod catalogs;
pub mod connections;
pub mod console;
pub mod exports;
pub mod history;
pub mod layout;
pub mod prefs;
pub mod session;
pub mod table_tab;
pub mod toasts;
pub mod transactions;
pub mod updates;
pub mod workspaces;
