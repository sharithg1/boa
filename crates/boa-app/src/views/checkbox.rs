//! checkbox.tsx at its real size. gpui-component's `Checkbox` sizes its box
//! in rems, and the app's rem is 13 px (see [`crate::theme`]), so its largest
//! box came out at 14.6 px. [`checkbox`] lays a medium `Checkbox` out under
//! a 16 px rem ([`WithRem`]): a 16 px box (`size-4`, `rounded-[4px]`), an
//! 8 px gap (`gap-2`) and a 12 px check. The label is the caller's, in px.

use gpui::prelude::*;
use gpui::{
    AnyElement, App, Bounds, ElementId, GlobalElementId, InspectorElementId, LayoutId, Pixels,
    SharedString, Window, div, px,
};
use gpui_component::checkbox::Checkbox;

use crate::theme::{self, Palette};

/// The rem the checkbox is laid out with: Tailwind's 16 px.
const CHECKBOX_REM: Pixels = px(16.);

/// Lays `child` out, prepaints and paints it with `rem` as the window's rem
/// size, so rem-sized gpui-component parts come out at their Tailwind size.
pub struct WithRem {
    rem: Pixels,
    child: AnyElement,
}

impl std::fmt::Debug for WithRem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WithRem")
            .field("rem", &self.rem)
            .finish_non_exhaustive()
    }
}

impl WithRem {
    pub fn new(rem: Pixels, child: impl IntoElement) -> Self {
        Self {
            rem,
            child: child.into_any_element(),
        }
    }
}

impl Element for WithRem {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        let child = &mut self.child;
        let id = window.with_rem_size(Some(self.rem), |window| child.request_layout(window, cx));
        (id, ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        let child = &mut self.child;
        window.with_rem_size(Some(self.rem), |window| child.prepaint(window, cx));
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        let child = &mut self.child;
        window.with_rem_size(Some(self.rem), |window| child.paint(window, cx));
    }
}

/// A labelled checkbox as the source's `Label` + `Checkbox`: a 16 px box,
/// `gap-2`, the label text-sm in `text-soft`. Chain `on_click` /
/// `disabled` on the returned `Checkbox`, then wrap it with [`sized`].
pub fn checkbox(
    id: impl Into<ElementId>,
    checked: bool,
    label: impl Into<SharedString>,
    p: &Palette,
) -> Checkbox {
    Checkbox::new(id).checked(checked).text_color(p.soft).child(
        div()
            .text_size(theme::text::SM)
            .line_height(theme::text::SM_LINE)
            .child(label.into()),
    )
}

/// `checkbox` (or any `Checkbox`) at its Tailwind size.
pub fn sized(checkbox: Checkbox) -> WithRem {
    WithRem::new(CHECKBOX_REM, checkbox)
}

#[cfg(test)]
mod tests {
    use gpui::{Context, Render, TestAppContext, Window, rems};
    use gpui_component::Root;

    use super::*;

    struct Probe;

    impl Render for Probe {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
                .flex()
                .child(div().size(rems(1.)).debug_selector(|| "app-rem".into()))
                .child(WithRem::new(
                    CHECKBOX_REM,
                    div().size(rems(1.)).debug_selector(|| "scoped-rem".into()),
                ))
        }
    }

    #[gpui::test]
    fn with_rem_lays_its_child_out_with_a_16_px_rem(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_component::init(cx);
            crate::theme::init(cx);
        });
        let (_, vcx) = cx.add_window_view(|window, cx| {
            let probe = cx.new(|_| Probe);
            Root::new(probe, window, cx)
        });
        vcx.run_until_parked();
        vcx.update(|window, cx| window.draw(cx).clear(cx));
        // gpui-component's Root: the rem is the 13 px body size ...
        let app = vcx.debug_bounds("app-rem").expect("drawn");
        assert_eq!(app.size.width, px(13.));
        // ... and 16 px (Tailwind's) inside WithRem.
        let scoped = vcx.debug_bounds("scoped-rem").expect("drawn");
        assert_eq!(scoped.size.width, px(16.));
        assert_eq!(scoped.size.height, px(16.));
    }
}
