//! StatementChooser.tsx: ⌘↵'s "Statements" popup (DataGrip's) when the caret
//! sits in a subquery: pick the whole statement or an inner query. The
//! editor keeps the focus and routes ↑ ↓ Enter Esc here; the highlighted
//! entry is filled in the editor so you can see exactly what will run.

use boa_core::sql::{Span, preview};
use gpui::prelude::*;
use gpui::{AnyElement, App, Bounds, MouseButton, Pixels, anchored, deferred, div, px};

use super::completion_popup::{RowHandler, place};
use crate::theme::{self, ActivePalette as _};

pub const WIDTH: f32 = 400.;
pub const ROW: f32 = 26.;
/// Characters of a statement's one-line preview (statements.ts `preview`).
pub const PREVIEW_CHARS: usize = 60;

/// The chooser, as a deferred overlay under (or above) the caret's line.
#[allow(clippy::too_many_arguments)]
pub fn statement_chooser(
    sql: &str,
    items: &[Span],
    selected: usize,
    anchor: Bounds<Pixels>,
    area: Bounds<Pixels>,
    on_pick: RowHandler,
    on_hover: RowHandler,
    cx: &App,
) -> AnyElement {
    let p = cx.palette().clone();
    let height = items.len() as f32 * ROW + 40.;
    let origin = place(anchor, area, WIDTH, height, 12., 2.);
    let rows = items.iter().enumerate().map(|(i, span)| {
        let (pick, hover) = (on_pick.clone(), on_hover.clone());
        let is_selected = i == selected;
        div()
            .id(("statement-item", i))
            .h(px(ROW))
            .flex()
            .items_center()
            .px_2()
            .rounded(px(4.))
            .font_family(theme::MONO_FONT)
            .text_size(theme::text::SM)
            .when(is_selected, |d| {
                d.bg(p.status_run.opacity(0.3)).text_color(p.body)
            })
            .when(!is_selected, |d| d.text_color(p.soft))
            .on_mouse_move(move |_, window, cx| {
                if !is_selected {
                    hover(i, window, cx)
                }
            })
            .on_click(move |_, window, cx| pick(i, window, cx))
            .child(
                div()
                    .min_w_0()
                    .truncate()
                    .child(preview(sql, *span, PREVIEW_CHARS)),
            )
    });
    deferred(
        anchored().position(origin).child(
            div()
                .id("statement-chooser")
                .occlude()
                .w(px(WIDTH))
                .flex()
                .flex_col()
                .p_1()
                .rounded(theme::radius::MD)
                .border_1()
                .border_color(p.border)
                .bg(p.popover)
                .shadow_lg()
                .on_mouse_down(MouseButton::Left, |_, window, cx| {
                    window.prevent_default();
                    cx.stop_propagation();
                })
                .child(
                    div()
                        .px_2()
                        .pt_1()
                        .pb(px(6.))
                        .text_center()
                        .text_size(theme::text::XS)
                        .font_weight(gpui::FontWeight::MEDIUM)
                        .text_color(p.soft)
                        .child("Statements"),
                )
                .children(rows),
        ),
    )
    .with_priority(1)
    .into_any_element()
}
