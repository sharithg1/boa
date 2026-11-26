//! Small chrome pieces shared by the views (chrome.tsx, IconButton.tsx,
//! StatusDot.tsx): icons, the ghost icon button with a tooltip, the status
//! dot, keycaps, and window dragging from the app's own title rows.

use std::cell::Cell;
use std::rc::Rc;

use gpui::prelude::*;
use gpui::{
    App, ClickEvent, Div, ElementId, Hsla, MouseButton, Pixels, SharedString, Stateful, Window,
    div, px,
};
use gpui_component::Icon;
use gpui_component::menu::PopupMenuItem;
use gpui_component::tooltip::Tooltip;
pub use gpui_component_assets::IconName;

use crate::theme::{self, ActivePalette as _};

/// A Lucide icon, `size` px square, in `color`.
pub fn icon(name: IconName, size: f32, color: Hsla) -> Icon {
    Icon::new(name).size(px(size)).text_color(color)
}

/// What `.tooltip(…)` takes.
pub type TooltipBuilder = Box<dyn Fn(&mut Window, &mut App) -> gpui::AnyView>;

/// A tooltip builder for `.tooltip(…)`.
pub fn tooltip(text: impl Into<SharedString>) -> TooltipBuilder {
    let text: SharedString = text.into();
    Box::new(move |window, cx| Tooltip::new(text.clone()).build(window, cx))
}

/// Every icon the views use (checked by a test: each must load from
/// [`crate::assets::Assets`]; gpui-component bundles only a default set, the
/// others are copied into `assets/icons/`). Add new ones here.
pub const USED_ICONS: &[IconName] = &[
    IconName::ArrowDown,
    IconName::ArrowLeft,
    IconName::ArrowRight,
    IconName::ArrowUp,
    IconName::Boxes,
    IconName::Braces,
    IconName::Check,
    IconName::ChevronDown,
    IconName::ChevronLeft,
    IconName::ChevronRight,
    IconName::ChevronsLeft,
    IconName::CircleAlert,
    IconName::Copy,
    IconName::ChevronsDownUp,
    IconName::CircleAlert,
    IconName::CircleCheck,
    IconName::Columns3,
    IconName::Copy,
    IconName::CornerDownLeft,
    IconName::Cylinder,
    IconName::Database,
    IconName::Download,
    IconName::Ellipsis,
    IconName::Eye,
    IconName::FileCode,
    IconName::FileDown,
    IconName::Folder,
    IconName::Info,
    IconName::KeyRound,
    IconName::Keyboard,
    IconName::Link2,
    IconName::ListOrdered,
    IconName::Lock,
    IconName::Minus,
    IconName::PanelBottom,
    IconName::PanelLeft,
    IconName::PanelRight,
    IconName::Pencil,
    IconName::Play,
    IconName::Plug,
    IconName::Plus,
    IconName::RefreshCw,
    IconName::ScrollText,
    IconName::Search,
    IconName::Server,
    IconName::Settings,
    IconName::Square,
    IconName::SquareCheck,
    IconName::SquareFunction,
    IconName::SquareTerminal,
    IconName::Table2,
    IconName::TextInitial,
    IconName::Trash,
    IconName::TriangleAlert,
    IconName::Type,
    IconName::Undo2,
    IconName::Unplug,
    IconName::X,
];

/// Lucide icons gpui-component doesn't bundle (`assets/icons/`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BoaIcon {
    /// Lucide `history` (query history).
    History,
    /// Lucide `undo-2` (revert all changes).
    Undo2,
}

impl BoaIcon {
    pub const ALL: &[BoaIcon] = &[BoaIcon::History, BoaIcon::Undo2];

    /// The SVG's asset path.
    pub fn path(self) -> &'static str {
        match self {
            BoaIcon::History => "icons/history.svg",
            BoaIcon::Undo2 => "icons/undo-2.svg",
        }
    }

    pub fn icon(self) -> Icon {
        Icon::default().path(self.path())
    }
}

/// A dropdown-menu row at TitleBar.tsx's "+" menu size: 28 px tall, a 16 px
/// icon, a 13 px label and its shortcut on the right. The stock
/// `PopupMenuItem` sizes in rems, and with the app's 13 px rem that gave
/// ~21 px rows, 11 px text and 10 px icons. Used by the title bar's "+" and
/// the explorer's "+" so the two menus look the same; `shortcut` is shown as
/// given, so keep it in step with [`crate::keymap::bindings`].
pub fn menu_row(name: IconName, label: &'static str, shortcut: &'static str) -> PopupMenuItem {
    PopupMenuItem::element(move |_, cx| {
        let p = cx.palette();
        div()
            .flex_1()
            .h(px(28.))
            .flex()
            .items_center()
            .gap(px(10.))
            .text_size(theme::text::SM)
            .line_height(theme::text::SM_LINE)
            .child(icon(name, 16., p.quiet))
            .child(div().flex_1().whitespace_nowrap().child(label))
            .child(
                div()
                    .flex_none()
                    .text_size(theme::text::XS)
                    .text_color(p.quiet)
                    .child(shortcut),
            )
    })
}

