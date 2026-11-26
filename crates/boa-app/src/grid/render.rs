//! Drawing the grid: the status strip, the sticky header, the visible rows
//! and cells, the error view and the overlays (JSON viewer, value editor).

use std::ops::Range;
use std::time::Instant;

use boa_core::grid::grid_edits::RowMark;
use boa_core::grid::table_query::SortDir;
use boa_core::grid::values::{JsonTokenKind, ValueKind, ValueStyle, json_tokens};
use gpui::prelude::*;
use gpui::{
    AnyElement, Context, DispatchPhase, FontWeight, HighlightStyle, Hsla,
    ListHorizontalSizingBehavior, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent,
    SharedString, StyledText, Window, anchored, canvas, deferred, div, point, px, uniform_list,
};
use gpui_component::Icon;
use gpui_component::input::Input;
use gpui_component::menu::ContextMenuExt as _;
use gpui_component::scroll::Scrollbar;

use super::columns::header_name_min_width;
use super::page::{DEFAULT_PLACEHOLDER, Page, Shown, cell_text, grouped, plural};
use super::selection::Cell;
use super::*;
use crate::theme::{self, ActivePalette as _, Palette};
use crate::views::chrome::{self, IconName};

/// A header's sort arrow and its position in a multi-column sort.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct HeaderSort {
    descending: bool,
    rank: usize,
}

impl ResultGrid {
    fn value_color(p: &Palette, kind: ValueKind) -> Hsla {
        match kind.style() {
            ValueStyle::Number => p.sql_number,
            ValueStyle::Keyword => p.sql_keyword,
            ValueStyle::Param => p.sql_param,
            ValueStyle::Table => p.sql_table,
            ValueStyle::Dim => p.dim,
            ValueStyle::Soft => p.soft,
        }
    }

    pub(crate) fn json_color(p: &Palette, kind: JsonTokenKind) -> Option<Hsla> {
        match kind {
            JsonTokenKind::Key => Some(p.sql_column),
            JsonTokenKind::String => Some(p.sql_string),
            JsonTokenKind::Number => Some(p.sql_number),
            JsonTokenKind::Literal => Some(p.sql_keyword),
            JsonTokenKind::Punct => Some(p.sql_operator),
            JsonTokenKind::Plain => None,
        }
    }

    fn json_text(p: &Palette, value: &str) -> StyledText {
        let head = value
            .char_indices()
            .nth(JSON_HEAD)
            .map_or(value, |(i, _)| &value[..i]);
        let mut text = String::with_capacity(head.len());
        let mut highlights = Vec::new();
        for token in json_tokens(head) {
            let start = text.len();
            text.push_str(&cell_text(&token.text));
            let Some(color) = Self::json_color(p, token.kind) else {
                continue;
            };
            highlights.push((
                start..text.len(),
                HighlightStyle {
                    color: Some(color),
                    ..HighlightStyle::default()
                },
            ));
        }
        StyledText::new(text).with_highlights(highlights)
    }

    fn header_sort(&self, page: &Page, col: usize) -> Option<HeaderSort> {
        match &self.table {
            Some(table) => {
                let name = &page.columns()[col].name;
                table
                    .sort
                    .iter()
                    .find(|(n, _)| n == name)
                    .map(|(_, s)| HeaderSort {
                        descending: s.dir == SortDir::Desc,
                        rank: s.rank,
                    })
            }
            None => page.sort().filter(|s| s.col == col).map(|s| HeaderSort {
                descending: s.descending,
                rank: 1,
            }),
        }
    }

