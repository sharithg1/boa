//! The SQL editor pane: gpui-component's code editor wrapped in our
//! [`SqlEditor`], with the Tauri console's behaviour on top (Workspace.tsx
//! `QueryView`):
//!
//! - highlighting from the ported tokenizer (`highlight`), schema / table /
//!   column names coloured once the catalog is set ([`SqlEditor::set_schemas`]);
//! - **arm-then-run** (⌘↵): the selection runs at once; otherwise the first
//!   ⌘↵ arms (fills) the statement under the caret and, when the caret is in a
//!   subquery, offers the enclosing statements in a chooser (↑ ↓, Enter, Esc);
//!   Enter or ⌘↵ again runs the armed one by emitting [`RunStatement`]. A
//!   caret move, an edit or losing focus disarms. The toolbar's Run
//!   ([`SqlEditor::run_or_arm`] with `direct`) skips arming when there is
//!   nothing to choose between;
//! - our own **completion popup** (`crate::views::completion_popup`), drawn
//!   at the word's start from the editor's layout: typing a name character,
//!   `.` or `"` opens it, Ctrl+Space asks explicitly (and stays explicit
//!   while erasing), ↑ ↓ PgUp PgDn move, Enter inserts before the rest of the
//!   word, Tab replaces the whole word, Esc closes; `count(|)` puts the caret
//!   inside, a schema (`billing.`) re-opens on its tables;
//! - the statement under the caret is outlined while the editor has focus;
//! - [`SqlEditorEvent`] reports text changes and the caret (status bar).
//!
//! gpui-component's own completion menu is not used (no provider is set).

mod completion;
pub(crate) mod highlight;
mod outline;
mod statement;

pub use completion::{Accept, Move, VISIBLE_ROWS, accept, complete_at, is_trigger, moved};
pub use highlight::{SharedCatalog, SqlHighlighter, style_runs, token_style};
pub use statement::{sql_to_run, statement_on, statements_on};

use std::cell::RefCell;
use std::ops::Range;
use std::rc::Rc;
use std::sync::Arc;

use boa_core::sql::{Catalog, CompletionResult, Schema, Span};
use gpui::{
    App, AppContext as _, Context, Entity, EntityInputHandler as _, EventEmitter, FocusHandle,
    Focusable, InteractiveElement as _, IntoElement, ParentElement as _, Pixels, Render,
    ScrollHandle, SharedString, Styled as _, Subscription, Window, actions, div, px,
};
use gpui_component::input::{
    Editor, EditorState, GutterStyle, IndentInline, InputEditsEvent, InputEvent, TabSize,
};

use crate::actions::{
    Confirm, Dismiss, RunOrArm, SelectNext, SelectPageDown, SelectPageUp, SelectPrevious,
};
use crate::theme::{self, ActivePalette as _, Palette};

actions!(
    sql_editor,
    [
        /// Tab: accept the open completion (replacing the whole word), else indent.
        AcceptCompletionOrIndent,
        /// Ctrl+Space: explicit completion (lists items for an empty prefix,
        /// e.g. the foreign-key conditions after `JOIN t ON`).
        TriggerCompletion,
    ]
);

/// Key context of the editor pane (around gpui-component's `Input` context).
pub const KEY_CONTEXT: &str = "SqlEditor";
/// Added to the pane's context while the completion popup is open (its keys
/// are bound in `SqlCompletion > Input`).
pub const COMPLETION_CONTEXT: &str = "SqlCompletion";
/// Added while a statement is armed (`StatementChooser > Input`).
pub const CHOOSER_CONTEXT: &str = "StatementChooser";

/// A demo script (dev runs and tests): a few statements against the local
/// `customers` / `orders` tables.
pub const SAMPLE_SQL: &str = "\
-- Boa SQL console
-- Cmd+Enter arms the statement under the caret, Enter (or Cmd+Enter) runs it.
-- Ctrl+Space completes; after JOIN t ON it offers the foreign-key conditions.

select id, name, email, country, created_at
from public.customers
order by created_at desc
limit 100;

select c.name, count(*) as orders, sum(o.total) as revenue
from public.orders o
join public.customers c on c.id = o.customer_id
group by c.name
order by revenue desc
limit 50;

-- every order: 100k rows, paged by Settings → row limit
select * from public.orders;

