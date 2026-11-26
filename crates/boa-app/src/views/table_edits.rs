//! TableEdits.tsx: the data-editing pieces of a table tab around the grid
//! (the grid itself stages the edits, `crate::grid`): the toolbar controls
//! (add / delete row, revert all, "N changes", Preview…, Submit) or the
//! reason the table is read-only, the SQL preview dialog (the exact
//! statements, run in one transaction; a failed submit points at its
//! statement), and the "Discard N pending changes?" prompt
//! (`DiscardEditsDialog`) before a reload, page change, new filter / sort or
//! closing the tab.
//!
//! The state lives in [`TableTab`]; the workspace view wires the events
//! ([`TableTabEvent`]) to [`open_preview`] and [`confirm_discard`].

use std::cell::Cell;

use boa_core::grid::edits::DiscardPrompt;
use boa_core::grid::grid_edits::{EditStatement, Editability, RowMark};
use boa_core::sql::tokenize;
use gpui::prelude::*;
use gpui::{
    AnyElement, App, ClipboardItem, Context, Entity, FocusHandle, Focusable, Global, SharedString,
    StyledText, Subscription, WeakEntity, Window, div, px,
};
use gpui_component::button::{Button, ButtonVariant, ButtonVariants as _};
use gpui_component::spinner::Spinner;
use gpui_component::{Disableable as _, Sizable as _, WindowExt as _};

use super::chrome::{BoaIcon, IconName, icon, kbd, tooltip};
use super::workspace::{tool_button, tool_button_icon};
use crate::app_state::AppState;
use crate::editor::highlight::style_runs;
use crate::grid::ResultGrid;
use crate::models::table_tab::{TableTab, statements_text};
use crate::models::toasts::NewToast;
use crate::theme::{self, ActivePalette as _, Palette};

/// The preview's width (`sm:max-w-3xl`).
pub const PREVIEW_WIDTH: f32 = 768.;
/// The preview's statement list grows up to this height (`max-h-[min(80vh,720px)]`
/// minus header and footer).
const PREVIEW_LIST_MAX: f32 = 560.;

fn plural(n: usize) -> &'static str {
    if n == 1 { "" } else { "s" }
}

/// "Discard 2 pending changes?" (DiscardEditsDialog).
pub fn discard_title(count: usize) -> String {
    format!("Discard {count} pending change{}?", plural(count))
}

/// "They have not been submitted. To reload, they will be lost."
pub fn discard_description(action: &str) -> String {
    format!("They have not been submitted. To {action}, they will be lost.")
}

/// The preview's description line.
pub fn preview_description(n: usize) -> String {
    format!(
        "{n} statement{}, run in one transaction (a savepoint inside an open one). Each UPDATE \
         and DELETE must affect exactly one row; any error rolls back all of them.",
        plural(n)
    )
}

/// "3 changes" in the toolbar.
pub fn changes_label(n: usize) -> String {
    format!("{n} change{}", plural(n))
}

// ── Toolbar ───────────────────────────────────────────────────────────────