    /// The visible rows of the list (called by `uniform_list` with its range).
    fn render_rows(&mut self, range: Range<usize>, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let started = Instant::now();
        let Some(page) = &self.page else {
            return Vec::new();
        };
        let p = cx.palette().clone();
        let Frame {
            scroll_x,
            viewport_width,
            ..
        } = self.frame;
        let width = if viewport_width > 0. {
            viewport_width
        } else {
            4000.
        };
        let cols = page
            .layout
            .visible(scroll_x, scroll_x + width - page.layout.numbers_width());
        let total = page.layout.numbers_width() + page.layout.total();
        let multi = page.selection.is_multi(page.row_count(), page.col_count());
        let active = page.selection.active();
        let rows = range
            .map(|r| {
                let mark = page.row_mark(r);
                let row_selected = active.is_some_and(|a| a.row == r);
                let mut row = div()
                    .h(px(ROW_HEIGHT))
                    .w(px(total))
                    .flex()
                    .flex_row()
                    .relative()
                    .child(div().flex_none().h_full().w(px(
                        page.layout.numbers_width() + page.layout.start(cols.start)
                    )));
                row = match mark {
                    Some(RowMark::Inserted) => row.bg(p.status_ok.opacity(0.1)),
                    Some(RowMark::Deleted) => row.bg(p.destructive.opacity(0.1)).line_through(),
                    _ if row_selected => row.bg(p.hover),
                    _ => row.hover(|s| s.bg(p.hover)),
                };
                for c in cols.clone() {
                    row = row.child(self.render_cell(page, Cell::new(r, c), mark, multi, &p, cx));
                }
                let marked = multi && page.selection.row_marked(r);
                let number_color = match mark {
                    Some(RowMark::Inserted) => p.status_ok,
                    Some(RowMark::Deleted) => p.destructive,
                    Some(RowMark::Updated) => p.status_run,
                    None if row_selected || marked => p.soft,
                    None => p.dim,
                };
                // The sticky row number: drawn last (on top), at the viewport's left edge.
                row.child(
                    div()
                        .absolute()
                        .top_0()
                        .left(px(scroll_x))
                        .w(px(page.layout.numbers_width()))
                        .h_full()
                        .bg(p.background)
                        .child(
                            div()
                                .size_full()
                                .px(px(8.))
                                .border_r_1()
                                .border_b_1()
                                .border_color(p.border_subtle)
                                .text_right()
                                .text_size(theme::text::XXS)
                                .line_height(px(ROW_HEIGHT - 1.))
                                .text_color(number_color)
                                .when(marked, |d| d.bg(p.status_run.opacity(0.15)))
                                .child(SharedString::from(page.row_label(r))),
                        ),
                )
                .into_any_element()
            })
            .collect();
        self.dev.add_build_time(started.elapsed());
        rows
    }

