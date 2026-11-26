//! RunProgress.tsx: feedback while a statement is in flight: a thin
//! sweeping bar along the Database Output header ([`run_bar`]), a spinner
//! with the elapsed time for the console toolbar ([`run_status`]); and, when
//! it fails, the error in a strip under the editor ([`run_failed`]).

use std::time::{Duration, Instant};

use boa_core::grid::format::elapsed;
use gpui::prelude::*;
use gpui::{
    Animation, AnimationExt as _, AnyElement, App, ClickEvent, Div, Window, div, px, relative,
};
use gpui_component::Sizable as _;
use gpui_component::spinner::Spinner;

use super::chrome::IconName;
use crate::grid::error::RunError;
use crate::theme::{self, ActivePalette as _};

/// The sweeping bar on the bottom edge of its (relative) parent.
pub fn run_bar(cx: &App) -> AnyElement {
    let color = cx.palette().status_run;
    div()
        .absolute()
        .left_0()
        .right_0()
        .bottom(px(-1.))
        .h(px(2.))
        .overflow_hidden()
        .child(
            div()
                .absolute()
                .top_0()
                .h_full()
                .w(relative(0.3))
                .bg(color)
                .with_animation(
                    "run-bar",
                    Animation::new(Duration::from_millis(1200)).repeat(),
                    |el, delta| el.left(relative(-0.3 + 1.3 * delta)),
                ),
        )
        .into_any_element()
}

/// "Executing… 1.2 s", counting from `since` (the view re-renders while running).
pub fn run_status(since: Instant, cx: &App) -> Div {
    let p = cx.palette();
    div()
        .flex()
        .flex_none()
        .items_center()
        .gap(px(6.))
        .whitespace_nowrap()
        .text_size(theme::text::XS)
        .text_color(p.dim)
        .child(Spinner::new().xsmall().color(p.status_run))
        .child("Executing…")
        .child(elapsed(since.elapsed().as_millis() as i64))
}

/// The console's last statement failed: its error (and the server's detail /
/// hint, and the line with a caret under the error position) in a strip
/// under the editor, with a dismiss button.
pub fn run_failed(
    error: &RunError,
    on_dismiss: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    cx: &App,
) -> Div {
    let p = cx.palette();
    let err = p.destructive;
    let text = p.status_err_text;
    div()
        .flex()
        .flex_none()
        .items_start()
        .gap_2()
        .max_h(px(128.))
        .overflow_hidden()
        .border_t_1()
        .border_color(err.opacity(0.4))
        .bg(err.opacity(0.1))
        .py(px(6.))
        .pr(px(6.))
        .pl_3()
        .font_family(theme::MONO_FONT)
        .text_size(theme::text::SM)
        .line_height(px(20.))
        .child(
            div().mt(px(2.)).flex_none().child(
                gpui_component::Icon::new(IconName::CircleAlert)
                    .size(px(16.))
                    .text_color(err),
            ),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .child(div().text_color(text).child(error.message.clone()))
                .when_some(error.detail.clone(), |d, detail| {
                    d.child(div().text_color(text.opacity(0.8)).child(detail))
                })
                .when_some(error.excerpt.clone(), |d, (line, caret)| {
                    d.child(
                        div()
                            .text_color(text.opacity(0.8))
                            .whitespace_nowrap()
                            .child(line),
                    )
                    .child(div().text_color(text).whitespace_nowrap().child(caret))
                }),
        )
        .child(
            div()
                .id("run-failed-dismiss")
                .flex_none()
                .size(px(20.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(4.))
                .cursor_pointer()
                .text_color(text.opacity(0.7))
                .hover(move |s| s.bg(err.opacity(0.2)).text_color(text))
                .tooltip(super::chrome::tooltip("Dismiss error"))
                .on_click(on_dismiss)
                .child(gpui_component::Icon::new(IconName::X).size(px(14.))),
        )
}