/// The edit controls of a table tab's toolbar, or the read-only reason
/// (lock + "Read-only: …"); nothing before the first page.
pub fn toolbar(
    table: &Entity<TableTab>,
    grid: &Entity<ResultGrid>,
    cx: &App,
) -> Option<AnyElement> {
    let p = cx.palette().clone();
    let t = table.read(cx);
    t.result.as_ref()?;
    let reason = match t.editability(cx) {
        Editability::ReadOnly { reason } => reason,
        Editability::Editable { .. } => "",
    };
    if !reason.is_empty() {
        return Some(
            div()
                .id("table-read-only")
                .flex()
                .flex_none()
                .items_center()
                .gap(px(4.))
                .px(px(6.))
                .text_size(theme::text::XS)
                .text_color(p.dim)
                .whitespace_nowrap()
                .tooltip(tooltip(reason))
                .child(icon(IconName::Lock, 12., p.dim))
                .child(reason)
                .into_any_element(),
        );
    }
    let count = t.pending_count();
    let submitting = t.submitting();
    let selected = grid.read(cx).active_row();
    let can_delete = selected.is_some_and(|row| {
        t.pending()
            .and_then(|p| p.row_mark(row))
            .is_none_or(|m| m != RowMark::Deleted)
    });
    let (add_grid, delete_grid) = (grid.clone(), grid.clone());
    let (revert, preview, submit) = (table.clone(), table.clone(), table.clone());
    Some(
        div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(2.))
            .child(
                tool_button(
                    "table-add-row",
                    IconName::Plus,
                    "Add row (⌘N)",
                    true,
                    None,
                    cx,
                )
                .on_click(move |_, window, cx| {
                    let handle = add_grid.focus_handle(cx);
                    window.focus(&handle, cx);
                    window.dispatch_action(Box::new(crate::actions::AddRow), cx);
                }),
            )
            .child(
                tool_button(
                    "table-delete-row",
                    IconName::Minus,
                    "Delete row (⌘⌫)",
                    can_delete,
                    None,
                    cx,
                )
                .on_click(move |_, window, cx| {
                    if can_delete {
                        let handle = delete_grid.focus_handle(cx);
                        window.focus(&handle, cx);
                        window.dispatch_action(Box::new(crate::grid::DeleteRow), cx);
                    }
                }),
            )
            .child(
                tool_button_icon(
                    "table-revert-all",
                    BoaIcon::Undo2.icon(),
                    "Revert all changes",
                    count > 0 && !submitting,
                    None,
                    cx,
                )
                .on_click(move |_, _, cx| revert.update(cx, |t, cx| t.revert_all(cx))),
            )
            .when(count > 0, |d| {
                d.child(
                    div()
                        .id("table-changes")
                        .flex_none()
                        .px(px(4.))
                        .text_size(theme::text::XS)
                        .text_color(p.status_run)
                        .whitespace_nowrap()
                        .tooltip(tooltip("Pending changes"))
                        .child(changes_label(count)),
                )
                .child(
                    xs_button("table-preview", "Preview…", false, &p)
                        .on_click(move |_, _, cx| preview.update(cx, |t, cx| t.open_preview(cx))),
                )
                .child(
                    xs_button("table-submit", "Submit", true, &p)
                        .loading(submitting)
                        .disabled(submitting)
                        .on_click(move |_, _, cx| submit.update(cx, |t, cx| t.submit_edits(cx))),
                )
            })
            .into_any_element(),
    )
}

/// A `size="xs"` button (button.tsx: h-6 gap-1 rounded-md px-2, text-xs
/// font-medium): `primary` (Submit) or ghost in the foreground colour.
fn xs_button(id: &'static str, label: &'static str, primary: bool, p: &Palette) -> Button {
    let button = Button::new(id);
    let button = if primary {
        button.primary()
    } else {
        button.ghost().text_color(p.body)
    };
    xs(button, None, label)
}

/// `button` at `size="xs"`: h-6, rounded-md, text-xs font-medium; `px-2`,
/// or `px-1.5` with a 12 px icon (`size-3`) 4 px before the label (`gap-1`).
fn xs(button: Button, icon_name: Option<IconName>, label: &'static str) -> Button {
    let with_icon = icon_name.is_some();
    button
        .with_size(px(24.))
        .h(px(24.))
        .px(px(if with_icon { 6. } else { 8. }))
        .rounded(theme::radius::MD)
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(4.))
                .text_size(theme::text::XS)
                .line_height(theme::text::XS_LINE)
                .font_weight(gpui::FontWeight::MEDIUM)
                .children(icon_name.map(|n| gpui_component::Icon::new(n).size(px(12.))))
                .child(label),
        )
}

// ── Discard prompt ────────────────────────────────────────────────────────