    fn render_cell(
        &self,
        page: &Page,
        cell: Cell,
        mark: Option<RowMark>,
        multi: bool,
        p: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let kind = page.kinds[cell.col];
        let active = page.selection.active() == Some(cell);
        let edited = mark != Some(RowMark::Inserted) && page.cell_edited(cell);
        let in_range = multi && !active && page.selection.contains(cell);
        let mut d = div()
            .flex_none()
            .w(px(page.layout.width(cell.col)))
            .h_full()
            .px(px(CELL_PAD))
            .border_r_1()
            .border_b_1()
            .border_color(p.border_subtle)
            .truncate()
            .line_height(px(ROW_HEIGHT - 1.))
            .when(kind == ValueKind::Number, |d| d.text_right())
            .when(edited, |d| d.bg(p.status_run.opacity(0.2)))
            .when(in_range, |d| d.bg(p.status_run.opacity(0.15)))
            .when(active, |d| {
                d.bg(p.status_run.opacity(0.25))
                    .border_1()
                    .border_color(p.status_run)
            });
        if let Some(editor) = self.editor.as_ref().filter(|e| e.cell == cell) {
            d = d.relative().child(
                div()
                    .absolute()
                    .inset_0()
                    .bg(p.field)
                    .border_2()
                    .border_color(p.status_run)
                    .px(px(CELL_PAD - 2.))
                    .flex()
                    .items_center()
                    .text_color(p.body)
                    .key_context(EDITOR_CONTEXT)
                    .on_action(cx.listener(|this, _: &CancelEdit, window, cx| {
                        this.finish_edit(false, None, true, window, cx)
                    }))
                    .on_action(cx.listener(|this, _: &CommitEditNext, window, cx| {
                        this.finish_edit(true, Some(1), true, window, cx)
                    }))
                    .on_action(cx.listener(|this, _: &CommitEditPrev, window, cx| {
                        this.finish_edit(true, Some(-1), true, window, cx)
                    }))
                    .child(
                        Input::new(&editor.input)
                            .appearance(false)
                            .px_0()
                            .py_0()
                            .h_full()
                            .text_size(theme::text::SM),
                    ),
            );
            // The input covers the cell; its text is the input's.
            return d.into_any_element();
        }
        match page.shown(cell.row, cell.col) {
            Shown::Null => d.text_color(p.dim).italic().child("<null>"),
            Shown::Default => d.text_color(p.dim).italic().child(DEFAULT_PLACEHOLDER),
            Shown::Text(v) if kind == ValueKind::Json => {
                // The braces button: on hover, always on the active cell.
                let group: SharedString = "grid-json-cell".into();
                let (hover_strong, body) = (p.hover_strong, p.body);
                d.relative()
                    .pr(px(28.))
                    .group(group.clone())
                    .text_color(p.soft)
                    .child(Self::json_text(p, v))
                    .child(
                        div()
                            .id(("view-json", cell.row * 4096 + cell.col))
                            .absolute()
                            .top(px(4.))
                            .right(px(4.))
                            .size(px(20.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(theme::radius::SM)
                            .text_color(p.quiet)
                            .hover(move |s| s.bg(hover_strong).text_color(body))
                            .when(!active, |d| {
                                d.invisible().group_hover(group, |s| s.visible())
                            })
                            .tooltip(chrome::tooltip("View JSON (Enter)"))
                            .on_mouse_down(MouseButton::Left, |_, window, cx| {
                                window.prevent_default();
                                cx.stop_propagation();
                            })
                            .on_click(cx.listener(move |this, _, window, cx| {
                                if let Some(page) = &mut this.page {
                                    page.selection.focus_cell(cell, false);
                                }
                                this.open_json(cell, window, cx);
                            }))
                            .child(Icon::new(IconName::Braces).size(px(14.))),
                    )
            }
            Shown::Text(v) => d
                .text_color(Self::value_color(p, kind))
                .child(SharedString::from(cell_text(v).into_owned())),
        }
        .into_any_element()
    }

    fn render_header(&self, page: &Page, p: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let Frame {
            scroll_x,
            viewport_width,
            ..
        } = self.frame;
        let width = if viewport_width > 0. {
            viewport_width
        } else {
            4000.
        };
        let cols = page
            .layout
            .visible(scroll_x, scroll_x + width - page.layout.numbers_width());
        let total = page.layout.numbers_width() + page.layout.total();
        let resizing = self.resize.map(|r| r.col);
        let server = self.table.is_some();
        let pk: &[String] = self.table.as_ref().map_or(&[], |t| &t.pk);
        let mut strip = div()
            .absolute()
            .top_0()
            .left(px(-scroll_x))
            .h_full()
            .w(px(total))
            .flex()
            .flex_row()
            .child(div().flex_none().h_full().w(px(
                page.layout.numbers_width() + page.layout.start(cols.start)
            )));
        for c in cols.clone() {
            let column = &page.columns()[c];
            let sort = self.header_sort(page, c);
            let title = format!(
                "{} {}\nClick to sort{}",
                column.name,
                column.ty,
                if server {
                    " (⇧: by several columns)"
                } else {
                    ""
                }
            );
            strip = strip.child(
                div()
                    .id(("column-header", c))
                    .debug_selector(move || format!("grid-header-{c}"))
                    .flex_none()
                    .w(px(page.layout.width(c)))
                    .h_full()
                    .px(px(CELL_PAD))
                    .border_r_1()
                    .border_color(p.border)
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .overflow_hidden()
                    .hover(|s| s.bg(p.hover))
                    .tooltip(chrome::tooltip(title))
                    .on_click(cx.listener(move |this, event: &gpui::ClickEvent, _, cx| {
                        this.header_clicked(c, event.modifiers().shift, cx);
                    }))
                    .when(pk.contains(&column.name), |d| {
                        d.child(
                            Icon::new(IconName::KeyRound)
                                .size(px(12.))
                                .text_color(p.status_wait),
                        )
                    })
                    .child(
                        div()
                            .debug_selector(move || format!("grid-header-{c}-name"))
                            .min_w(px(header_name_min_width(&column.name)))
                            .flex_shrink(1.)
                            .truncate()
                            .text_color(p.soft)
                            .child(SharedString::from(column.name.clone())),
                    )
                    .child(
                        // `ml-auto shrink-0`, but it gives way (truncated)
                        // before the name drops below one character.
                        div()
                            .debug_selector(move || format!("grid-header-{c}-type"))
                            .ml_auto()
                            .min_w_0()
                            .flex_shrink(1.)
                            .truncate()
                            .text_size(theme::text::XXS)
                            .text_color(p.sql_type)
                            .child(SharedString::from(column.ty.clone())),
                    )
                    .when_some(sort, |d, sort| {
                        d.child(
                            div()
                                .debug_selector(move || format!("grid-header-{c}-sort"))
                                .flex_none()
                                .flex()
                                .items_center()
                                .child(
                                    Icon::new(if sort.descending {
                                        IconName::ArrowDown
                                    } else {
                                        IconName::ArrowUp
                                    })
                                    .size(px(12.))
                                    .text_color(p.quiet),
                                ),
                        )
                        .when(sort.rank > 1, |d| {
                            d.child(
                                div()
                                    .debug_selector(move || format!("grid-header-{c}-rank"))
                                    .ml(px(-4.))
                                    .flex_none()
                                    .text_size(theme::text::XXS)
                                    .text_color(p.dim)
                                    .child(sort.rank.to_string()),
                            )
                        })
                    }),
            );
        }
        // Resize handles over the column edges (`-right-[3px] w-1.5`).
        for c in cols {
            let x = page.layout.numbers_width() + page.layout.start(c + 1);
            strip = strip.child(
                div()
                    .id(("column-resize", c))
                    .absolute()
                    .top_0()
                    .h_full()
                    .left(px(x - 3.))
                    .w(px(6.))
                    .cursor_col_resize()
                    .when(resizing == Some(c), |d| d.bg(p.border_strong))
                    .hover(|s| s.bg(p.border_strong))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                            cx.stop_propagation();
                            this.start_resize(c, event.position.x, cx);
                        }),
                    ),
            );
        }
        div()
            .flex_none()
            .h(px(HEADER_HEIGHT))
            .w_full()
            .relative()
            .overflow_hidden()
            .border_b_1()
            .border_color(p.border)
            .bg(p.background)
            .font_family(theme::MONO_FONT)
            .child(strip)
            .child(
                div()
                    .absolute()
                    .top_0()
                    .left_0()
                    .w(px(page.layout.numbers_width()))
                    .h_full()
                    .bg(p.background)
                    .border_r_1()
                    .border_color(p.border),
            )
            .into_any_element()
    }

    fn render_grid(&mut self, p: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let Some(page) = &self.page else {
            return div().into_any_element();
        };
        let row_count = page.row_count();
        let header = self.render_header(page, p, cx);
        let pill_text: SharedString = plural(row_count, "row").into();
        let list = uniform_list(
            "result-rows",
            row_count,
            cx.processor(|this, range: Range<usize>, _, cx| this.render_rows(range, cx)),
        )
        .track_scroll(&self.scroll)
        .with_horizontal_sizing_behavior(ListHorizontalSizingBehavior::Unconstrained)
        .size_full()
        .on_mouse_down(MouseButton::Left, cx.listener(Self::body_mouse_down))
        .on_mouse_down(MouseButton::Right, cx.listener(Self::body_right_mouse_down));

        // While dragging (cells, rows or a column edge), follow the mouse
        // anywhere in the window, not just over the grid.
        let tracking = self.drag.is_some() || self.resize.is_some();
        let entity = cx.entity().downgrade();
        let follow = canvas(
            |_, _, _| {},
            move |_, _, window, _| {
                if !tracking {
                    return;
                }
                let moved = entity.clone();
                window.on_mouse_event(move |event: &MouseMoveEvent, phase, _, cx| {
                    if phase == DispatchPhase::Bubble {
                        moved
                            .update(cx, |this, cx| this.mouse_moved(event, cx))
                            .ok();
                    }
                });
                let released = entity.clone();
                window.on_mouse_event(move |_: &MouseUpEvent, phase, _, cx| {
                    if phase == DispatchPhase::Bubble {
                        released.update(cx, |this, cx| this.mouse_released(cx)).ok();
                    }
                });
            },
        )
        .absolute()
        .size_full();

        let menu_entity = cx.entity().downgrade();
        // The UI font for what the trigger's context menu inherits; the rows are mono.
        let body = div()
            .id("result-body")
            .flex_1()
            .min_h_0()
            .relative()
            .font_family(theme::UI_FONT)
            .child(div().size_full().font_family(theme::MONO_FONT).child(list))
            .child(
                div()
                    .absolute()
                    .inset_0()
                    .child(Scrollbar::new(&self.scroll)),
            )
            .when(row_count == 0, |d| {
                d.child(
                    div()
                        .absolute()
                        .top(px(24.))
                        .w_full()
                        .flex()
                        .justify_center()
                        .font_family(theme::UI_FONT)
                        .text_color(p.quiet)
                        .child("No rows"),
                )
            })
            .child(
                div()
                    .absolute()
                    .bottom(px(12.))
                    .w_full()
                    .flex()
                    .justify_center()
                    .child(
                        div()
                            .rounded(theme::radius::MD)
                            .border_1()
                            .border_color(p.border)
                            .bg(p.popover)
                            .px(px(10.))
                            .py(px(2.))
                            .font_family(theme::UI_FONT)
                            .text_size(theme::text::XS)
                            .line_height(theme::text::XS_LINE)
                            .text_color(p.soft)
                            .shadow_md()
                            .child(pill_text),
                    ),
            )
            .child(follow)
            .context_menu(move |menu, window, cx| {
                let Some(model) = menu_entity
                    .upgrade()
                    .and_then(|grid| grid.read(cx).menu_model())
                else {
                    return menu;
                };
                model.build(menu, window, cx)
            });

        div()
            .size_full()
            .flex()
            .flex_col()
            .when(self.resize.is_some(), |d| d.cursor_col_resize())
            .child(header)
            .child(body)
            .into_any_element()
    }

    fn render_status(&self, p: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let info: Option<(SharedString, Hsla)> = if self.running {
            Some(("Running…".into(), p.status_run))
        } else if self.error.is_some() {
            Some(("Failed".into(), p.status_err_text))
        } else if let Some(notice) = &self.notice {
            Some((notice.clone().into(), p.dim))
        } else {
            self.page.as_ref().map(|page| {
                let limit = if page.result.truncated && !page.result.pageable {
                    " · row limit reached"
                } else {
                    ""
                };
                (format!("{} ms{limit}", page.result.ms).into(), p.dim)
            })
        };
        let busy = self.running;
        let pending = self
            .page
            .as_ref()
            .map_or(0, |page| page.pending.change_count());
        let read_only = self.shown_read_only_reason();
        let pager = self
            .page
            .as_ref()
            .filter(|page| page.result.pageable && (page.offset > 0 || page.result.truncated))
            .map(|page| {
                let size = page.page_size.max(1);
                let (offset, count, more) =
                    (page.offset, page.loaded_count(), page.result.truncated);
                let range = if count > 0 {
                    format!("{}–{}", grouped(offset + 1), grouped(offset + count))
                } else {
                    format!("{}+", grouped(offset))
                };
                let button = |id: &'static str, label: &'static str, enabled: bool, to: usize| {
                    div()
                        .id(id)
                        .debug_selector(move || id.to_owned())
                        .px(px(4.))
                        .rounded(theme::radius::SM)
                        .text_color(if enabled { p.soft } else { p.ghost })
                        .when(enabled, |d| {
                            d.hover(|s| s.bg(p.hover_strong)).on_click(cx.listener(
                                move |_, _, _, cx| {
                                    cx.emit(GridEvent::PageRequested { offset: to });
                                },
                            ))
                        })
                        .child(label)
                };
                div()
                    .flex()
                    .items_center()
                    .gap(px(2.))
                    .child(button("page-first", "«", !busy && offset > 0, 0))
                    .child(button(
                        "page-prev",
                        "‹",
                        !busy && offset > 0,
                        offset.saturating_sub(size),
                    ))
                    .child(
                        div()
                            .min_w(px(64.))
                            .px(px(4.))
                            .text_size(theme::text::XS)
                            .text_color(p.soft)
                            .child(format!("{range}{}", if more { " …" } else { "" })),
                    )
                    .child(button("page-next", "›", !busy && more, offset + size))
            });
        div()
            .flex_none()
            .h(px(STATUS_HEIGHT))
            .px(px(8.))
            .flex()
            .items_center()
            .gap(px(8.))
            .border_b_1()
            .border_color(p.border)
            .font_family(theme::UI_FONT)
            .child(
                div()
                    .pl(px(4.))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(p.body)
                    .child("Results"),
            )
            .when_some(info, |d, (text, color)| {
                d.child(
                    div()
                        .text_size(theme::text::XS)
                        .text_color(color)
                        .truncate()
                        .child(text),
                )
            })
            .child(div().flex_1())
            .when_some(read_only, |d, reason| {
                d.child(
                    div()
                        .text_size(theme::text::XS)
                        .text_color(p.dim)
                        .truncate()
                        .child(reason),
                )
            })
            .when(pending > 0, |d| {
                d.child(
                    div()
                        .text_size(theme::text::XS)
                        .text_color(p.status_run)
                        .child(format!("{} staged", plural(pending, "change"))),
                )
            })
            .when_some(pager, |d, pager| d.child(pager))
            .into_any_element()
    }

    fn render_error(error: &RunError, p: &Palette) -> AnyElement {
        div()
            .size_full()
            .p(px(16.))
            .flex()
            .flex_col()
            .gap(px(8.))
            .child(
                div()
                    .text_color(p.status_err_text)
                    .child(SharedString::from(error.message.clone())),
            )
            .when_some(error.detail.clone(), |d, detail| {
                d.children(
                    detail
                        .lines()
                        .map(|l| {
                            div()
                                .text_color(p.soft)
                                .child(SharedString::from(l.to_owned()))
                        })
                        .collect::<Vec<_>>(),
                )
            })
            .when_some(error.excerpt.clone(), |d, (line, caret)| {
                d.child(
                    div()
                        .mt(px(8.))
                        .whitespace_nowrap()
                        .child(div().text_color(p.quiet).child(line))
                        .child(div().text_color(p.status_err_text).child(caret)),
                )
            })
            .into_any_element()
    }

    fn render_message(text: impl Into<SharedString>, color: Hsla) -> AnyElement {
        div()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .font_family(theme::UI_FONT)
            .text_color(color)
            .child(text.into())
            .into_any_element()
    }

    /// The modal layer over the whole window (a scrim that closes on click,
    /// the dialog centred).
    fn render_overlay(
        &self,
        window: &mut Window,
        p: &Palette,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let (content, (w, h)): (AnyElement, (f32, f32)) = match self.overlay.as_ref()? {
            Overlay::Json { viewer, .. } => (viewer.clone().into_any_element(), (768., 720.)),
            Overlay::Value { editor, .. } => (editor.clone().into_any_element(), (672., 560.)),
        };
        let size = window.viewport_size();
        let (vw, vh) = (f32::from(size.width), f32::from(size.height));
        // `sm:max-w-3xl` / `2xl` wide, `min(80vh, 720px)` / `min(70vh, 560px)` tall.
        let height_share = if h > 600. { 0.8 } else { 0.7 };
        let (w, h) = (w.min(vw - 32.), h.min(vh * height_share));
        let scrim = p.scrim;
        Some(
            deferred(
                anchored().position(point(px(0.), px(0.))).child(
                    div()
                        .id("grid-overlay")
                        .occlude()
                        .w(size.width)
                        .h(size.height)
                        .bg(scrim)
                        .flex()
                        .items_center()
                        .justify_center()
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, window, cx| this.close_overlay(window, cx)),
                        )
                        .child(
                            div()
                                .id("grid-dialog")
                                .occlude()
                                .w(px(w))
                                .h(px(h))
                                .rounded(theme::radius::XL)
                                .border_1()
                                .border_color(p.border)
                                .bg(p.popover)
                                .shadow_lg()
                                .overflow_hidden()
                                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                                .child(content),
                        ),
                ),
            )
            .with_priority(2)
            .into_any_element(),
        )
    }
}