select status, count(*), avg(total)::numeric(12, 2) as avg_total
from public.orders
where placed_at > now() - interval '30 days'
  and payload ->> 'channel' = 'web'
group by status;
";

/// Registers the editor's key bindings (all of Boa's live in
/// `crate::keymap`). Call once at startup, after `gpui_component::init`.
pub fn init(cx: &mut App) {
    crate::keymap::init(cx);
}

/// Emitted when the user runs SQL: the selection, or the armed statement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunStatement {
    pub sql: String,
}

impl EventEmitter<RunStatement> for SqlEditor {}

/// The caret, 1-based (store.ts `cursor`: the column counts UTF-16 units, as JS did).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CaretPos {
    pub line: usize,
    pub col: usize,
}

impl std::fmt::Display for CaretPos {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{}", self.line, self.col)
    }
}

/// What else the editor reports.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SqlEditorEvent {
    /// The text changed (read it with [`SqlEditor::text`]).
    Changed,
    /// The caret moved; None when the editor lost focus or has a selection.
    Caret(Option<CaretPos>),
}

impl EventEmitter<SqlEditorEvent> for SqlEditor {}

/// The open completion list.
#[derive(Debug, Clone)]
pub struct OpenCompletion {
    pub result: CompletionResult,
    pub selected: usize,
    /// Opened with Ctrl+Space: stays open as the prefix is erased.
    pub explicit: bool,
}

/// The armed statement (and its enclosing ones, outermost first).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Armed {
    pub items: Vec<Span>,
    pub selected: usize,
    /// The caret when it was armed.
    pub at: usize,
}

impl Armed {
    pub fn range(&self) -> Range<usize> {
        let s = self.items[self.selected];
        s.from..s.to
    }
}

/// The SQL editor pane.
pub struct SqlEditor {
    state: Entity<EditorState>,
    schemas: Arc<Vec<Schema>>,
    catalog: SharedCatalog,
    palette: Palette,
    default_schema: Option<String>,
    /// The statement under the caret (focused, no selection).
    outlined: Option<Range<usize>>,
    /// What is drawn: (range, armed).
    drawn: Option<(Range<usize>, bool)>,
    completion: Option<OpenCompletion>,
    armed: Option<Armed>,
    /// The text inserted by accepting a completion: that edit doesn't reopen the list.
    accepting: Option<String>,
    caret: Option<CaretPos>,
    /// The text the last caret check saw.
    last_text: SharedString,
    list_scroll: ScrollHandle,
    _subscriptions: Vec<Subscription>,
}

impl std::fmt::Debug for SqlEditor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SqlEditor")
            .field("schemas", &self.schemas.len())
            .field("default_schema", &self.default_schema)
            .field("outlined", &self.outlined)
            .field("armed", &self.armed)
            .field(
                "completion",
                &self.completion.as_ref().map(|c| c.result.items.len()),
            )
            .finish_non_exhaustive()
    }
}

/// UTF-16 offsets of a byte range (the input handler's unit).
fn utf16_range(text: &str, range: Range<usize>) -> Range<usize> {
    let start = text
        .get(..range.start)
        .map_or(0, |s| s.encode_utf16().count());
    let len = text
        .get(range.start..range.end)
        .map_or(0, |s| s.encode_utf16().count());
    start..start + len
}

/// The 1-based caret position of byte `offset`.
pub fn caret_pos(text: &str, offset: usize) -> CaretPos {
    let before = text.get(..offset.min(text.len())).unwrap_or(text);
    let line_start = before.rfind('\n').map_or(0, |i| i + 1);
    CaretPos {
        line: before.matches('\n').count() + 1,
        col: before[line_start..].encode_utf16().count() + 1,
    }
}

