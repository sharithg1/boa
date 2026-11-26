//! Pager.tsx: DataGrip-style result paging: first / previous / next page
//! around the shown row range ("501–1,000"). Pages are stateless
//! LIMIT/OFFSET re-runs, so there is no total and no "last page".

use std::rc::Rc;

use gpui::prelude::*;
use gpui::{App, Div, ElementId, Window, div, px};

use super::chrome::{IconName, icon_button};
use crate::grid::page::grouped;
use crate::theme::{self, ActivePalette as _};

/// Where the pager is, and what it may do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PagerState {
    /// Rows skipped before this page.
    pub offset: usize,
    /// Rows on this page.
    pub count: usize,
    pub page_size: usize,
    /// More rows exist after this page.
    pub more: bool,
    /// A page is loading.
    pub busy: bool,
}

impl PagerState {
    /// Pager.tsx renders nothing for a single page.
    pub fn shown(&self) -> bool {
        self.offset > 0 || self.more
    }

    /// "501–1,000", or "1,000+" for an empty page past the end; " …" when more follow.
    pub fn label(&self) -> String {
        let range = if self.count > 0 {
            format!(
                "{}–{}",
                grouped(self.offset + 1),
                grouped(self.offset + self.count)
            )
        } else {
            format!("{}+", grouped(self.offset))
        };
        if self.more {
            format!("{range} …")
        } else {
            range
        }
    }

    pub fn first(&self) -> Option<usize> {
        (!self.busy && self.offset > 0).then_some(0)
    }

    pub fn previous(&self) -> Option<usize> {
        (!self.busy && self.offset > 0).then(|| self.offset.saturating_sub(self.page_size))
    }

    pub fn next(&self) -> Option<usize> {
        (!self.busy && self.more).then(|| self.offset + self.page_size)
    }
}

/// The pager (nothing for a single page); `on_page` gets the new offset.
pub fn pager(
    id: impl Into<ElementId>,
    state: PagerState,
    on_page: impl Fn(usize, &mut Window, &mut App) + 'static,
    cx: &App,
) -> Option<Div> {
    if !state.shown() {
        return None;
    }
    let id: ElementId = id.into();
    let on_page = Rc::new(on_page);
    let p = cx.palette();
    let button =
        |suffix: &'static str, icon: IconName, label: &'static str, target: Option<usize>| {
            let on_page = on_page.clone();
            let el = icon_button((id.clone(), suffix), icon, 14., label, cx);
            match target {
                Some(offset) => el.on_click(move |_, window, cx| on_page(offset, window, cx)),
                None => el.opacity(0.4).cursor_default(),
            }
        };
    Some(
        div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(2.))
            .child(button(
                "first",
                IconName::ChevronsLeft,
                "First page",
                state.first(),
            ))
            .child(button(
                "prev",
                IconName::ChevronLeft,
                "Previous page",
                state.previous(),
            ))
            .child(
                div()
                    .min_w(px(64.))
                    .px_1()
                    .text_center()
                    .font_family(theme::MONO_FONT)
                    .text_size(theme::text::XS)
                    .text_color(p.soft)
                    .child(state.label()),
            )
            .child(button(
                "next",
                IconName::ChevronRight,
                "Next page",
                state.next(),
            )),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn st(offset: usize, count: usize, more: bool) -> PagerState {
        PagerState {
            offset,
            count,
            page_size: 500,
            more,
            busy: false,
        }
    }

    #[test]
    fn hidden_for_one_page() {
        assert!(!st(0, 20, false).shown());
        assert!(st(0, 500, true).shown());
        assert!(st(500, 20, false).shown());
    }

    #[test]
    fn range_label_and_targets() {
        let s = st(500, 500, true);
        assert_eq!(s.label(), "501–1,000 …");
        assert_eq!(
            (s.first(), s.previous(), s.next()),
            (Some(0), Some(0), Some(1000))
        );
        assert_eq!(st(1000, 0, false).label(), "1,000+");
        let first = st(0, 500, true);
        assert_eq!(
            (first.first(), first.previous(), first.next()),
            (None, None, Some(500))
        );
        let busy = PagerState { busy: true, ..s };
        assert_eq!(
            (busy.first(), busy.previous(), busy.next()),
            (None, None, None)
        );
    }
}