impl Render for ResultGrid {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let started = Instant::now();
        self.dev.render_started();
        let (bounds, offset) = self.scroll_state();
        self.frame = Frame {
            scroll_x: (-f32::from(offset.x)).max(0.),
            viewport_width: f32::from(bounds.size.width),
            viewport_height: f32::from(bounds.size.height),
        };
        if let Some(since) = self.paint_probe.take() {
            let rows = self.page.as_ref().map_or(0, Page::row_count);
            window.on_next_frame(move |_, _| {
                tracing::info!(
                    "grid: first paint of {rows} rows {} ms after the content arrived",
                    since.elapsed().as_millis()
                );
            });
            dev::on_first_paint(self, window, cx);
        }
        let p = cx.palette().clone();
        let body = if let Some(error) = &self.error {
            Self::render_error(error, &p)
        } else if self.page.is_some() {
            self.render_grid(&p, cx)
        } else if self.running {
            Self::render_message("Running…", p.quiet)
        } else if let Some(notice) = &self.notice {
            Self::render_message(notice.clone(), p.quiet)
        } else {
            Self::render_message("Run a statement with ⌘↵ to see its result here", p.dim)
        };
        let overlay = self.render_overlay(window, &p, cx);
        let grid = div()
            .id("result-grid")
            .key_context(CONTEXT)
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(|this, _: &MoveUp, _, cx| this.move_active(-1, 0, false, cx)))
            .on_action(cx.listener(|this, _: &MoveDown, _, cx| this.move_active(1, 0, false, cx)))
            .on_action(cx.listener(|this, _: &MoveLeft, _, cx| this.move_active(0, -1, false, cx)))
            .on_action(cx.listener(|this, _: &MoveRight, _, cx| this.move_active(0, 1, false, cx)))
            .on_action(cx.listener(|this, _: &SelectUp, _, cx| this.move_active(-1, 0, true, cx)))
            .on_action(cx.listener(|this, _: &SelectDown, _, cx| this.move_active(1, 0, true, cx)))
            .on_action(cx.listener(|this, _: &SelectLeft, _, cx| this.move_active(0, -1, true, cx)))
            .on_action(cx.listener(|this, _: &SelectRight, _, cx| this.move_active(0, 1, true, cx)))
            .on_action(cx.listener(|this, _: &PageUp, _, cx| {
                let n = this.visible_rows() as isize;
                this.move_active(-n, 0, false, cx)
            }))
            .on_action(cx.listener(|this, _: &PageDown, _, cx| {
                let n = this.visible_rows() as isize;
                this.move_active(n, 0, false, cx)
            }))
            .on_action(cx.listener(|this, _: &SelectAll, _, cx| this.select_all(cx)))
            .on_action(cx.listener(|this, _: &Copy, _, cx| this.copy(None, cx)))
            .on_action(cx.listener(Self::copy_as))
            // Edit → Select All / Copy in the menu bar dispatch the input
            // actions; in the grid they mean the grid's own.
            .on_action(
                cx.listener(|this, _: &gpui_component::input::SelectAll, _, cx| {
                    this.select_all(cx)
                }),
            )
            .on_action(
                cx.listener(|this, _: &gpui_component::input::Copy, _, cx| this.copy(None, cx)),
            )
            .on_action(cx.listener(|this, _: &Collapse, window, cx| this.collapse(window, cx)))
            .on_action(cx.listener(|this, _: &Edit, window, cx| this.edit_active(window, cx)))
            .on_action(cx.listener(|this, _: &EditLarge, window, cx| this.edit_large(window, cx)))
            .on_action(cx.listener(|this, _: &ViewJson, window, cx| this.view_json(window, cx)))
            .on_action(cx.listener(|this, _: &SetNull, window, cx| this.set_null(window, cx)))
            .on_action(cx.listener(|this, _: &Revert, _, cx| this.revert(cx)))
            .on_action(cx.listener(|this, _: &RevertCell, _, cx| this.revert_cell(cx)))
            .on_action(cx.listener(|this, _: &RevertRow, _, cx| this.revert_row(cx)))
            .on_action(cx.listener(|this, _: &DeleteRow, _, cx| this.delete_row(cx)))
            .on_action(
                cx.listener(|this, _: &crate::actions::AddRow, window, cx| {
                    this.add_row(window, cx)
                }),
            )
            .on_action(
                cx.listener(|this, _: &crate::actions::SubmitEdits, window, cx| {
                    this.submit(window, cx)
                }),
            )
            .on_key_down(cx.listener(Self::key_down))
            .size_full()
            .flex()
            .flex_col()
            .bg(p.background)
            .font_family(theme::MONO_FONT)
            .text_size(theme::text::SM)
            .line_height(theme::text::SM_LINE)
            .text_color(p.body)
            .when(self.toolbar, |d| d.child(self.render_status(&p, cx)))
            .child(div().flex_1().min_h_0().relative().child(body))
            .when_some(self.dev.paint_marker(), |d, marker| {
                // Painted last: marks the end of the grid's paint (bench only).
                d.child(
                    canvas(
                        |_, _, _| {},
                        move |_, _, _, _| marker.set(Some(Instant::now())),
                    )
                    .absolute()
                    .size_0(),
                )
            });
        self.dev.add_build_time(started.elapsed());
        // The overlay sits outside the grid's key context: keys in the
        // viewer / editor never reach the grid's bindings.
        div().size_full().child(grid).children(overlay)
    }
}
