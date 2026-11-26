//! The grid's input: mouse (cells, row numbers, column edges, header
//! sorting), keys (selection, copy, editing shortcuts), the inline cell
//! input, the JSON viewer / value editor overlays and the right-click menu
//! (DataGrid.tsx `cellDown`, `rowDown`, `onKeyDown`, `editKey`,
//! `startEdit` / `finishEdit`, `openBig`, `openJson`, `copy`, `editMenu`).

use boa_core::grid::copy_as::CopyFormat;
use boa_core::grid::grid_edits::{Editability, RowMark};
use boa_core::grid::values::ValueKind;
use boa_core::model::Value;
use gpui::prelude::*;
use gpui::{
    ClipboardItem, Context, Focusable as _, KeyDownEvent, Keystroke, MouseButton, MouseDownEvent,
    MouseMoveEvent, Pixels, ScrollStrategy, Window, div, point, px,
};
use gpui_component::input::{InputEvent, InputState};
use gpui_component::kbd::Kbd;
use gpui_component::menu::{PopupMenu, PopupMenuItem};
use gpui_component::{ActiveTheme as _, Icon, Sizable as _};

use super::page::{CopyTarget, Page};
use super::selection::{Cell, RowClick};
use super::{
    COPY_TOAST_FOR, CellEditor, ColumnResize, CopyAs, GridEvent, Hit, LONG_VALUE, Overlay,
    ROW_HEIGHT, ResultGrid,
};
use crate::models::toasts::NewToast;
use crate::views::json_viewer::{JsonTarget, JsonViewer, JsonViewerEvent};
use crate::views::value_editor::{ValueEditor, ValueEditorEvent, ValueTarget};

impl ResultGrid {
    // ── Geometry ──────────────────────────────────────────────────────────

    pub(super) fn scroll_state(&self) -> (gpui::Bounds<Pixels>, gpui::Point<Pixels>) {
        let s = self.scroll.0.borrow();
        (s.base_handle.bounds(), s.base_handle.offset())
    }

    /// What `position` (window coordinates) is over. `clamp`: positions
    /// outside the grid map to the nearest cell (dragging past the edges).
    pub(super) fn hit(&self, position: gpui::Point<Pixels>, clamp: bool) -> Option<Hit> {
        let page = self.page.as_ref()?;
        let n = page.row_count();
        if n == 0 || page.col_count() == 0 {
            return None;
        }
        let (bounds, offset) = self.scroll_state();
        let vx = f32::from(position.x - bounds.origin.x);
        let vy = f32::from(position.y - bounds.origin.y);
        let row = ((vy - f32::from(offset.y)) / ROW_HEIGHT).floor();
        let row = if clamp {
            row.clamp(0., (n - 1) as f32) as usize
        } else if row < 0. || row >= n as f32 {
            return None;
        } else {
            row as usize
        };
        let data_x = vx - f32::from(offset.x) - page.layout.numbers_width();
        if !clamp && vx < page.layout.numbers_width() {
            return Some(Hit::RowNumber(row));
        }
        let col = if clamp {
            page.layout.column_at_clamped(data_x)?
        } else {
            page.layout.column_at(data_x)?
        };
        Some(Hit::Cell(Cell::new(row, col)))
    }

    /// Scrolls so that the cell is visible (`scrollIntoView({block: 'nearest',
    /// inline: 'nearest'})`).
    pub(super) fn reveal(&self, cell: Cell) {
        let Some(page) = &self.page else { return };
        self.scroll
            .scroll_to_item(cell.row, ScrollStrategy::Nearest);
        let (bounds, offset) = self.scroll_state();
        let width = f32::from(bounds.size.width) - page.layout.numbers_width();
        if width <= 0. || cell.col >= page.col_count() {
            return;
        }
        let scroll_x = -f32::from(offset.x);
        let next = page.layout.scroll_to_reveal(cell.col, scroll_x, width);
        if next != scroll_x {
            self.scroll
                .0
                .borrow()
                .base_handle
                .set_offset(point(px(-next), offset.y));
        }
    }