/// Asks before throwing away `table`'s pending edits (DiscardEditsDialog):
/// Discard goes on with what asked, Keep editing / Esc keeps them.
pub fn confirm_discard(
    table: Entity<TableTab>,
    prompt: &DiscardPrompt,
    window: &mut Window,
    cx: &mut App,
) {
    let title = discard_title(prompt.count);
    let description = discard_description(&prompt.action);
    window.open_alert_dialog(cx, move |alert, _, _| {
        let (ok, cancel, close) = (table.clone(), table.clone(), table.clone());
        alert
            .title(title.clone())
            .description(description.clone())
            .show_cancel(true)
            .cancel_text("Keep editing")
            .ok_text("Discard")
            .ok_variant(ButtonVariant::Danger)
            .on_ok(move |_, _, cx| {
                ok.update(cx, |t, cx| t.resolve_discard(true, cx));
                true
            })
            .on_cancel(move |_, _, cx| {
                cancel.update(cx, |t, cx| t.resolve_discard(false, cx));
                true
            })
            .on_close(move |_, _, cx| {
                // Esc / closed without an answer: keep editing (a no-op after one).
                close.update(cx, |t, cx| t.resolve_discard(false, cx));
            })
    });
}

// ── SQL preview ───────────────────────────────────────────────────────────

/// The preview dialog's content (TableEdits.tsx `PreviewDialog`).
pub struct EditsPreview {
    table: Entity<TableTab>,
    focus_handle: FocusHandle,
    closed: bool,
    _subscriptions: Vec<Subscription>,
}

impl std::fmt::Debug for EditsPreview {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EditsPreview")
            .field("closed", &self.closed)
            .finish_non_exhaustive()
    }
}

/// The preview showing, if any (one at a time).
#[derive(Default)]
struct ActivePreview(Option<WeakEntity<EditsPreview>>);

impl Global for ActivePreview {}

/// The open preview, if any (tests, dev hooks).
pub fn active_preview(cx: &App) -> Option<Entity<EditsPreview>> {
    cx.try_global::<ActivePreview>()?
        .0
        .as_ref()?
        .upgrade()
        .filter(|p| !p.read(cx).closed)
}

impl EditsPreview {
    pub fn new(table: Entity<TableTab>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let subscriptions = vec![cx.observe_in(&table, window, |this, table, window, cx| {
            let t = table.read(cx);
            // Submitted, cancelled elsewhere, or nothing left to submit.
            if (!t.preview || t.pending_count() == 0) && !this.closed {
                this.closed = true;
                window.close_dialog(cx);
            }
            cx.notify();
        })];
        Self {
            table,
            focus_handle: cx.focus_handle(),
            closed: false,
            _subscriptions: subscriptions,
        }
    }

    pub fn table(&self) -> &Entity<TableTab> {
        &self.table
    }

    /// Submit (the button; ↵ is the dialog's Confirm, see [`open_preview`]).
    pub fn submit(&mut self, cx: &mut Context<Self>) {
        self.table.update(cx, |t, cx| t.submit_edits(cx));
    }

    /// Cancel: closes the preview, the edits stay.
    pub fn cancel(&mut self, cx: &mut Context<Self>) {
        self.table.update(cx, |t, cx| t.close_preview(cx));
    }

    /// Copy: the statements as a script.
    pub fn copy(&mut self, cx: &mut Context<Self>) {
        let text = statements_text(&self.table.read(cx).statements(cx));
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        if let Some(state) = AppState::try_global(cx).cloned() {
            state.toasts.update(cx, |t, cx| {
                t.push_for(
                    NewToast::info("Copied SQL"),
                    Some(std::time::Duration::from_millis(1600)),
                    cx,
                );
            });
        }
    }

    /// "Open in console": the statements in a new console; the preview closes.
    pub fn open_in_console(&mut self, cx: &mut Context<Self>) {
        let (key, text) = {
            let t = self.table.read(cx);
            (t.key.clone(), statements_text(&t.statements(cx)))
        };
        self.table.update(cx, |t, cx| t.close_preview(cx));
        if let Some(state) = AppState::try_global(cx).cloned() {
            state
                .workspaces
                .update(cx, |w, cx| w.new_query_tab_with(key, text, cx));
        }
    }
}

