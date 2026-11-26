//! Every action of the app, in one place (HANDOFF-PARITY.md §10).
//!
//! Actions are what key bindings ([`crate::keymap`]), the menu bar
//! ([`crate::app::app_menus`]) and buttons dispatch. Each lists its keys and the
//! view that handles it. Handlers of features that later phases build are
//! stubs that show a "not yet" toast ([`crate::views::root::not_yet`]); the
//! action and its binding exist already, so the keymap is complete and later
//! phases only replace the handler.
//!
//! Actions declared by the Phase 0 editor and grid stay in their modules and
//! are re-exported here, so this module is the one index.
//!
//! Namespace: `boa` (so `boa::ToggleSidebar` in a keymap file).

use gpui::{Action, actions};
use serde::Deserialize;

// ── App (menu bar; handled by `cx.on_action` in app.rs, any window) ────────────
actions!(
    boa,
    [
        /// ⌘Q / Boa → Quit Boa: asks first with a transaction open; a second
        /// ⌘Q within 3 s quits anyway (`app::request_quit`).
        Quit,
        /// Boa → About Boa: opens (or focuses) the About window.
        About,
        /// Boa → Check for Updates…: opens About and checks (stub until Phase 8).
        CheckForUpdates,
        /// ⌘, / Boa → Settings…: toggles the Settings view of the main window.
        ToggleSettings,
        /// ⌘H / Boa → Hide Boa.
        Hide,
        /// ⌥⌘H / Boa → Hide Others.
        HideOthers,
        /// Boa → Show All.
        ShowAll,
        /// ⌘M / Window → Minimize.
        Minimize,
        /// Window → Zoom (the Tauri "Maximize" item; macOS calls it Zoom).
        Zoom,
        /// ⌘W / File / Window → Close Window: closes the About window; in the
        /// main window ⌘W closes the tab, and this only when no tab is open
        /// (asks like a quit with a transaction open). `app::close_window`.
        CloseWindow,
        /// ⌃⌘F / View → Enter Full Screen.
        ToggleFullScreen,
    ]
);

// ── Main window (key context `Workspace`; handled by views::root) ──────────────
actions!(
    boa,
    [
        /// ⌘K, ⌘P: toggle the command palette (Phase 7).
        TogglePalette,
        /// ⌘⌥E: toggle query history (Phase 7).
        ToggleQueryHistory,
        /// ⌘O: new connection, unless a dialog is open (Phase 2).
        NewConnection,
        /// ⌘T: new console on the active database (Phase 4).
        NewQueryTab,
        /// ⌥N: new console on the active database, else the first connected
        /// one (Sidebar `newQueryTarget`; Phase 4).
        NewQueryConsole,
        /// ⌘W: close the active tab (Phase 4).
        CloseTab,
        /// ⌥↑: previous database (Phase 4).
        PreviousDatabase,
        /// ⌥↓: next database (Phase 4).
        NextDatabase,
        /// ⌥B: show / hide the Database Explorer.
        ToggleSidebar,
        /// ⌥E: show / hide the right panel.
        ToggleRightPanel,
        /// ⌥J: show / hide the Database Output panel.
        ToggleBottomPanel,
        /// ⌘⇧↵: commit the selected database's open transaction
        /// (views::tx_controls); nothing while a dialog is open.
        Commit,
        /// ⌘⇧⌫: roll back the selected database's open transaction; nothing
        /// while a dialog is open.
        Rollback,
    ]
);

/// ⌥1 … ⌥9: activate the n-th tab (0-based index; Phase 4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Action)]
#[action(namespace = boa, no_json)]
pub struct ActivateTab(pub usize);

// ── SQL editor (key context `SqlEditor`) ──────────────────────────────────────
actions!(
    boa,
    [
        /// ⌘↵ / Ctrl+↵ in a console: run the selection, else arm the
        /// statement under the caret; again (or Enter) runs it (editor).
        RunOrArm,
        /// ⌘↵ in the value editor: save the value (Phase 4).
        SaveValue,
    ]
);
/// Tab: accept the open completion, else indent.
pub use crate::editor::AcceptCompletionOrIndent;
/// Ctrl+Space: explicit completion (bound in `SqlEditor > Input`).
pub use crate::editor::TriggerCompletion;

// ── Result grid (key context `ResultGrid`) ────────────────────────────────────
/// The Phase 0 grid's actions: arrows / ⇧-arrows / PgUp / PgDn move and
/// extend, ⌘A `SelectAll`, ⌘C `Copy` (TSV), Esc `Collapse` (clear selection),
/// Enter / F2 `Edit`, ⇧↵ `EditLarge` (value editor), ⌘⌥N `SetNull`, ⌘⌥Z
/// `Revert`, ⌘⌫ / Delete `DeleteRow`; in the cell input Esc `CancelEdit`,
/// Tab / ⇧Tab `CommitEditNext` / `CommitEditPrev`.
pub use crate::grid::{
    CancelEdit, Collapse, CommitEditNext, CommitEditPrev, Copy as CopyCells, CopyAs, DeleteRow,
    Edit, EditLarge, MoveDown, MoveLeft, MoveRight, MoveUp, PageDown, PageUp, Revert, RevertCell,
    RevertRow, SelectAll, SelectDown, SelectLeft, SelectRight, SelectUp, SetNull, ViewJson,
};
actions!(
    boa,
    [
        /// ⌘N, Alt+Insert: add a row to a table tab (Phase 5).
        AddRow,
        /// ⌘↵ in a table tab's grid: preview and submit the staged edits (Phase 5).
        SubmitEdits,
    ]
);

// ── Lists: palette, query history, statement chooser, completion popup ──────
actions!(
    boa,
    [
        /// ↑ (and ⇧Tab in the palette): previous item.
        SelectPrevious,
        /// ↓ (and Tab in the palette): next item.
        SelectNext,
        /// PgUp: a page up.
        SelectPageUp,
        /// PgDn: a page down.
        SelectPageDown,
        /// Enter: open / apply the selected item.
        Confirm,
        /// Esc: close the list.
        Dismiss,
        /// ⌘⌫ in query history: delete the selected entry (Phase 7).
        DeleteHistoryEntry,
        /// ⌘↵ in query history: open the selected statement in a new console.
        OpenHistoryInConsole,
        /// ⌘C in query history: copy the selected statement (the search
        /// field's selection when it has one).
        CopyHistoryEntry,
    ]
);

// ── Smaller views ─────────────────────────────────────────────────────────────
actions!(
    boa,
    [
        /// Enter in the table filter bar: apply WHERE / ORDER BY (Phase 5).
        ApplyFilter,
        /// Esc in the table filter bar: put back the field's applied text (Phase 5).
        ClearFilter,
        /// ⌘F in Settings: focus the search field.
        FocusSettingsSearch,
        /// Esc in Settings: close it (Back).
        CloseSettings,
        /// Enter in the scope picker: apply (Phase 3).
        ApplyScope,
        /// → in the Database Explorer: expand the row, or go to its first child.
        ExpandNode,
        /// ← in the Database Explorer: collapse the row, or go to its parent.
        CollapseNode,
        /// Esc in the right panel's search: clear it (Phase 7).
        ClearPanelSearch,
        /// Enter in About: copy the version info.
        CopyVersionInfo,
    ]
);

/// Text editing in every input (Edit menu and keys): gpui-component's input
/// actions, so the menu reaches the focused Input / Editor.
pub use gpui_component::input::{
    Copy as EditCopy, Cut as EditCut, Paste as EditPaste, Redo as EditRedo,
    SelectAll as EditSelectAll, Undo as EditUndo,
};