    pub(super) fn visible_rows(&self) -> usize {
        ((self.frame.viewport_height / ROW_HEIGHT).floor() as usize).max(2) - 1
    }

    // ── Mouse ─────────────────────────────────────────────────────────────

    pub(super) fn body_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(hit) = self.hit(event.position, false) else {
            return;
        };
        if let (Hit::Cell(cell), Some(editor)) = (hit, &self.editor)
            && editor.cell == cell
        {
            return; // a click inside the inline input
        }
        self.finish_edit(true, None, false, window, cx);
        window.focus(&self.focus_handle, cx);
        // We focus ourselves (and maybe the inline input below): keep the
        // root's track_focus from refocusing the grid after us.
        window.prevent_default();
        let editable = self.can_edit_any();
        let Some(page) = &mut self.page else { return };
        let m = event.modifiers;
        match hit {
            Hit::Cell(cell) => {
                if event.click_count >= 2 && !m.shift {
                    page.selection.press_cell(cell, false);
                    self.drag = None;
                    // Double-click: edit (data editor) or view JSON.
                    if editable {
                        self.start_edit(cell, None, window, cx);
                    } else {
                        self.open_json(cell, window, cx);
                    }
                    return;
                }
                self.drag = Some(page.selection.press_cell(cell, m.shift));
            }
            Hit::RowNumber(row) => {
                let how = if m.shift {
                    RowClick::Extend
                } else if m.platform {
                    page.selection.expand_all(page.row_count());
                    RowClick::Toggle
                } else {
                    RowClick::Replace
                };
                self.drag = page.selection.press_row(row, how);
            }
        }
        cx.notify();
    }

    /// Right button: select the cell (or row) under the mouse unless it is
    /// already selected, so the menu acts on it (`cellDown` / `rowDown` with button 2).
    pub(super) fn body_right_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.finish_edit(true, None, false, window, cx);
        window.focus(&self.focus_handle, cx);
        let Some(hit) = self.hit(event.position, false) else {
            return;
        };
        let Some(page) = &mut self.page else { return };
        match hit {
            Hit::Cell(cell) => {
                if !page.selection.contains(cell) {
                    page.selection.press_cell(cell, false);
                }
            }
            Hit::RowNumber(row) => {
                let picked = page
                    .selection
                    .picked()
                    .is_some_and(|_| page.selection.row_marked(row));
                if !picked {
                    page.selection.press_row(row, RowClick::Replace);
                }
            }
        }
        self.drag = None;
        cx.notify();
    }

    pub(super) fn mouse_moved(&mut self, event: &MouseMoveEvent, cx: &mut Context<Self>) {
        if let Some(resize) = self.resize {
            if event.pressed_button != Some(MouseButton::Left) {
                self.resize = None;
                cx.notify();
                return;
            }
            if let Some(page) = &mut self.page {
                let dx = f32::from(event.position.x - resize.start_x);
                page.layout.resize(resize.col, resize.start_width + dx);
                cx.notify();
            }
            return;
        }
        let Some(kind) = self.drag else { return };
        if event.pressed_button != Some(MouseButton::Left) {
            self.drag = None;
            return;
        }
        let Some(Hit::Cell(cell)) = self.hit(event.position, true) else {
            return;
        };
        let Some(page) = &mut self.page else { return };
        let before = page.selection.clone();
        page.selection.drag_to(kind, cell);
        if page.selection != before {
            self.reveal(cell);
            cx.notify();
        }
    }

    pub(super) fn mouse_released(&mut self, cx: &mut Context<Self>) {
        if self.drag.take().is_some() | self.resize.take().is_some() {
            cx.notify();
        }
    }

    pub(super) fn start_resize(&mut self, col: usize, x: Pixels, cx: &mut Context<Self>) {
        let Some(page) = &self.page else { return };
        self.resize = Some(ColumnResize {
            col,
            start_x: x,
            start_width: page.layout.width(col),
        });
        cx.notify();
    }

    /// Header click: a console result sorts locally (asc → desc → none);
    /// a table tab asks its owner for an ORDER BY (⇧ adds the column).
    pub fn header_clicked(&mut self, col: usize, shift: bool, cx: &mut Context<Self>) {
        if self.table.is_some() {
            cx.emit(GridEvent::ServerSortRequested {
                column: col,
                additive: shift,
            });
            return;
        }
        if self.editor.is_some() {
            return;
        }
        if let Some(page) = &mut self.page {
            page.cycle_sort(col);
            cx.notify();
        }
    }

    // ── Keys ──────────────────────────────────────────────────────────────

    pub(super) fn move_active(
        &mut self,
        dr: isize,
        dc: isize,
        extend: bool,
        cx: &mut Context<Self>,
    ) {
        if self.editor.is_some() {
            return;
        }
        let Some(page) = &mut self.page else { return };
        let (rows, cols) = (page.row_count(), page.col_count());
        if let Some(cell) = page.selection.move_by(dr, dc, extend, rows, cols) {
            self.reveal(cell);
            cx.notify();
        }
    }

    pub(super) fn select_all(&mut self, cx: &mut Context<Self>) {
        if let Some(page) = &mut self.page
            && self.editor.is_none()
            && page.selection.select_all()
        {
            cx.notify();
        }
    }

    fn copy_target(&self) -> CopyTarget<'_> {
        match &self.table {
            Some(t) => CopyTarget {
                table: Some(t.table.as_str()),
                pk: &t.pk,
            },
            None => CopyTarget::default(),
        }
    }

    /// Copies the selection: TSV (⌘C), or `format` from "Copy as". Shows a toast.
    pub fn copy(&mut self, format: Option<CopyFormat>, cx: &mut Context<Self>) {
        if self.editor.is_some() {
            return;
        }
        let Some(page) = &self.page else { return };
        let Some(copied) = page.copy(format, self.copy_target()) else {
            return;
        };
        match copied {
            Ok(copied) => {
                cx.write_to_clipboard(ClipboardItem::new_string(copied.text));
                Self::toast(NewToast::info(copied.title), Some(COPY_TOAST_FOR), cx);
            }
            Err(err) => Self::toast(NewToast::error("Copy failed").body(err), None, cx),
        }
    }

    pub(super) fn copy_as(&mut self, action: &CopyAs, _: &mut Window, cx: &mut Context<Self>) {
        self.copy(Some(action.0), cx);
    }

    pub(super) fn collapse(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.editor.is_some() {
            self.finish_edit(false, None, true, window, cx);
            return;
        }
        let collapsed = self
            .page
            .as_mut()
            .is_some_and(|page| page.selection.collapse());
        if collapsed {
            cx.notify();
        } else {
            cx.propagate();
        }
    }

    fn active(&self) -> Option<Cell> {
        self.page.as_ref().and_then(|p| p.selection.active())
    }

    /// Enter / F2: edit the active cell; read-only, Enter on a JSON cell opens the viewer.
    pub(super) fn edit_active(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.editor.is_some() {
            return;
        }
        let Some(cell) = self.active() else { return };
        if self.can_edit_any() {
            self.start_edit(cell, None, window, cx);
        } else {
            self.open_json(cell, window, cx);
        }
    }

    /// ⇧↵: the value editor (also from inside the inline input, dropping its text).
    pub(super) fn edit_large(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let cell = match self.editor.take() {
            Some(editor) => editor.cell,
            None => match self.active() {
                Some(cell) => cell,
                None => return,
            },
        };
        self.open_value_editor(cell, window, cx);
    }

    /// Runs `f` on the page and announces a change of the staged edits.
    fn edit_pending(&mut self, cx: &mut Context<Self>, f: impl FnOnce(&mut Page)) {
        let Some(page) = &mut self.page else { return };
        let before = page.pending.clone();
        f(page);
        if page.pending != before {
            cx.emit(GridEvent::PendingChanged);
        }
        cx.notify();
    }

    pub(super) fn set_null(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.can_edit_any() {
            return;
        }
        // Inside the inline input too: cancel it and set NULL (DataGrid.tsx).
        self.finish_edit(false, None, true, window, cx);
        let Some(cell) = self.active() else { return };
        self.edit_pending(cx, |page| {
            if page.can_edit(cell.row) {
                page.stage(cell, None);
            }
        });
    }

    pub(super) fn revert(&mut self, cx: &mut Context<Self>) {
        if !self.can_edit_any() || self.editor.is_some() {
            return;
        }
        let Some(cell) = self.active() else { return };
        self.edit_pending(cx, |page| page.revert(cell));
    }

    pub(super) fn revert_cell(&mut self, cx: &mut Context<Self>) {
        if !self.can_edit_any() || self.editor.is_some() {
            return;
        }
        let Some(cell) = self.active() else { return };
        self.edit_pending(cx, |page| page.revert_cell(cell));
    }

    pub(super) fn revert_row(&mut self, cx: &mut Context<Self>) {
        if !self.can_edit_any() || self.editor.is_some() {
            return;
        }
        let Some(cell) = self.active() else { return };
        self.edit_pending(cx, |page| page.revert_row(cell.row));
    }

    pub(super) fn delete_row(&mut self, cx: &mut Context<Self>) {
        if !self.can_edit_any() || self.editor.is_some() {
            return;
        }
        let Some(cell) = self.active() else { return };
        self.edit_pending(cx, |page| page.delete_row(cell.row));
    }

    /// ⌘N / Alt+Insert: a new row at the end, selected (works without a selection).
    pub(super) fn add_row(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.can_edit_any() {
            return;
        }
        self.finish_edit(true, None, false, window, cx);
        let mut added = None;
        self.edit_pending(cx, |page| {
            let row = page.add_row();
            let cell = Cell::new(row, 0);
            page.selection.focus_cell(cell, false);
            added = Some(cell);
        });
        if let Some(cell) = added {
            self.reveal(cell);
        }
        window.focus(&self.focus_handle, cx);
    }

    /// ⌘↵: ask the owner to preview and submit the staged edits.
    pub(super) fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.can_edit_any() {
            cx.propagate();
            return;
        }
        self.finish_edit(true, None, true, window, cx);
        cx.emit(GridEvent::SubmitRequested);
    }

    /// Typing a character on a selected cell starts editing it with that text.
    pub(super) fn key_down(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.editor.is_some() || !self.can_edit_any() {
            return;
        }
        let m = event.keystroke.modifiers;
        if m.platform || m.control || m.alt || m.function {
            return;
        }
        let Some(text) = event.keystroke.key_char.as_deref() else {
            return;
        };
        let mut chars = text.chars();
        let (Some(ch), None) = (chars.next(), chars.next()) else {
            return;
        };
        if ch.is_control() {
            return;
        }
        let Some(cell) = self.active() else { return };
        if self.start_edit(cell, Some(text.to_owned()), window, cx) {
            // Handled: the key must not also reach the new input as text.
            cx.stop_propagation();
        }
    }

    // ── Inline edit ───────────────────────────────────────────────────────

    /// Opens the inline input on a cell (`startEdit`); `initial`: the
    /// character typed to start. Multi-line, long (> 200 chars) and JSON
    /// values open the value editor instead. False when the cell can't be edited.
    pub fn start_edit(
        &mut self,
        cell: Cell,
        initial: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.can_edit_any() {
            return false;
        }
        let Some(page) = &mut self.page else {
            return false;
        };
        if !page.can_edit(cell.row) || cell.col >= page.col_count() {
            return false;
        }
        let value = page.value(cell.row, cell.col).map(str::to_owned);
        let big = initial.is_none()
            && value.as_deref().is_some_and(|v| {
                v.contains('\n')
                    || v.chars().count() > LONG_VALUE
                    || page.kinds[cell.col] == ValueKind::Json
            });
        if big {
            return self.open_value_editor(cell, window, cx);
        }
        page.selection.focus_cell(cell, false);
        let touched = initial.is_some();
        let text = initial.or_else(|| value.clone()).unwrap_or_default();
        let input = cx.new(|cx| {
            let state = InputState::new(window, cx);
            if value.is_none() && !touched {
                state.placeholder("<null>")
            } else {
                state
            }
        });
        input.update(cx, |state, cx| {
            state.set_value(text, window, cx);
            if !touched {
                state.select_all(window, cx);
            }
            state.focus(window, cx);
        });
        let events =
            cx.subscribe_in(
                &input,
                window,
                |this, _, event: &InputEvent, window, cx| match event {
                    InputEvent::Change => {
                        if let Some(editor) = &mut this.editor {
                            editor.touched = true;
                        }
                    }
                    InputEvent::PressEnter { .. } => this.finish_edit(true, None, true, window, cx),
                    InputEvent::Blur => this.finish_edit(true, None, false, window, cx),
                    InputEvent::Focus => {}
                },
            );
        self.editor = Some(CellEditor {
            cell,
            input,
            touched,
            _events: events,
        });
        self.reveal(cell);
        cx.notify();
        true
    }

    /// Closes the inline input (`finishEdit`): `save` stages the text if it
    /// changed, `step` then moves the active cell (Tab / ⇧Tab).
    pub(super) fn finish_edit(
        &mut self,
        save: bool,
        step: Option<isize>,
        refocus: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.editor.is_none() {
            return;
        }
        let cell = self.editor.as_ref().map(|e| e.cell);
        self.close_editor(save, cx);
        if let (Some(dc), Some(cell), Some(page)) = (step, cell, &mut self.page) {
            let cols = page.col_count();
            let col = cell.col.saturating_add_signed(dc).min(cols - 1);
            let next = Cell::new(cell.row.min(page.row_count().saturating_sub(1)), col);
            page.selection.focus_cell(next, false);
            self.reveal(next);
        }
        if refocus {
            window.focus(&self.focus_handle, cx);
        }
        cx.notify();
    }

    /// Drops the inline input, staging its text first with `save` (when changed).
    pub(super) fn close_editor(&mut self, save: bool, cx: &mut Context<Self>) {
        let Some(editor) = self.editor.take() else {
            return;
        };
        if save && editor.touched {
            let text = editor.input.read(cx).value().to_string();
            self.edit_pending(cx, |page| page.stage(editor.cell, Some(text)));
        }
        cx.notify();
    }

    // ── Overlays ──────────────────────────────────────────────────────────

    /// Opens the JSON viewer on a cell if it holds JSON (`openJson`).
    pub fn open_json(&mut self, cell: Cell, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some(page) = &self.page else { return false };
        if cell.col >= page.col_count() || page.kinds[cell.col] != ValueKind::Json {
            return false;
        }
        let Some(value) = page.value(cell.row, cell.col) else {
            return false;
        };
        let column = &page.columns()[cell.col];
        let target = JsonTarget {
            column: column.name.clone(),
            ty: column.ty.clone(),
            row: page.row_number(cell.row),
            value: value.to_owned(),
        };
        let viewer = cx.new(|cx| JsonViewer::new(target, cx));
        let events = cx.subscribe_in(
            &viewer,
            window,
            |this, _, event: &JsonViewerEvent, window, cx| match event {
                JsonViewerEvent::Close => this.close_overlay(window, cx),
            },
        );
        window.focus(&viewer.focus_handle(cx), cx);
        self.overlay = Some(Overlay::Json {
            viewer,
            _events: events,
        });
        cx.notify();
        true
    }

    /// The braces button / ViewJson action: the active cell in the viewer.
    pub(super) fn view_json(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(cell) = self.active() {
            self.open_json(cell, window, cx);
        }
    }

    /// Opens the value editor on a cell (`openBig`). False when it can't be edited.
    pub fn open_value_editor(
        &mut self,
        cell: Cell,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.can_edit_any() {
            return false;
        }
        let Some(page) = &mut self.page else {
            return false;
        };
        if !page.can_edit(cell.row) || cell.col >= page.col_count() {
            return false;
        }
        let Some(row) = page.row_ref(cell.row) else {
            return false;
        };
        page.selection.focus_cell(cell, false);
        let original: Value = page.value(cell.row, cell.col).map(str::to_owned);
        let column = &page.columns()[cell.col];
        let target = ValueTarget {
            column: column.name.clone(),
            ty: column.ty.clone(),
            row: page.row_number(cell.row),
            value: original.clone(),
            json: page.kinds[cell.col] == ValueKind::Json,
        };
        let editor = cx.new(|cx| ValueEditor::new(target, window, cx));
        let events = cx.subscribe_in(
            &editor,
            window,
            |this, _, event: &ValueEditorEvent, window, cx| match event {
                ValueEditorEvent::Apply(value) => this.apply_value(value.clone(), window, cx),
                ValueEditorEvent::Cancel => this.close_overlay(window, cx),
            },
        );
        editor.update(cx, |e, cx| e.focus(window, cx));
        self.overlay = Some(Overlay::Value {
            editor,
            row,
            col: cell.col,
            original,
            _events: events,
        });
        cx.notify();
        true
    }

    /// The value editor's Apply / ⌘↵: stages the value when it changed.
    fn apply_value(&mut self, value: Value, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(Overlay::Value {
            row, col, original, ..
        }) = &self.overlay
        {
            let (row, col, changed) = (*row, *col, &value != original);
            if changed && self.can_edit_any() {
                self.edit_pending(cx, |page| {
                    if let Some(view) = page.view_of(row) {
                        page.stage(super::selection::Cell::new(view, col), value);
                    }
                });
            }
        }
        self.close_overlay(window, cx);
    }

    pub(super) fn close_overlay(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.overlay.take().is_some() {
            window.focus(&self.focus_handle, cx);
            cx.notify();
        }
    }

    // ── Context menu ──────────────────────────────────────────────────────

    /// What the right-click menu offers now (None without a page).
    pub(super) fn menu_model(&self) -> Option<MenuModel> {
        let page = self.page.as_ref()?;
        let has_shape = page.selected_columns().is_some();
        let target = self.copy_target();
        // DataGrid.tsx: `why = shape && unavailable(format, shape)`.
        let formats = CopyFormat::ALL
            .into_iter()
            .map(|f| {
                let why = has_shape
                    .then(|| page.copy_unavailable(f, target))
                    .flatten();
                (f, !has_shape || why.is_some(), why)
            })
            .collect();
        let editing = matches!(self.editability, Editability::Editable { .. }).then(|| {
            let active = page.selection.active();
            let mark = active.and_then(|c| page.row_mark(c.row));
            EditMenu {
                no_cell: active.is_none() || mark == Some(RowMark::Deleted),
                cell_edited: active.is_some_and(|c| page.cell_edited(c)),
                row_changed: mark.is_some(),
            }
        });
        Some(MenuModel {
            has_shape,
            formats,
            editing,
        })
    }
}