/// One statement, highlighted like the editor (SqlLine), with its ";".
fn sql_line(sql: &str, p: &Palette) -> StyledText {
    let tokens = tokenize(sql, None);
    let runs = style_runs(&tokens, &(0..sql.len()), p);
    StyledText::new(format!("{sql};")).with_highlights(runs)
}

impl Focusable for EditsPreview {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for EditsPreview {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = cx.palette().clone();
        let (title, statements, error, submitting): (String, Vec<EditStatement>, _, bool) = {
            let t = self.table.read(cx);
            (
                format!("{}.{}", t.schema, t.table),
                t.statements(cx),
                t.submit_error().cloned(),
                t.submitting(),
            )
        };
        let n = statements.len();
        let failed_at = error.as_ref().and_then(|e| e.index);
        let rows = statements.iter().enumerate().map(|(i, s)| {
            let failed = failed_at == Some(i);
            div()
                .flex()
                .gap(px(12.))
                .px(px(16.))
                .py(px(2.))
                .when(failed, |d| d.bg(p.destructive.opacity(0.15)))
                .child(
                    div()
                        .w(px(24.))
                        .flex_none()
                        .text_right()
                        .text_color(p.dim)
                        .child((i + 1).to_string()),
                )
                .child(
                    div()
                        .min_w_0()
                        .flex_1()
                        .child(div().text_color(p.body).child(sql_line(&s.sql, &p)))
                        .when_some(error.clone().filter(|_| failed), |d, e| {
                            let text = match &e.detail {
                                Some(detail) => format!("{}\n{detail}", e.message),
                                None => e.message.clone(),
                            };
                            d.child(
                                div()
                                    .mt(px(2.))
                                    .font_family(theme::UI_FONT)
                                    .text_size(theme::text::XS)
                                    .text_color(p.status_err_text)
                                    .child(text),
                            )
                        }),
                )
        });
        let rows: Vec<_> = rows.collect();
        let whole_error = error.filter(|e| e.index.is_none());
        // dialog.tsx's close button: a 16 px X at 70 %, top-4 right-4.
        let close = div()
            .id("edits-preview-close")
            .absolute()
            .top(px(16.))
            .right(px(16.))
            .rounded(px(2.))
            .cursor_pointer()
            .opacity(0.7)
            .hover(|s| s.opacity(1.))
            .tooltip(tooltip("Close"))
            .on_click(cx.listener(|this, _, _, cx| this.cancel(cx)))
            .child(icon(IconName::X, 16., p.body));
        // TableEdits.tsx PreviewDialog: `p-0 gap-0` on the dialog surface;
        // the header, list and footer carry their own padding.
        div()
            .id("edits-preview")
            .key_context("EditsPreview")
            .track_focus(&self.focus_handle)
            .relative()
            .flex()
            .flex_col()
            .text_color(p.body)
            .child(
                div()
                    .py(px(12.))
                    .pl(px(16.))
                    .pr(px(48.))
                    .border_b_1()
                    .border_color(p.border)
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .gap(px(4.))
                            .text_size(theme::text::SM)
                            .line_height(theme::text::SM_LINE)
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .child("Submit changes to")
                            .child(div().font_family(theme::MONO_FONT).child(title)),
                    )
                    .child(
                        div()
                            .text_size(theme::text::XS)
                            .line_height(theme::text::XS_LINE)
                            .text_color(p.quiet)
                            .child(preview_description(n)),
                    ),
            )
            .child(
                div()
                    .id("edits-preview-list")
                    .max_h(px(PREVIEW_LIST_MAX))
                    .overflow_y_scroll()
                    .py(px(8.))
                    .bg(p.editor)
                    .font_family(theme::MONO_FONT)
                    .text_size(theme::text::SM)
                    .line_height(theme::text::SM_LINE)
                    .children(rows),
            )
            .when_some(whole_error, |d, e| {
                d.child(
                    div()
                        .px(px(16.))
                        .py(px(8.))
                        .border_t_1()
                        .border_color(p.border)
                        .text_size(theme::text::XS)
                        .line_height(theme::text::XS_LINE)
                        .text_color(p.status_err_text)
                        .child(e.message),
                )
            })
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .px(px(16.))
                    .py(px(10.))
                    .border_t_1()
                    .border_color(p.border)
                    .child(
                        xs(
                            Button::new("preview-copy").ghost().text_color(p.body),
                            Some(IconName::Copy),
                            "Copy",
                        )
                        .on_click(cx.listener(|this, _, _, cx| this.copy(cx))),
                    )
                    .child(
                        xs(
                            Button::new("preview-console").ghost().text_color(p.body),
                            Some(IconName::FileCode),
                            "Open in console",
                        )
                        .on_click(cx.listener(|this, _, _, cx| this.open_in_console(cx))),
                    )
                    .child(div().flex_1())
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .text_size(theme::text::XS)
                            .text_color(p.dim)
                            .child(kbd("↵", cx))
                            .child("submit"),
                    )
                    .child(
                        xs(Button::new("preview-cancel").outline(), None, "Cancel")
                            .on_click(cx.listener(|this, _, _, cx| this.cancel(cx))),
                    )
                    .child(
                        xs(
                            Button::new("preview-submit")
                                .primary()
                                .when(submitting, |b| b.child(Spinner::new().with_size(px(12.)))),
                            None,
                            "Submit",
                        )
                        .disabled(submitting || n == 0)
                        .on_click(cx.listener(|this, _, _, cx| this.submit(cx))),
                    ),
            )
            .child(close)
    }
}

