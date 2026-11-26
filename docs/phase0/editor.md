# Phase 0: the SQL editor

Code: `crates/boa-app/src/editor/` (`mod.rs` the view, `highlight.rs`,
`completion.rs`, `statement.rs`, `tests.rs`). Screenshots:
[editor.png](editor.png) (sample script, highlighting, the outlined statement
under the caret) and [editor-completion.png](editor-completion.png) (the
completion menu after `o.`, catalog from the live database).

**Verdict: gpui-component's `Editor` is good enough for the console.** Editing,
undo, selection, IME, scrolling and the gutter came for free; highlighting and
completion plugged into its seams with ~400 lines of glue over the ported
`boa_core::sql`. The weak spot is the completion *menu*: it is theirs and
closed (see below). Matching the Tauri popup exactly means drawing our own
popup, which is feasible (the editor exposes caret geometry) but not done here.

## What's built

| Piece | How |
|---|---|
| Editor | `EditorState` (code-editor mode) + `Editor` element wrapped in `SqlEditor`. No soft wrap, line numbers, tab = 4 spaces, Geist Mono 13/20 |
| Highlighting | `SqlHighlighter: InputHighlighter`: re-tokenizes the whole buffer with `boa_core::sql::tokenize_into` on each edit, `styles()` maps token kinds to `Palette::sql_*` (`SqlHighlight.tsx` `CLASS`). Schema / table / column names are coloured once the catalog is in (the highlighter factory is reinstalled with it) |
| Editor colours | `theme.rs` puts `--editor`, `--editor-line` (caret-line band), `--editor-gutter(-active)` into gpui-component's `highlight_theme`, the only place its Editor reads them from |
| Catalog | On `SpikeConnection` Connected: `list_schemas` + `introspect` (all schemas). Status in the toolbar ("1 schema, 2 tables") |
| Completion | `SqlCompletions: CompletionProvider` calls `boa_core::sql::complete` (default schema `public`, `explicit` when invoked) and returns LSP items whose `text_edit` replaces the typed prefix. Opens on a name character, `.` or `"` (the Tauri regex); **Ctrl+Space** (bound by us; gpui-component has no default) asks explicitly, so `JOIN t ON |` lists the FK conditions first. Enter or Tab accept, Esc closes, ↑/↓ move |
| ⌘↵ | gpui-component maps ⌘↵ to `Enter { secondary }` and emits `PressEnter { secondary: true }` without inserting a newline; we emit `RunStatement` with the selection if it isn't blank, else the statement under the caret (`statements_at`, minus the blank-line rule of `Workspace.tsx` `onStatement`). Nothing on a blank line between statements |
| Statement outline | Our own overlay (`editor/outline.rs`, status-ok at 45 %; armed: 70 % frame, 15 % fill) around the statement under the caret while focused with no selection. It starts 3 px left of column 0, so the caret is inside, and steps line by line around each line's own text, as the Tauri app's `rangePath` does (a blank line is one cell wide). Not a `RangeDecoration` frame: the editor clips those at column 0 |
| Toolbar | Minimal: Run (same as ⌘↵), catalog status, `database.schema` |
| Demo | `SAMPLE_SQL`: four statements on customers / orders, including `select * from public.orders` (100k rows). Caret starts on the first statement |

Public API (for the integrator): `SqlEditor::new(window, cx)`, event
`RunStatement { sql }`, `run(cx)`, `text(cx)`, `set_text(..)`,
`set_schemas(Vec<Schema>, cx)`, `catalog_status()`, `outlined_statement()`,
`state()` (the gpui-component `Entity<EditorState>`), `type_at_end(..)`
(scripting). `editor::init(cx)` registers the key bindings (called from `app::init`).
Pure helpers: `sql_to_run`, `statement_on`, `statements_on`, `style_runs`,
`token_style`, `completion_items`, `position_at`.

## How it was verified