/// The edit part of the menu (`editMenu`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct EditMenu {
    /// No active cell, or its row is deleted.
    no_cell: bool,
    cell_edited: bool,
    row_changed: bool,
}

/// The right-click menu's state (DataGrid's `copyMenu` + `editMenu`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MenuModel {
    /// Something is selected.
    pub has_shape: bool,
    /// Every "Copy as" format, whether it is disabled, and why (its
    /// tooltip; only with something selected, as the source's `title`).
    pub formats: Vec<(CopyFormat, bool, Option<&'static str>)>,
    editing: Option<EditMenu>,
}

impl MenuModel {
    /// No edit items (read-only content).
    #[cfg(test)]
    pub fn editing_is_none(&self) -> bool {
        self.editing.is_none()
    }

    pub(super) fn build(
        self,
        menu: PopupMenu,
        window: &mut Window,
        cx: &mut Context<PopupMenu>,
    ) -> PopupMenu {
        let formats = self.formats.clone();
        let menu =
            menu.min_w(px(220.))
                .menu_with_disabled("Copy", Box::new(super::Copy), !self.has_shape);
        // The trigger is disabled with nothing selected (`ContextMenuSubTrigger
        // disabled={!shape}`): a row that looks like it and never opens.
        let mut menu = if self.has_shape {
            menu.submenu("Copy as", window, cx, move |mut sub, _, _| {
                sub = sub.min_w(px(190.));
                for &(format, disabled, why) in &formats {
                    sub = match why {
                        Some(why) => sub.item(disabled_with_reason(format, why)),
                        None => sub.menu_with_disabled(
                            format.label(),
                            Box::new(CopyAs(format)),
                            disabled,
                        ),
                    };
                }
                sub
            })
        } else {
            menu.item(disabled_submenu_trigger("Copy as"))
        };
        let Some(e) = self.editing else { return menu };
        menu = menu
            .separator()
            .menu_with_disabled("Edit", Box::new(super::Edit), e.no_cell)
            .menu_with_disabled("Open in editor", Box::new(super::EditLarge), e.no_cell)
            .menu_with_disabled("Set NULL", Box::new(super::SetNull), e.no_cell)
            .separator()
            .menu("Add row", Box::new(crate::actions::AddRow))
            .menu_with_disabled("Delete row", Box::new(super::DeleteRow), e.no_cell)
            .separator()
            .menu_with_disabled("Revert cell", Box::new(super::RevertCell), !e.cell_edited)
            .item(revert_row_item(!e.row_changed))
            .separator()
            .menu("Preview and submit…", Box::new(crate::actions::SubmitEdits));
        menu
    }
}

