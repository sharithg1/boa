//! TableFilterBar.tsx: DataGrip's filter bar above a table tab's data, a
//! free-form WHERE condition and ORDER BY applied on the server
//! ([`TableTab::apply_query`] builds the SELECT through
//! `boa_core::grid::table_query`). Enter applies both fields (an emptied
//! one is removed), Escape puts back the applied text of the field, × clears
//! a field and applies. Header clicks rewrite the ORDER BY, which then shows
//! here. Text typed but not applied yet is tinted (`text-status-run`).
//!
//! Keys: `enter` → [`ApplyFilter`], `escape` → [`ClearFilter`] in
//! `FilterBar > Input` (keymap.rs), handled here.

use boa_core::grid::table_query::TableQuery;
use gpui::prelude::*;
use gpui::{App, Context, Entity, Focusable as _, SharedString, Subscription, Window, div, px};
use gpui_component::input::{Input, InputEvent, InputState};

use super::chrome::{IconName, icon, tooltip};
use crate::actions::{ApplyFilter, ClearFilter};
use crate::models::table_tab::TableTab;
use crate::theme::{self, ActivePalette as _};

/// Placeholders (TableFilterBar.tsx).
pub const WHERE_PLACEHOLDER: &str = "e.g. status = 'active' and id > 10";
pub const ORDER_BY_PLACEHOLDER: &str = "e.g. created_at desc, id";

/// One of the two fields.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilterField {
    Where,
    OrderBy,
}

impl FilterField {
    pub fn label(self) -> &'static str {
        match self {
            Self::Where => "WHERE",
            Self::OrderBy => "ORDER BY",
        }
    }
}

/// The filter bar of one table tab.
pub struct TableFilterBar {
    table: Entity<TableTab>,
    where_input: Entity<InputState>,
    order_input: Entity<InputState>,
    /// The applied clauses the drafts were last reset to.
    applied: TableQuery,
    _subscriptions: Vec<Subscription>,
}

impl std::fmt::Debug for TableFilterBar {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TableFilterBar")
            .field("applied", &self.applied)
            .finish_non_exhaustive()
    }
}