impl SqlEditor {
    /// An editor on [`SAMPLE_SQL`], caret on its first statement (dev runs, tests).
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let first = SAMPLE_SQL.find("\nselect").map_or(0, |i| i + 1);
        let this = Self::with_text(SAMPLE_SQL, window, cx);
        this.state
            .update(cx, |state, cx| state.set_selected_range(first..first, cx));
        this
    }

    /// An editor on `text` (a console's SQL), caret at the start.
    pub fn with_text(
        text: impl Into<SharedString>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let palette = cx
            .try_global::<Palette>()
            .cloned()
            .unwrap_or_else(Palette::dark);
        let catalog: SharedCatalog = Rc::new(RefCell::new(None));
        let text: SharedString = text.into();
        let state = cx.new(|cx| {
            let mut state = EditorState::new(window, cx)
                .line_number(true)
                .soft_wrap(false)
                .folding(false)
                .tab_size(TabSize {
                    tab_size: 4,
                    hard_tabs: false,
                })
                .default_value(text);
            state.set_highlighter_factory(highlight::factory(catalog.clone(), palette.clone()), cx);
            state
        });
        let subscriptions = vec![
            cx.subscribe_in(&state, window, Self::on_input_event),
            cx.subscribe_in(&state, window, Self::on_edits),
            // Caret moves notify the state without an event.
            cx.observe_in(&state, window, |this, _, window, cx| {
                this.on_selection(window, cx)
            }),
        ];
        // The window focuses the workspace right after building it; take
        // the focus once that has happened (React's `autoFocus`).
        let focus = state.clone();
        window.defer(cx, move |window, cx| {
            focus.update(cx, |state, cx| state.focus(window, cx));
        });
        Self {
            state,
            schemas: Arc::new(Vec::new()),
            catalog,
            palette,
            default_schema: None,
            outlined: None,
            drawn: None,
            completion: None,
            armed: None,
            accepting: None,
            caret: None,
            last_text: SharedString::default(),
            list_scroll: ScrollHandle::new(),
            _subscriptions: subscriptions,
        }
    }

    /// The editor state (gpui-component's), for callers that need more.
    pub fn state(&self) -> &Entity<EditorState> {
        &self.state
    }

    /// The SQL in the editor.
    pub fn text(&self, cx: &App) -> SharedString {
        self.state.read(cx).value()
    }

    /// Replaces the SQL (undo history cleared), caret at the start.
    pub fn set_text(
        &mut self,
        text: impl Into<SharedString>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let text: SharedString = text.into();
        self.state
            .update(cx, |state, cx| state.set_value(text, window, cx));
    }

    /// The byte range of the outlined statement (the one under the caret,
    /// while the editor has focus and nothing is selected).
    pub fn outlined_statement(&self) -> Option<Range<usize>> {
        self.outlined.clone()
    }

    /// The armed statement and its alternatives, if any.
    pub fn armed(&self) -> Option<&Armed> {
        self.armed.as_ref()
    }

    /// The open completion list, if any.
    pub fn completion(&self) -> Option<&OpenCompletion> {
        self.completion.as_ref()
    }

    /// The caret (1-based), while focused without a selection.
    pub fn caret(&self) -> Option<CaretPos> {
        self.caret
    }

    /// The schemas completion and highlighting use.
    pub fn schemas(&self) -> &Arc<Vec<Schema>> {
        &self.schemas
    }

    /// Uses `schemas` (the database's catalog) for completion and highlighting.
    pub fn set_schemas(&mut self, schemas: impl Into<Arc<Vec<Schema>>>, cx: &mut Context<Self>) {
        let schemas = schemas.into();
        if Arc::ptr_eq(&schemas, &self.schemas) {
            return;
        }
        *self.catalog.borrow_mut() = Some(Catalog::from_schemas(&schemas));
        self.schemas = schemas;
        // A new highlighter re-tokenizes with the catalog.
        let factory = highlight::factory(self.catalog.clone(), self.palette.clone());
        self.state
            .update(cx, |state, cx| state.set_highlighter_factory(factory, cx));
        cx.notify();
    }

    /// The console's default schema (completion ranks it first).
    pub fn set_default_schema(&mut self, schema: Option<String>) {
        self.default_schema = schema;
    }

    fn default_schema(&self) -> Option<&str> {
        self.default_schema
            .as_deref()
            .or_else(|| self.schemas.first().map(|s| s.name.as_str()))
    }

    /// Runs the selection, or the statement under the caret, at once (no
    /// arming). Emits [`RunStatement`] and returns the SQL; None on a blank
    /// line between statements.
    pub fn run(&mut self, cx: &mut Context<Self>) -> Option<String> {
        let state = self.state.read(cx);
        let sql = sql_to_run(&state.value(), state.selected_range())?;
        cx.emit(RunStatement { sql: sql.clone() });
        Some(sql)
    }

    /// ⌘↵ (Workspace.tsx `run`): runs the armed statement if there is one,
    /// else the selection; otherwise arms the statement under the caret
    /// (offering its enclosing statements when inside a subquery). `direct`
    /// (the toolbar button) runs at once when there is nothing to choose.
    pub fn run_or_arm(&mut self, direct: bool, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(armed) = self.armed.clone() {
            self.pick_statement(armed.selected, cx);
            return;
        }
        let (text, selection) = {
            let state = self.state.read(cx);
            (state.value(), state.selected_range())
        };
        let selected = text.get(selection.clone()).unwrap_or_default();
        if !selected.trim().is_empty() {
            cx.emit(RunStatement {
                sql: selected.to_owned(),
            });
            return;
        }
        let items = statements_on(&text, selection.start);
        if items.is_empty() {
            return;
        }
        if direct && items.len() == 1 {
            cx.emit(RunStatement {
                sql: items[0].text(&text).to_owned(),
            });
            return;
        }
        self.completion = None;
        // From the toolbar button: keys must reach the editor.
        self.state.update(cx, |state, cx| state.focus(window, cx));
        self.armed = Some(Armed {
            items,
            selected: 0,
            at: selection.start,
        });
        self.refresh_marks(window, cx);
        cx.notify();
    }

    /// Runs the armed statement `i` (Enter, a click in the chooser).
    pub fn pick_statement(&mut self, i: usize, cx: &mut Context<Self>) {
        let Some(armed) = self.armed.take() else {
            return;
        };
        cx.notify();
        if let Some(span) = armed.items.get(i) {
            let text = self.text(cx);
            cx.emit(RunStatement {
                sql: span.text(&text).to_owned(),
            });
        }
    }

    /// Highlights another armed statement (hover in the chooser).
    pub fn hover_statement(&mut self, i: usize, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(armed) = &mut self.armed
            && i < armed.items.len()
            && armed.selected != i
        {
            armed.selected = i;
            self.refresh_marks(window, cx);
            cx.notify();
        }
    }

    /// Opens the list at the caret (`explicit`: Ctrl+Space).
    pub fn suggest(&mut self, explicit: bool, cx: &mut Context<Self>) {
        let cursor = self.state.read(cx).cursor();
        self.suggest_at(cursor, explicit, cx);
    }

    fn suggest_at(&mut self, pos: usize, explicit: bool, cx: &mut Context<Self>) {
        let text = self.text(cx);
        self.completion = complete_at(&text, pos, &self.schemas, self.default_schema(), explicit)
            .map(|result| OpenCompletion {
                result,
                selected: 0,
                explicit,
            });
        self.list_scroll.scroll_to_item(0);
        cx.notify();
    }

    /// Accepts item `i`: Enter (`whole` false) replaces the word up to the
    /// caret, Tab (`whole`) the whole word.
    pub fn accept_completion(
        &mut self,
        i: usize,
        whole: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(open) = self.completion.take() else {
            return;
        };
        let Some(item) = open.result.items.get(i) else {
            return;
        };
        let cursor = self.state.read(cx).cursor();
        let a = accept(&open.result, item, cursor, whole);
        self.accepting = Some(a.insert.clone());
        self.state.update(cx, |state, cx| {
            let text = state.value();
            let range = utf16_range(&text, a.range.clone());
            state.replace_text_in_range(Some(range), &a.insert, window, cx);
            state.set_selected_range(a.caret..a.caret, cx);
        });
        if a.reopen {
            self.suggest_at(a.caret, false, cx);
        }
        cx.notify();
    }

    /// Hover in the list.
    pub fn hover_completion(&mut self, i: usize, cx: &mut Context<Self>) {
        if let Some(open) = &mut self.completion
            && i < open.result.items.len()
            && open.selected != i
        {
            open.selected = i;
            cx.notify();
        }
    }

    /// Types `text` at the caret as if from the keyboard (so completion
    /// opens), with the editor focused. For scripted screenshots.
    pub fn type_text(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.state.update(cx, |state, cx| {
            state.focus(window, cx);
            state.replace_text_in_range(None, text, window, cx);
        });
    }

    /// Types `text` at the end of the buffer as if from the keyboard (so
    /// completion opens), with the editor focused. For scripted screenshots.
    pub fn type_at_end(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.state.update(cx, |state, cx| {
            state.focus(window, cx);
            let end = state.text().len();
            state.set_selected_range(end..end, cx);
            state.replace_text_in_range(None, text, window, cx);
        });
    }

    fn on_input_event(
        &mut self,
        _: &Entity<EditorState>,
        event: &InputEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            InputEvent::Blur => {
                // Workspace.tsx onBlur: close the list and the chooser.
                self.completion = None;
                self.armed = None;
                self.on_selection(window, cx);
                cx.notify();
            }
            InputEvent::Focus => self.on_selection(window, cx),
            _ => {}
        }
    }

    /// Typing a name character, `.` or `"` (re)opens the list; erasing
    /// follows it while open; any other edit closes it. Edits disarm.
    fn on_edits(
        &mut self,
        _: &Entity<EditorState>,
        event: &InputEditsEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        cx.emit(SqlEditorEvent::Changed);
        if self.armed.take().is_some() {
            self.refresh_marks(window, cx);
        }
        if self.accepting.take().is_some() {
            return;
        }
        let (typed, erased) = match event.edits() {
            [(edit, text)] => {
                let inserted =
                    edit.new_end_byte > edit.start_byte && edit.old_end_byte == edit.start_byte;
                let deleted =
                    edit.new_end_byte == edit.start_byte && edit.old_end_byte > edit.start_byte;
                let typed = inserted && edit.new_end_byte <= text.len() && {
                    let s = text.slice(edit.start_byte..edit.new_end_byte).to_string();
                    !s.contains('\n') && s.chars().count() <= 2 && is_trigger(&s)
                };
                (typed, deleted && self.completion.is_some())
            }
            _ => (false, false),
        };
        if typed || erased {
            let explicit = self.completion.as_ref().is_some_and(|c| c.explicit);
            self.suggest(explicit, cx);
        } else if self.completion.take().is_some() {
            cx.notify();
        }
    }

    /// Caret moves: the armed statement goes when the caret leaves where it
    /// was armed, the list when the caret leaves the word; the outline and
    /// the status bar's caret follow.
    fn on_selection(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (selection, focused, text) = {
            let state = self.state.read(cx);
            (
                state.selected_range(),
                state.focus_handle(cx).is_focused(window),
                state.value(),
            )
        };
        if let Some(armed) = &self.armed
            && (!selection.is_empty() || selection.start != armed.at)
        {
            self.armed = None;
            cx.notify();
        }
        // The state notifies before it reports an edit: when the text
        // changed, the edit (`on_edits`) decides about the list.
        let edited = text != self.last_text;
        if edited {
            self.last_text = text.clone();
        }
        if let Some(open) = &self.completion
            && !edited
            && (!selection.is_empty()
                || selection.start < open.result.from
                || selection.start > open.result.to)
        {
            self.completion = None;
            cx.notify();
        }
        let caret = (focused && selection.is_empty()).then(|| caret_pos(&text, selection.start));
        if caret != self.caret {
            self.caret = caret;
            cx.emit(SqlEditorEvent::Caret(caret));
        }
        self.refresh_marks(window, cx);
    }

    // SqlHighlight.tsx: armed fill-status-ok/15 stroke-status-ok/70, else
    // fill-none stroke-status-ok/45.
    fn fill_color(p: &Palette) -> gpui::Hsla {
        p.status_ok.opacity(0.15)
    }

    fn stroke_color(p: &Palette, armed: bool) -> gpui::Hsla {
        p.status_ok.opacity(if armed { 0.7 } else { 0.45 })
    }

    /// Fills the armed statement, else outlines the one under the caret.
    fn refresh_marks(&mut self, window: &Window, cx: &mut Context<Self>) {
        let state = self.state.read(cx);
        let selection = state.selected_range();
        let focused = state.focus_handle(cx).is_focused(window);
        self.outlined = if focused && selection.is_empty() {
            statement_on(&state.value(), selection.start)
        } else {
            None
        };
        let drawn = match &self.armed {
            Some(armed) => Some((armed.range(), true)),
            None => self.outlined.clone().map(|r| (r, false)),
        };
        if drawn == self.drawn {
            return;
        }
        // Painted in `render` (`outline`), not as editor decorations: those
        // are clipped at column 0 and can't take DataGrip's box shape.
        self.drawn = drawn;
        cx.notify();
    }

    // ── Keys (bound in `SqlEditor > Input`, `SqlCompletion > Input`,
    // `StatementChooser > Input`; see crate::keymap) ───────────────────────

    fn accept_completion_or_indent(
        &mut self,
        _: &AcceptCompletionOrIndent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(open) = &self.completion {
            let selected = open.selected;
            self.accept_completion(selected, true, window, cx);
        } else {
            window.dispatch_action(Box::new(IndentInline), cx);
        }
    }

    fn trigger_completion(
        &mut self,
        _: &TriggerCompletion,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.suggest(true, cx);
    }

    fn run_or_arm_action(&mut self, _: &RunOrArm, window: &mut Window, cx: &mut Context<Self>) {
        self.run_or_arm(false, window, cx);
    }

    fn move_selection(&mut self, step: Move, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(armed) = &mut self.armed {
            let n = armed.items.len();
            if n > 1 && matches!(step, Move::Up | Move::Down) {
                armed.selected = moved(armed.selected, n, step);
                self.refresh_marks(window, cx);
                cx.notify();
                return;
            }
        }
        if let Some(open) = &mut self.completion {
            open.selected = moved(open.selected, open.result.items.len(), step);
            self.list_scroll.scroll_to_item(open.selected);
            cx.notify();
            return;
        }
        // Not ours: the key moves the caret (the next binding).
        cx.propagate();
    }

    fn confirm(&mut self, _: &Confirm, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(armed) = &self.armed {
            let selected = armed.selected;
            self.pick_statement(selected, cx);
        } else if let Some(open) = &self.completion {
            let selected = open.selected;
            self.accept_completion(selected, false, window, cx);
        } else {
            cx.propagate();
        }
    }

    fn dismiss(&mut self, _: &Dismiss, window: &mut Window, cx: &mut Context<Self>) {
        if self.armed.take().is_some() {
            self.refresh_marks(window, cx);
            cx.notify();
        } else if self.completion.take().is_some() {
            cx.notify();
        } else {
            cx.propagate();
        }
    }

    fn render_overlays(&self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let state = self.state.read(cx);
        let area = state.input_bounds();
        if let Some(armed) = self.armed.as_ref().filter(|a| a.items.len() > 1) {
            let anchor = state.range_to_bounds(&(armed.at..armed.at))?;
            let text = state.value();
            let this = cx.entity().downgrade();
            let hover = cx.entity().downgrade();
            return Some(crate::views::statement_chooser::statement_chooser(
                &text,
                &armed.items,
                armed.selected,
                anchor,
                area,
                Rc::new(move |i, _, cx| {
                    this.update(cx, |this, cx| this.pick_statement(i, cx)).ok();
                }),
                Rc::new(move |i, window, cx| {
                    hover
                        .update(cx, |this, cx| this.hover_statement(i, window, cx))
                        .ok();
                }),
                cx,
            ));
        }
        let open = self.completion.as_ref()?;
        let anchor = state.range_to_bounds(&(open.result.from..open.result.from))?;
        let this = cx.entity().downgrade();
        let hover = cx.entity().downgrade();
        Some(crate::views::completion_popup::completion_popup(
            &open.result.items,
            open.selected,
            anchor,
            area,
            &self.list_scroll,
            Rc::new(move |i, window, cx| {
                this.update(cx, |this, cx| this.accept_completion(i, false, window, cx))
                    .ok();
            }),
            Rc::new(move |i, _, cx| {
                hover
                    .update(cx, |this, cx| this.hover_completion(i, cx))
                    .ok();
            }),
            cx,
        ))
    }
}