/// The key "Revert row" shows (DataGrid.tsx: `${MOD}${ALT}Z`). That key is
/// bound to [`super::Revert`] (the cell if edited, else the row), as in the
/// source, so the menu can't take the hint from a binding of `RevertRow`.
pub const REVERT_ROW_HINT: &str = "cmd-alt-z";

/// "Revert row" with the ⌘⌥Z hint, styled as the stock items' hints.
fn revert_row_item(disabled: bool) -> PopupMenuItem {
    PopupMenuItem::element(|_, _| {
        let key = Keystroke::parse(REVERT_ROW_HINT).expect("a valid keystroke");
        div()
            .flex_1()
            .flex()
            .items_center()
            .justify_between()
            .gap_3()
            .child("Revert row")
            .child(
                Kbd::new(key)
                    .p_0()
                    .flex_nowrap()
                    .border_0()
                    .bg(gpui::transparent_white()),
            )
    })
    .action(Box::new(super::RevertRow))
    .disabled(disabled)
}

/// A "Copy as" format that can't copy the selection, with the reason as its
/// tooltip (DataGrid.tsx `title={why}`).
fn disabled_with_reason(format: CopyFormat, why: &'static str) -> PopupMenuItem {
    PopupMenuItem::element(move |_, _| {
        div()
            .id(format.label())
            .flex_1()
            .child(format.label())
            .tooltip(crate::views::chrome::tooltip(why))
    })
    .action(Box::new(CopyAs(format)))
    .disabled(true)
}

/// A disabled submenu trigger: the label and the stock chevron.
fn disabled_submenu_trigger(label: &'static str) -> PopupMenuItem {
    PopupMenuItem::element(move |_, cx| {
        div()
            .flex_1()
            .flex()
            .items_center()
            .justify_between()
            .gap_2()
            .child(label)
            .child(
                Icon::new(gpui_component::IconName::ChevronRight)
                    .xsmall()
                    .text_color(cx.theme().muted_foreground),
            )
    })
    .action(Box::new(gpui::NoAction))
    .disabled(true)
}