/// Opens the SQL preview of `table` (or keeps the one showing).
pub fn open_preview(
    table: Entity<TableTab>,
    window: &mut Window,
    cx: &mut App,
) -> Option<Entity<EditsPreview>> {
    if let Some(open) = active_preview(cx) {
        if open.read(cx).table == table {
            return Some(open);
        }
        open.update(cx, |p, _| p.closed = true);
        window.close_dialog(cx);
    }
    if !table.read(cx).preview {
        return None;
    }
    let view = cx.new(|cx| EditsPreview::new(table.clone(), window, cx));
    cx.set_global(ActivePreview(Some(view.downgrade())));
    let first = Cell::new(true);
    let content = view.clone();
    window.open_dialog(cx, move |dialog, window, cx| {
        if first.replace(false) {
            let handle = content.focus_handle(cx);
            window.focus(&handle, cx);
        }
        let (on_close, table, ok_table) = (content.downgrade(), table.clone(), table.clone());
        super::connection_dialog::dialog_surface(dialog, cx)
            .p(px(0.))
            .w(px(PREVIEW_WIDTH))
            .close_button(false)
            // ↵ (the dialog's Confirm): submit; the preview closes itself
            // once the edits are in.
            .on_ok(move |_, _, cx| {
                ok_table.update(cx, |t, cx| t.submit_edits(cx));
                false
            })
            .on_close(move |_, _, cx| {
                if let Some(view) = on_close.upgrade() {
                    view.update(cx, |p, _| p.closed = true);
                }
                table.update(cx, |t, cx| t.close_preview(cx));
            })
            .child(content.clone())
    });
    Some(view)
}

/// The SQL of each statement the preview lists (tests).
pub fn preview_sql(view: &Entity<EditsPreview>, cx: &App) -> Vec<SharedString> {
    let t = view.read(cx).table.read(cx);
    t.statements(cx)
        .into_iter()
        .map(|s| SharedString::from(s.sql))
        .collect()
}

#[cfg(feature = "dev-hooks")]
pub mod dev;
#[cfg(test)]
mod tests;