/// The gutter (Workspace.tsx: `w-14 border-r border-editor-divider pr-5`,
/// the text `pl-3`): 56 px wide with its 1 px divider, the line numbers
/// right-aligned 20 px before the divider, the text 12 px after it.
pub const GUTTER_WIDTH: Pixels = px(56.);
/// Where the line numbers end (56 − 1 − 20).
pub const NUMBERS_RIGHT: Pixels = px(35.);
/// Space between the divider and column 0.
pub const TEXT_PAD_LEFT: Pixels = px(12.);

/// gpui-base starts the line-number column this far right of the editor's
/// left padding (its `LINE_NUMBER_RIGHT_MARGIN`, measured on screen).
const NUMBERS_INSET: Pixels = px(6.);

/// The editor's left padding that puts the right edge of three digits (the
/// editor reserves three for up to 999 lines) at [`NUMBERS_RIGHT`].
fn gutter_pad_left(window: &Window) -> Pixels {
    let text = window.text_system();
    let font = text.resolve_font(&gpui::font(theme::MONO_FONT));
    let digit = "0123456789"
        .chars()
        .map(|d| text.layout_width(font, theme::text::SM, d))
        .fold(px(0.), Pixels::max);
    (NUMBERS_RIGHT - NUMBERS_INSET - digit * 3.).max(px(0.))
}

