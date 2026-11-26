//! CompletionPopup.tsx: the completion list under the word being completed.
//! The editor keeps the focus throughout (it routes ↑ ↓ PgUp PgDn Enter Tab
//! Esc here, see `crate::editor`); a click picks an item without blurring
//! it. Rows have the Tauri app's per-kind icons (a yellow key for primary-key
//! columns; Lucide `letter-text` is `text-initial` in the bundled set), the label and a muted detail.

use std::rc::Rc;

use boa_core::sql::{Completion, CompletionKind};
use gpui::prelude::*;
use gpui::{
    AnyElement, App, Bounds, Hsla, MouseButton, Pixels, Point, ScrollHandle, Window, anchored,
    deferred, div, point, px,
};

use super::chrome::IconName;
use crate::theme::{self, ActivePalette as _, Palette};

/// Row height; the list shows at most [`VISIBLE`] rows before scrolling.
pub const ROW: f32 = 22.;
pub const VISIBLE: usize = crate::editor::VISIBLE_ROWS;
/// Icon column, so labels line up with the word being completed.
pub const GUTTER: f32 = 26.;
pub const WIDTH: f32 = 380.;

/// A click / hover on row `i`.
pub type RowHandler = Rc<dyn Fn(usize, &mut Window, &mut App)>;

/// The icon and tint of an item (CompletionPopup.tsx `ICON`).
pub fn kind_icon(item: &Completion, p: &Palette) -> (IconName, Hsla) {
    if item.pk {
        return (IconName::KeyRound, p.sql_number);
    }
    match item.kind {
        CompletionKind::Keyword => (IconName::TextInitial, p.sql_keyword),
        CompletionKind::Schema => (IconName::Database, p.sql_table),
        CompletionKind::Table => (IconName::Table2, p.sql_table),
        CompletionKind::View => (IconName::Eye, p.sql_table),
        CompletionKind::Cte => (IconName::Braces, p.sql_table),
        CompletionKind::Column => (IconName::Columns3, p.sql_column),
        CompletionKind::Alias => (IconName::Table2, p.quiet),
        CompletionKind::Function => (IconName::SquareFunction, p.sql_function),
        CompletionKind::Type => (IconName::Type, p.sql_type),
        CompletionKind::Join => (IconName::Link2, p.link),
    }
}

/// Where a popup of `width` × `height` goes (window coordinates): under the
/// anchor's line, or above it when there is no room below but room above;
/// `dx` px left of the anchor, kept 4 px inside `area` horizontally.
pub fn place(
    anchor: Bounds<Pixels>,
    area: Bounds<Pixels>,
    width: f32,
    height: f32,
    dx: f32,
    gap: f32,
) -> Point<Pixels> {
    let (top, bottom) = (f32::from(anchor.top()), f32::from(anchor.bottom()));
    let below = bottom + gap;
    let y = if below + height > f32::from(area.bottom())
        && top - height - gap > f32::from(area.top())
    {
        top - height - gap
    } else {
        below
    };
    let right_limit = f32::from(area.right()) - width - 4.;
    let x = (f32::from(anchor.left()) - dx)
        .min(right_limit)
        .max(f32::from(area.left()) + 4.);
    point(px(x), px(y))
}

/// The popup, as a deferred overlay anchored in window coordinates.
#[allow(clippy::too_many_arguments)]
pub fn completion_popup(
    items: &[Completion],
    selected: usize,
    anchor: Bounds<Pixels>,
    area: Bounds<Pixels>,
    scroll: &ScrollHandle,
    on_pick: RowHandler,
    on_hover: RowHandler,
    cx: &App,
) -> AnyElement {
    let p = cx.palette().clone();
    let height = items.len().min(VISIBLE) as f32 * ROW + 10.;
    let origin = place(anchor, area, WIDTH, height, GUTTER, 0.);
    let rows = items.iter().enumerate().map(|(i, item)| {
        let (icon, tint) = kind_icon(item, &p);
        let (pick, hover) = (on_pick.clone(), on_hover.clone());
        div()
            .id(("completion-item", i))
            .flex_none()
            .h(px(ROW))
            .flex()
            .items_center()
            .gap_2()
            .px(px(6.))
            .rounded(theme::radius::SM)
            .when(i == selected, |d| d.bg(p.hover_strong))
            .on_mouse_move(move |_, window, cx| hover(i, window, cx))
            .on_click(move |_, window, cx| pick(i, window, cx))
            .child(
                gpui_component::Icon::new(icon)
                    .size(px(14.))
                    .text_color(tint)
                    .flex_none(),
            )
            .child(
                div()
                    .min_w_0()
                    .truncate()
                    .text_color(p.body)
                    .child(item.label.clone()),
            )
            .child(div().flex_1())
            .when_some(item.detail.clone(), |d, detail| {
                d.child(
                    div()
                        .min_w_0()
                        .truncate()
                        .text_size(theme::text::XS)
                        .text_color(p.faint)
                        .child(detail),
                )
            })
    });
    deferred(
        anchored().position(origin).child(
            div()
                .id("completion-popup")
                .occlude()
                .w(px(WIDTH))
                .max_h(px(height))
                .overflow_y_scroll()
                .track_scroll(scroll)
                .flex()
                .flex_col()
                .p_1()
                .rounded(theme::radius::MD)
                .border_1()
                .border_color(p.border)
                .bg(p.popover)
                .shadow_lg()
                .font_family(theme::MONO_FONT)
                .text_size(theme::text::SM)
                .line_height(theme::text::SM_LINE)
                // Keep the editor focused.
                .on_mouse_down(MouseButton::Left, |_, window, cx| {
                    window.prevent_default();
                    cx.stop_propagation();
                })
                .children(rows),
        ),
    )
    .with_priority(1)
    .into_any_element()
}
