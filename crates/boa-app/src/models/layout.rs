//! [`Layout`]: which panels and overlays of the main window show: the
//! Database Explorer (⌥B), the right panel (⌥E), the Database Output panel
//! (⌥J) and the Settings view (⌘,). store.ts kept these in its state
//! (`sidebarOpen`, `rightPanelOpen`, `bottomOpen`, `settings`).
//!
//! The three panel flags are part of the session in the Tauri app
//! (`boa.session.v1`: `sidebarOpen` / `rightPanelOpen` / `bottomOpen`), so
//! they are persisted in `<config dir>/session.json`, not in prefs.json
//! (whose fields stay exactly `boa.prefs.v2`'s). This entity doesn't write
//! anything: [`super::session::Session`], the one writer of session.json,
//! restores the flags at launch ([`Layout::restore_panels`]) and saves them
//! when this entity notifies. Panel sizes are prefs (`sidebarWidth` …) and
//! live in [`super::prefs::Prefs`].

use std::path::Path;

use gpui::{Context, EventEmitter};
use serde_json::Value;

use super::session::SESSION_FILE;

/// The right panel starts open in windows at least this wide (store.ts:
/// `window.innerWidth >= 1100`), when the session doesn't say.
pub const RIGHT_PANEL_MIN_WINDOW_WIDTH: f32 = 1100.;

/// Settings pages (store.ts `SettingsPage`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SettingsPage {
    General,
    Connections,
    Keyboard,
}

impl SettingsPage {
    pub const ALL: [SettingsPage; 3] = [
        SettingsPage::General,
        SettingsPage::Connections,
        SettingsPage::Keyboard,
    ];

    pub fn label(self) -> &'static str {
        match self {
            SettingsPage::General => "General",
            SettingsPage::Connections => "Connections",
            SettingsPage::Keyboard => "Keyboard",
        }
    }
}

/// A panel that ⌥B / ⌥E / ⌥J toggles.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PanelKind {
    Sidebar,
    RightPanel,
    Bottom,
}

/// Emitted when anything in the layout changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LayoutEvent {
    /// A panel was shown or hidden.
    Panel(PanelKind, bool),
    /// Settings opened (Some) or closed (None).
    Settings(Option<SettingsPage>),
}

/// The panel flags saved in session.json.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SavedPanels {
    pub sidebar_open: Option<bool>,
    pub right_panel_open: Option<bool>,
    pub bottom_open: Option<bool>,
}

/// Which panels and overlays show.
#[derive(Debug)]
pub struct Layout {
    sidebar_open: bool,
    right_panel_open: bool,
    bottom_open: bool,
    settings: Option<SettingsPage>,
}

impl EventEmitter<LayoutEvent> for Layout {}

impl Layout {
    /// The store.ts defaults: sidebar and Database Output open, the right
    /// panel open in windows at least 1100 px wide.
    pub fn new(window_width: f32) -> Self {
        Self {
            sidebar_open: true,
            right_panel_open: window_width >= RIGHT_PANEL_MIN_WINDOW_WIDTH,
            bottom_open: true,
            settings: None,
        }
    }

    /// The flags saved in the session, over the defaults (session.ts
    /// `startSession`: `sidebarOpen ?? s.sidebarOpen` …). No events.
    pub fn restore_panels(&mut self, saved: SavedPanels, cx: &mut Context<Self>) {
        self.sidebar_open = saved.sidebar_open.unwrap_or(self.sidebar_open);
        self.right_panel_open = saved.right_panel_open.unwrap_or(self.right_panel_open);
        self.bottom_open = saved.bottom_open.unwrap_or(self.bottom_open);
        cx.notify();
    }

    /// The flags as the session saves them.
    pub fn panels(&self) -> SavedPanels {
        SavedPanels {
            sidebar_open: Some(self.sidebar_open),
            right_panel_open: Some(self.right_panel_open),
            bottom_open: Some(self.bottom_open),
        }
    }

    pub fn sidebar_open(&self) -> bool {
        self.sidebar_open
    }

    pub fn right_panel_open(&self) -> bool {
        self.right_panel_open
    }

    pub fn bottom_open(&self) -> bool {
        self.bottom_open
    }

    /// Whether `panel` shows.
    pub fn is_open(&self, panel: PanelKind) -> bool {
        match panel {
            PanelKind::Sidebar => self.sidebar_open,
            PanelKind::RightPanel => self.right_panel_open,
            PanelKind::Bottom => self.bottom_open,
        }
    }

    /// Shows or hides `panel`.
    pub fn set_open(&mut self, panel: PanelKind, open: bool, cx: &mut Context<Self>) {
        if self.is_open(panel) == open {
            return;
        }
        match panel {
            PanelKind::Sidebar => self.sidebar_open = open,
            PanelKind::RightPanel => self.right_panel_open = open,
            PanelKind::Bottom => self.bottom_open = open,
        }
        cx.emit(LayoutEvent::Panel(panel, open));
        cx.notify();
    }

    /// ⌥B / ⌥E / ⌥J.
    pub fn toggle(&mut self, panel: PanelKind, cx: &mut Context<Self>) {
        let open = !self.is_open(panel);
        self.set_open(panel, open, cx);
    }

    /// The Settings page showing, if Settings is open.
    pub fn settings(&self) -> Option<SettingsPage> {
        self.settings
    }

    /// Opens Settings on `page`, or closes it (None).
    pub fn set_settings(&mut self, page: Option<SettingsPage>, cx: &mut Context<Self>) {
        if self.settings == page {
            return;
        }
        self.settings = page;
        cx.emit(LayoutEvent::Settings(page));
        cx.notify();
    }

    /// ⌘,: open on General, or close.
    pub fn toggle_settings(&mut self, cx: &mut Context<Self>) {
        let page = match self.settings {
            Some(_) => None,
            None => Some(SettingsPage::General),
        };
        self.set_settings(page, cx);
    }
}

/// The panel flags of `<dir>/session.json` (all None when the file is
/// missing or unreadable, or a flag is not a boolean: session.ts `flag()`).
pub fn load_panels(dir: &Path) -> SavedPanels {
    let Some(Value::Object(map)) = std::fs::read_to_string(dir.join(SESSION_FILE))
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok())
    else {
        return SavedPanels::default();
    };
    let flag = |key: &str| map.get(key).and_then(Value::as_bool);
    SavedPanels {
        sidebar_open: flag("sidebarOpen"),
        right_panel_open: flag("rightPanelOpen"),
        bottom_open: flag("bottomOpen"),
    }
}