/// The 1 px divider between the line numbers and the text, from the
/// editor's last layout (the text area starts [`TEXT_PAD_LEFT`] after it).
fn gutter_divider(state: Entity<EditorState>, color: gpui::Hsla) -> impl IntoElement {
    gpui::canvas(
        |_, _, _| {},
        move |bounds, (), window: &mut Window, cx| {
            let state = state.read(cx);
            let Some(offset) = state.text_origin_offset() else {
                return;
            };
            let x = state.input_bounds().left() + offset - TEXT_PAD_LEFT - px(1.);
            window.paint_quad(gpui::fill(
                gpui::Bounds::new(
                    gpui::point(x, bounds.top()),
                    gpui::size(px(1.), bounds.size.height),
                ),
                color,
            ));
        },
    )
    .absolute()
    .inset_0()
}

impl Focusable for SqlEditor {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.state.read(cx).focus_handle(cx)
    }
}

impl Render for SqlEditor {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = cx.palette().clone();
        let mut context = KEY_CONTEXT.to_owned();
        if self.completion.is_some() {
            context.push(' ');
            context.push_str(COMPLETION_CONTEXT);
        }
        if self.armed.is_some() {
            context.push(' ');
            context.push_str(CHOOSER_CONTEXT);
        }
        let overlays = self.render_overlays(cx);
        let frame = self.drawn.clone().map(|(range, armed)| {
            outline::outline(
                self.state.clone(),
                range,
                Self::stroke_color(&self.palette, armed),
                armed.then(|| Self::fill_color(&self.palette)),
            )
        });
        div()
            .id("sql-editor")
            .key_context(context.as_str())
            .on_action(cx.listener(Self::accept_completion_or_indent))
            .on_action(cx.listener(Self::trigger_completion))
            .on_action(cx.listener(Self::run_or_arm_action))
            .on_action(cx.listener(|this, _: &SelectPrevious, window, cx| {
                this.move_selection(Move::Up, window, cx)
            }))
            .on_action(cx.listener(|this, _: &SelectNext, window, cx| {
                this.move_selection(Move::Down, window, cx)
            }))
            .on_action(cx.listener(|this, _: &SelectPageUp, window, cx| {
                this.move_selection(Move::PageUp, window, cx)
            }))
            .on_action(cx.listener(|this, _: &SelectPageDown, window, cx| {
                this.move_selection(Move::PageDown, window, cx)
            }))
            .on_action(cx.listener(Self::confirm))
            .on_action(cx.listener(Self::dismiss))
            .size_full()
            .relative()
            .bg(p.editor)
            .child(
                // Absolute, so its height is definite and scrolling to the caret is exact.
                div().absolute().inset_0().child(
                    Editor::new(&self.state)
                        .bordered(false)
                        .size_full()
                        .rounded(px(0.))
                        .font_family(theme::MONO_FONT)
                        .text_size(theme::text::SM)
                        .line_height(theme::text::SM_LINE)
                        // Workspace.tsx's gutter: the numbers end `pr-5` before
                        // the divider, the text starts `pl-3` after it.
                        .pl(gutter_pad_left(window))
                        .gutter_style(
                            GutterStyle::default()
                                .with_gap(GUTTER_WIDTH + TEXT_PAD_LEFT - NUMBERS_RIGHT)
                                .with_line_number_color(p.editor_gutter)
                                .with_active_line_number_color(p.editor_gutter_active),
                        ),
                ),
            )
            .child(gutter_divider(self.state.clone(), p.editor_divider))
            .children(frame)
            .children(overlays)
    }
}

#[cfg(test)]
mod tests;