| What | How |
|---|---|
| Highlight runs (colours per kind, catalog names, italics flag, exact coverage of any sub-range, stale tokens) | unit tests |
| Completion items: `select * from cu|` → customers; FK join `c.id = o.customer_id` first after `JOIN … ON` (explicit only); pk badge; LSP positions on multi-byte text | unit tests |
| Statement / selection rules | unit tests |
| ⌘↵ with the caret in the 2nd statement emits exactly it; a selection overrides; blank line runs nothing; ⌘↵ inserts nothing; the demo starts on a runnable statement | `#[gpui::test]` with real keystrokes through the keymap |
| Typing opens the menu, Tab accepts; Ctrl+Space → FK joins, Enter accepts | `#[gpui::test]` |
| Outline follows the caret, clears on a blank line | `#[gpui::test]` |
| Tab indents without a menu; ⌘Z / ⌘⇧Z | `#[gpui::test]` |
| IME: marked text then commit (`replace_and_mark_text_in_range` → `replace_text_in_range`) | `#[gpui::test]` through the input-handler API, **not** with a real input method |
| Look: colours, gutter, caret-line band, outline, completion menu against the live catalog | screenshots (`--features screenshot`, `BOA_EDITOR_TEXT` / `BOA_EDITOR_TYPE` script the text) |
| Mouse selection, scrolling feel, trackpad, real IME (Japanese / Korean), caret blink, popup positioning near window edges | **not verified** (no screen access) |

## What was painful

- **The completion menu is closed.** Rows are label + italic detail, no
  custom rendering, so the Tauri per-kind icons became a text badge in the
  detail (`column  integer · o`, `pk  bigint · o`, `join  foreign key`). The
  selected index is private, only Enter (and ↑/↓/Esc) reach it: **Tab** is
  not an accept key, so we bind Tab in `SqlEditor > Input` to an action that
  dispatches Enter when the menu is open, IndentInline otherwise. Tab and
  Enter therefore both replace *up to the caret*; the Tauri app's Tab replaced
  the whole word. No PageUp/PageDown in the menu.
- **No snippets / caret inside an item.** `Completion::caret` (`count(|)`) is
  handled by watching `InputEditsEvent` for an insert equal to a just-offered
  item and moving the caret; picking a schema (`billing.`) re-opens the menu
  for its tables via a spawned `show_completions`. Both are best effort and
  **not covered by tests**.
- Explicit mode isn't sticky: after Ctrl+Space, typing re-queries as
  "triggered", so erasing back to an empty prefix closes the list (Tauri kept
  it open).
- Menu matching highlight uses `highlight_contains` (the Tauri popup didn't
  highlight); the popup anchors at the caret, not the word start; width capped
  at 380 px like the Tauri popup but not fixed.
- Several things we rely on are `#[doc(hidden)]` in 0.11 (`soft_wrap`,
  `folding` builders, `completion_menu_state()`): upgrade risk.
- Editor colours only through the global theme (`highlight_theme`), and the
  Editor forces a 1.5 line height (we override with `.line_height(20px)`).
- Testing: focus events and the completion overlay need a drawn frame
  (`window.draw` in tests), and GPUI's `simulate_keystrokes("enter")` also
  types the "\n" when an action propagates (the menu's Enter does), so tests
  send Enter as a bare key-down like the native window.
- Scripted screenshot only: inserting multi-line text at the very end of a
  long buffer in one call left the last line half below the fold (the reveal
  seems computed before relayout). Not seen with keystroke-sized edits; worth a
  look in Phase 4.

## Gaps vs. the Tauri console (USAGE.md, `Workspace.tsx`)

- **⌘↵ arming and the statement chooser.** Tauri: ⌘↵ first *arms* (fills) the
  statement, and offers the enclosing subqueries in a "Statements" chooser;
  Enter or ⌘↵ again runs. Here ⌘↵ runs the whole statement at once
  (`statements_on` already returns the subqueries for a chooser).
- Toolbar: Execute / Cancel / History / Tx controls / default-schema picker
  (Phase 4 / 6). The default schema is fixed to `public`.
- Caret line / column for the status bar; session restore of the SQL.
- Gutter: no divider line and narrower than the Tauri `w-14 pr-5`; whether the
  caret-line band hides with a selection, as Tauri's did, is unchecked.
- Comments are not italic: only upright Geist Mono faces are bundled.
- Per-kind icons, PK key icon, Tab-replaces-whole-word in completion (see above).
- Large scripts: highlighting re-tokenizes the whole buffer and the outline
  re-reads the text on each caret move. Fine for consoles; not measured beyond
  them (gpui-component documents ~50K lines).

## Estimate to parity for this pane

About **1 to 1.5 weeks**: arming + statement chooser (1–2 days), our own
completion popup with icons / Tab semantics / sticky explicit mode, drawn from
the editor's caret geometry (2–3 days, optional if the badge is acceptable),
toolbar with default schema (1 day, on Phase 4's run / cancel plumbing),
status-bar caret position and session restore hooks (1 day), large-script
profiling and the end-of-buffer scroll check (1 day).