/// IconButton.tsx: a 24 px ghost button with an icon (16 px, or 14 px for
/// the explorer toolbar's `size-3.5` icons) and a tooltip. Muted, brighter on
/// hover. Its mouse-down doesn't reach a title row's window drag.
pub fn icon_button(
    id: impl Into<ElementId>,
    name: IconName,
    icon_size: f32,
    label: impl Into<SharedString>,
    cx: &App,
) -> Stateful<Div> {
    let p = cx.palette();
    let (fg, hover_fg, hover_bg) = (p.quiet, p.body, p.hover_strong);
    div()
        .id(id)
        .flex_none()
        .size(px(24.))
        .rounded(theme::radius::MD)
        .flex()
        .items_center()
        .justify_center()
        .cursor_pointer()
        .text_color(fg)
        .hover(move |s| s.bg(hover_bg).text_color(hover_fg))
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .tooltip(tooltip(label))
        .child(Icon::new(name).size(px(icon_size)))
}

/// Status dot kinds (StatusDot.tsx `DotKind`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DotKind {
    Run,
    Wait,
    Err,
    Ok,
    Idle,
    Disc,
}

/// StatusDot.tsx: a filled 7 px (or 6 px) dot, or a ring for idle /
/// disconnected (dashed in the TS; GPUI has no dashed borders, so solid).
pub fn status_dot(kind: DotKind, size: f32, cx: &App) -> Div {
    let p = cx.palette();
    let d = div().flex_none().size(px(size)).rounded_full();
    match kind {
        DotKind::Run => d.bg(p.status_run),
        DotKind::Wait => d.bg(p.status_wait),
        DotKind::Err => d.bg(p.destructive),
        DotKind::Ok => d.bg(p.status_ok),
        DotKind::Idle | DotKind::Disc => d.border(px(1.5)).border_color(p.dim),
    }
}

/// chrome.tsx `Kbd`: a keycap hint.
pub fn kbd(text: impl Into<SharedString>, cx: &App) -> Div {
    let p = cx.palette();
    div()
        .flex_none()
        .rounded(theme::radius::SM)
        .border_1()
        .border_color(p.border)
        .bg(p.muted)
        .px(px(5.))
        .font_family(theme::MONO_FONT)
        .text_size(theme::text::XXS)
        .line_height(px(17.))
        .text_color(p.quiet)
        .child(text.into())
}

/// A thin vertical separator (`mx-1 h-4 w-px bg-border`).
pub fn vsep(cx: &App) -> Div {
    div()
        .flex_none()
        .mx_1()
        .h(px(16.))
        .w(px(1.))
        .bg(cx.palette().border)
}

/// Window dragging from a row the app draws under the transparent title bar
/// (Tauri's `data-tauri-drag-region="deep"`): a press on the row's empty parts
/// then a move drags the window; a double click zooms (or minimizes, per the
/// system setting). Buttons on the row stop the press from reaching it.
///
/// The main window sets `app_owns_titlebar_drag`, so AppKit leaves the title
/// area to us; this is how it moves.
#[derive(Debug, Clone, Default)]
pub struct WindowDrag(Rc<Cell<bool>>);

impl WindowDrag {
    pub fn new() -> Self {
        Self::default()
    }

    /// Makes `el` a drag region.
    pub fn apply(&self, el: Stateful<Div>) -> Stateful<Div> {
        let down = self.0.clone();
        let up = self.0.clone();
        let moved = self.0.clone();
        el.on_mouse_down(MouseButton::Left, move |event, window, _| {
            if event.click_count >= 2 {
                down.set(false);
                window.titlebar_double_click();
            } else {
                down.set(true);
            }
        })
        .on_mouse_up(MouseButton::Left, move |_, _, _| up.set(false))
        .on_mouse_move(move |event, window, _| {
            if event.pressed_button != Some(MouseButton::Left) {
                moved.set(false);
            } else if moved.replace(false) {
                window.start_window_move();
            }
        })
    }
}

/// Stops a click from also reaching the row behind (tabs' close buttons).
pub fn stop(_: &ClickEvent, _: &mut Window, cx: &mut App) {
    cx.stop_propagation();
}

/// Width reserved for the traffic lights (window-chrome.ts TRAFFIC_LIGHTS_INSET:
/// x 14 + ~60 for the buttons + a 12 gap); 0 in fullscreen, where macOS hides them.
pub const TRAFFIC_LIGHTS_INSET: Pixels = px(86.);

/// The title-row inset for this window: [`TRAFFIC_LIGHTS_INSET`], 0 in fullscreen.
pub fn traffic_lights_inset(window: &Window) -> Pixels {
    if window.is_fullscreen() {
        px(0.)
    } else {
        TRAFFIC_LIGHTS_INSET
    }
}
