//! PanelResizeHandle.tsx: the drag handle on a side panel's inner edge (the
//! sidebar's right edge, the right panel's left edge, the Database Output
//! panel's top edge). The panel follows the pointer while dragging, clamped
//! (sidebar 200–480, right 280–800, bottom 120–min(900, window − 200)), and
//! the size is saved to the prefs once, on release; a double click restores
//! the default.
//!
//! The root view owns the one [`PanelDrag`] in progress and, while there is
//! one, a window-level `MouseMove` / `MouseUp` listener (a `canvas`), so the
//! drag keeps going when the pointer leaves the 6 px handle.

use boa_core::state::prefs::{self as core, Prefs as CorePrefs};
use gpui::prelude::*;
use gpui::{App, Div, Hsla, MouseButton, MouseDownEvent, Pixels, Point, Stateful, Window, div, px};

use crate::models::layout::PanelKind;

/// Thickness of the handle (`w-1.5` / `h-1.5`), centred on the panel edge.
pub const HANDLE: Pixels = px(6.);

impl PanelKind {
    /// Whether dragging along x (side panels) or y (the bottom panel).
    pub fn vertical_edge(self) -> bool {
        !matches!(self, PanelKind::Bottom)
    }

    /// Clamps a size in px for a window `window_height` tall.
    pub fn clamp(self, size: f64, window_height: f64) -> u32 {
        match self {
            PanelKind::Sidebar => core::clamp_sidebar_width(size),
            PanelKind::RightPanel => core::clamp_right_panel_width(size),
            PanelKind::Bottom => core::clamp_bottom_height(size, Some(window_height)),
        }
    }

    /// The saved size, clamped (what the panel shows when not dragging).
    pub fn saved_size(self, prefs: &CorePrefs, window_height: f64) -> u32 {
        match self {
            PanelKind::Sidebar => prefs.sidebar_width_px(),
            PanelKind::RightPanel => prefs.right_panel_width_px(),
            PanelKind::Bottom => prefs.bottom_height_px(Some(window_height)),
        }
    }

    /// The default size (double click on the handle).
    pub fn default_size(self) -> u32 {
        let d = CorePrefs::default();
        match self {
            PanelKind::Sidebar => d.sidebar_width,
            PanelKind::RightPanel => d.right_panel_width,
            PanelKind::Bottom => d.bottom_height,
        }
    }

    /// Writes `size` to the pref that holds it.
    pub fn store(self, prefs: &mut CorePrefs, size: u32) {
        match self {
            PanelKind::Sidebar => prefs.sidebar_width = size,
            PanelKind::RightPanel => prefs.right_panel_width = size,
            PanelKind::Bottom => prefs.bottom_height = size,
        }
    }

    /// The handle's accessible label (and tooltip text in the TS).
    pub fn handle_label(self) -> &'static str {
        match self {
            PanelKind::Sidebar => "Resize sidebar",
            PanelKind::RightPanel => "Resize right panel",
            PanelKind::Bottom => "Resize output panel",
        }
    }
}

/// A resize in progress.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PanelDrag {
    pub panel: PanelKind,
    /// Pointer coordinate (x or y) at the press.
    start: f32,
    /// Panel size at the press.
    start_size: f32,
    /// The size now (clamped).
    pub size: u32,
}

impl PanelDrag {
    /// Starts dragging `panel` (now `size` px) from `position`.
    pub fn begin(panel: PanelKind, position: Point<Pixels>, size: u32) -> Self {
        let start = if panel.vertical_edge() {
            position.x
        } else {
            position.y
        };
        Self {
            panel,
            start: f32::from(start),
            start_size: size as f32,
            size,
        }
    }

    /// The pointer moved to `position`: the new clamped size. Dragging away
    /// from the panel grows it.
    pub fn update(&mut self, position: Point<Pixels>, window_height: f32) -> u32 {
        let (pos, grows_with) = match self.panel {
            PanelKind::Sidebar => (position.x, 1.),
            PanelKind::RightPanel => (position.x, -1.),
            PanelKind::Bottom => (position.y, -1.),
        };
        let delta = (f32::from(pos) - self.start) * grows_with;
        self.size = self
            .panel
            .clamp(f64::from(self.start_size + delta), f64::from(window_height));
        self.size
    }
}

/// The handle for `panel`: a 6 px strip (absolutely positioned by the caller)
/// that highlights on hover and while `active`. `on_down` gets the press (a
/// double click resets, see the root view).
pub fn handle(
    panel: PanelKind,
    active: bool,
    highlight: Hsla,
    on_down: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
) -> Stateful<Div> {
    let id = match panel {
        PanelKind::Sidebar => "resize-sidebar",
        PanelKind::RightPanel => "resize-right-panel",
        PanelKind::Bottom => "resize-bottom-panel",
    };
    let el = div()
        .id(id)
        .absolute()
        .hover(move |s| s.bg(highlight))
        .when(active, |d| d.bg(highlight))
        .on_mouse_down(MouseButton::Left, move |event, window, cx| {
            cx.stop_propagation();
            window.prevent_default();
            on_down(event, window, cx);
        });
    if panel.vertical_edge() {
        el.w(HANDLE).cursor_col_resize()
    } else {
        el.h(HANDLE).cursor_row_resize()
    }
}
