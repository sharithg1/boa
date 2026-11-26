//! Boa's GPUI app. See `crates/boa-app/README.md` for the architecture.
//!
//! - [`app`]: application setup (app actions, menu bar, main window).
//! - [`app_state`]: the app-wide models as a global ([`app_state::AppState`]),
//!   and the config dir (`BOA_CONFIG_DIR`).
//! - [`actions`] / [`keymap`]: every action and every key binding.
//! - [`models`]: GPUI entities holding the app's state (no views).
//! - [`views`]: one module per React component of the Tauri app.
//! - [`editor`] / [`grid`]: the Phase 0 SQL editor and result grid.
//! - [`theme`] / [`assets`]: colours, type scale, fonts, icons.
//! - [`platform`]: macOS wake / network-change notifications and the
//!   Dock / logout quit guard (the one module allowed `unsafe`).
//! - [`updater`]: in-app updates (fetch, verify, install, relaunch).

pub mod actions;
pub mod app;
pub mod app_state;
pub mod assets;
pub mod editor;
pub mod grid;
pub mod keymap;
pub mod models;
pub mod platform;
pub mod theme;
pub mod updater;
pub mod views;

#[cfg(test)]
mod tests;
