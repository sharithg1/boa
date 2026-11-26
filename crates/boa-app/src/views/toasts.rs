//! Toasts.tsx: the toasts of [`crate::models::toasts::Toasts`], stacked at
//! the bottom right (16 px from the right edge, 34 px from the bottom, just
//! above the status bar; 8 px apart, newest nearest the corner, at most 400 px
//! wide). A card: status dot (success / error), title (medium for success /
//! error), body (up to 3 lines), optional action button, and a × that shows
//! on hover (success / error).

use gpui::prelude::*;
use gpui::{Context, Entity, FontWeight, SharedString, Subscription, Window, div, px};

use super::chrome::{DotKind, IconName, status_dot};
use crate::models::toasts::{ToastKind, Toasts};
use crate::theme::{self, ActivePalette as _};

/// The toasts overlay.
#[derive(Debug)]
pub struct ToastsView {
    toasts: Entity<Toasts>,
    _subscription: Subscription,
}

impl ToastsView {
    pub fn new(toasts: Entity<Toasts>, cx: &mut Context<Self>) -> Self {
        let subscription = cx.observe(&toasts, |_, _, cx| cx.notify());
        Self {
            toasts,
            _subscription: subscription,
        }
    }
}

impl Render for ToastsView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = cx.palette().clone();
        let toasts = self.toasts.read(cx).toasts().to_vec();
        let model = self.toasts.clone();
        let cards = toasts.into_iter().map(|t| {
            let id = t.id;
            let group: SharedString = format!("toast-{id}").into();
            let plain = t.kind == ToastKind::Info;
            let dot = match t.kind {
                ToastKind::Error => Some(DotKind::Err),
                ToastKind::Success => Some(DotKind::Ok),
                ToastKind::Info => None,
            };
            let action = t.action.clone().map(|action| {
                let model = model.clone();
                div()
                    .id(("toast-action", id))
                    .flex_none()
                    .my(px(-2.))
                    .px(px(6.))
                    .py(px(2.))
                    .rounded(px(4.))
                    .cursor_pointer()
                    .text_size(theme::text::SM)
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(p.body)
                    .hover(|s| s.bg(p.hover_strong))
                    .on_click(move |_, window, cx| {
                        (action.run)(window, cx);
                        model.update(cx, |m, cx| m.dismiss(id, cx));
                    })
                    .child(action.label.clone())
            });
            let dismiss = (!plain).then(|| {
                let model = model.clone();
                div()
                    .id(("toast-dismiss", id))
                    .flex_none()
                    .mr(px(-4.))
                    .size(px(16.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(3.))
                    .cursor_pointer()
                    .text_color(p.dim)
                    .opacity(0.)
                    .group_hover(group.clone(), |s| s.opacity(1.))
                    .hover(|s| s.text_color(p.body))
                    .on_click(move |_, _, cx| model.update(cx, |m, cx| m.dismiss(id, cx)))
                    .child(gpui_component::Icon::new(IconName::X).size(px(14.)))
            });
            div()
                .id(("toast", id))
                .group(group.clone())
                .flex()
                .items_start()
                .gap(px(10.))
                .max_w(px(400.))
                .rounded(theme::radius::LG)
                .border_1()
                .border_color(p.border)
                .bg(p.muted)
                .px(px(13.))
                .py(px(9.))
                .text_size(theme::text::SM)
                .text_color(p.body)
                .shadow_lg()
                .children(dot.map(|kind| div().mt(px(6.)).child(status_dot(kind, 7., cx))))
                .child(
                    div()
                        .min_w_0()
                        .flex_1()
                        .child(
                            div()
                                .when(!plain, |d| d.font_weight(FontWeight::MEDIUM))
                                .child(t.title.clone()),
                        )
                        .children(t.body.clone().map(|body| {
                            div()
                                .mt(px(2.))
                                .text_size(theme::text::XS)
                                .line_height(theme::text::XS_LINE)
                                .text_color(p.quiet)
                                .line_clamp(3)
                                .child(body)
                        })),
                )
                .children(action)
                .children(dismiss)
        });
        div()
            .absolute()
            .right(px(16.))
            .bottom(px(34.))
            .flex()
            .flex_col()
            .items_end()
            .gap(px(8.))
            .children(cards)
    }
}