impl TableFilterBar {
    pub fn new(table: Entity<TableTab>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let applied = table.read(cx).query();
        let field = |text: &str, placeholder: &'static str, window: &mut Window, cx: &mut App| {
            let text = text.to_owned();
            cx.new(|cx| {
                let mut state = InputState::new(window, cx).placeholder(placeholder);
                state.set_value(text, window, cx);
                state
            })
        };
        let where_input = field(&applied.filter, WHERE_PLACEHOLDER, window, cx);
        let order_input = field(&applied.order_by, ORDER_BY_PLACEHOLDER, window, cx);
        let subscriptions = vec![
            cx.subscribe(&where_input, |_, _, e: &InputEvent, cx| {
                if matches!(e, InputEvent::Change) {
                    cx.notify();
                }
            }),
            cx.subscribe(&order_input, |_, _, e: &InputEvent, cx| {
                if matches!(e, InputEvent::Change) {
                    cx.notify();
                }
            }),
            // The applied clauses changed (a header click, a restore): show them.
            cx.observe_in(&table, window, |this, table, window, cx| {
                let now = table.read(cx).query();
                if now != this.applied {
                    this.applied = now;
                    this.reset_drafts(window, cx);
                }
                cx.notify();
            }),
        ];
        Self {
            table,
            where_input,
            order_input,
            applied,
            _subscriptions: subscriptions,
        }
    }

    fn input(&self, field: FilterField) -> &Entity<InputState> {
        match field {
            FilterField::Where => &self.where_input,
            FilterField::OrderBy => &self.order_input,
        }
    }

    /// The field's input (tests, focus).
    pub fn input_of(&self, field: FilterField) -> Entity<InputState> {
        self.input(field).clone()
    }

    fn applied_text(&self, field: FilterField) -> &str {
        match field {
            FilterField::Where => &self.applied.filter,
            FilterField::OrderBy => &self.applied.order_by,
        }
    }

    /// The typed text of a field.
    pub fn draft_text(&self, field: FilterField, cx: &App) -> SharedString {
        self.input(field).read(cx).value()
    }

    /// Both fields as typed.
    pub fn draft(&self, cx: &App) -> TableQuery {
        TableQuery {
            filter: self.draft_text(FilterField::Where, cx).to_string(),
            order_by: self.draft_text(FilterField::OrderBy, cx).to_string(),
        }
    }

    fn reset_drafts(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        for field in [FilterField::Where, FilterField::OrderBy] {
            let text = self.applied_text(field).to_owned();
            self.input(field)
                .update(cx, |i, cx| i.set_value(text, window, cx));
        }
    }

    /// Sets a field's text as if typed (tests, dev hooks).
    pub fn set_draft(
        &mut self,
        field: FilterField,
        text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let text = text.to_owned();
        self.input(field)
            .update(cx, |i, cx| i.set_value(text, window, cx));
        cx.notify();
    }

    /// Enter: applies both fields (asks first about pending edits).
    pub fn apply(&mut self, cx: &mut Context<Self>) {
        let q = self.draft(cx);
        self.table.update(cx, |t, cx| t.apply_query(q, cx));
    }

    /// ×: empties a field and applies.
    pub fn clear(&mut self, field: FilterField, cx: &mut Context<Self>) {
        let mut q = self.draft(cx);
        match field {
            FilterField::Where => q.filter.clear(),
            FilterField::OrderBy => q.order_by.clear(),
        }
        self.table.update(cx, |t, cx| t.apply_query(q, cx));
    }

    /// Escape: the focused field gets its applied text back.
    pub fn restore(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        for field in [FilterField::Where, FilterField::OrderBy] {
            if self
                .input(field)
                .read(cx)
                .focus_handle(cx)
                .is_focused(window)
            {
                let text = self.applied_text(field).to_owned();
                self.input(field)
                    .update(cx, |i, cx| i.set_value(text, window, cx));
            }
        }
        cx.notify();
    }

    fn render_field(
        &self,
        field: FilterField,
        disabled: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let p = cx.palette().clone();
        let draft = self.draft_text(field, cx);
        let changed = draft.as_ref() != self.applied_text(field);
        let id = match field {
            FilterField::Where => "filter-where",
            FilterField::OrderBy => "filter-order-by",
        };
        div()
            .id(id)
            .flex()
            .flex_1()
            .min_w_0()
            .items_center()
            .gap(px(8.))
            .h(px(24.))
            .px(px(8.))
            .rounded(theme::radius::MD)
            .border_1()
            .border_color(p.input)
            .bg(p.field)
            .child(
                div()
                    .flex_none()
                    .font_family(theme::MONO_FONT)
                    // `text-2xs`
                    .text_size(theme::text::XXS)
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .text_color(p.dim)
                    .child(field.label()),
            )
            .child(
                div().flex_1().min_w_0().child(
                    Input::new(self.input(field))
                        .appearance(false)
                        .disabled(disabled)
                        .px_0()
                        .py_0()
                        .h(px(22.))
                        .font_family(theme::MONO_FONT)
                        .text_size(theme::text::XS)
                        .text_color(if changed { p.status_run } else { p.body }),
                ),
            )
            .when(!draft.is_empty() && !disabled, |d| {
                d.child(
                    div()
                        .id(SharedString::from(format!("{id}-clear")))
                        .flex_none()
                        .rounded(theme::radius::SM)
                        .cursor_pointer()
                        .tooltip(tooltip(format!("Clear {}", field.label())))
                        .child(icon(IconName::X, 12., p.dim))
                        .on_click(cx.listener(move |this, _, _, cx| this.clear(field, cx))),
                )
            })
    }
}

impl Render for TableFilterBar {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = cx.palette().clone();
        let disabled = !self.table.read(cx).connected(cx);
        div()
            .key_context(crate::keymap::FILTER_BAR)
            .on_action(cx.listener(|this, _: &ApplyFilter, _, cx| this.apply(cx)))
            .on_action(cx.listener(|this, _: &ClearFilter, window, cx| this.restore(window, cx)))
            .flex()
            .flex_none()
            .items_center()
            .gap(px(8.))
            .h(px(36.))
            .px(px(8.))
            .border_b_1()
            .border_color(p.border)
            .child(self.render_field(FilterField::Where, disabled, cx))
            .child(self.render_field(FilterField::OrderBy, disabled, cx))
    }
}
