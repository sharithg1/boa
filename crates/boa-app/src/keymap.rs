//! Every key binding, in one place, with a key context per view.
//!
//! # Contexts
//!
//! | Context | Set by | Holds |
//! |---|---|---|
//! | (none) | | app keys: ⌘Q, ⌘M, ⌘H, ⌥⌘H, ⌃⌘F (a binding without a context counts as the deepest: never bind one a view also binds) |
//! | [`WORKSPACE`] | the main window's root view | the global shortcuts of App.tsx |
//! | [`SQL_EDITOR`] | the console editor | ⌘↵, Ctrl+Space, Tab (in `SqlEditor > Input`); ↑ ↓ PgUp PgDn Enter Esc in `SqlCompletion > Input`, ↑ ↓ Enter Esc in `StatementChooser > Input`, while open |
//! | [`RESULT_GRID`] / [`GRID_CELL_EDITOR`] | the result grid / its inline input | DataGrid.tsx keys |
//! | [`VALUE_EDITOR`] / [`JSON_VIEWER`] | value editor / JSON viewer | ⌘↵, Esc / Esc |
//! | [`FILTER_BAR`] | table filter bar (Phase 5) | Enter, Esc |
//! | [`SIDEBAR`] | Database Explorer | ↑ ↓ → ← Enter in the tree |
//! | [`RIGHT_PANEL`] | right panel | Esc in its search |
//! | [`PALETTE`] | command palette (Phase 7) | ↑ ↓ Tab ⇧Tab Enter Esc |
//! | [`QUERY_HISTORY`] | query history | ↑ ↓ PgUp PgDn Enter ⌘↵ ⌘C ⌘⌫ Esc |
//! | [`STATEMENT_CHOOSER`] | statement chooser (Phase 4) | ↑ ↓ Enter Esc |
//! | [`SCOPE_PICKER`] | scope picker | Enter, Esc (also in its filter box) |
//! | [`SETTINGS`] | Settings view | ⌘F, Esc |
//! | [`ABOUT`] | About window | ⌘W (Close Window); Enter is a key-down listener (no repeats) |
//! | [`DIALOG`] | gpui-component's dialogs | ⌘K ⌘P ⌘⌥E ⌘, (App.tsx takes them over dialogs too) |
//!
//! # Who wins (audit against gpui-component)
//!
//! GPUI prefers the binding whose context matched deepest in the focused
//! element's path; at the same depth the one bound later wins. gpui-component
//! binds (in `gpui_component::init`, which runs before [`init`]):
//!
//! - `Root` (the window root, an ancestor of everything): tab / shift-tab
//!   (focus cycling), ⌘C (copy selected text). Our `Workspace` bindings are
//!   deeper, so they win; none of them uses those keys anyway.
//! - `Input` (every text field and the code editor, about 86 bindings): arrows,
//!   ⌥←/→, ⇧⌥←/→/↑/↓, ⌘⌥↑/↓, ⌘A/C/X/V/Z, ⌘⇧Z, ⌘F, ⌘⇧F, ⌘., ⌘[ ⌘], Enter,
//!   ⇧Enter, ⌘Enter, Esc, Tab, ⇧Tab, Home/End… None of the global shortcuts
//!   (⌘K ⌘P ⌘⌥E ⌘O ⌘, ⌘T ⌘W ⌥N ⌥1–9 ⌥↑ ⌥↓ ⌥B ⌥E ⌥J ⌘⇧↵ ⌘⇧⌫) is bound
//!   there, so they reach the `Workspace` bindings from inside the editor too
//!   ("shortcuts work everywhere, including inside the editor"). Where a view
//!   must override an Input key it binds it in `<View> > Input` (same depth
//!   as Input's own, bound later): `SqlEditor > Input` Tab, `GridCellEditor >
//!   Input` Esc / Tab, `Settings > Input` Esc, `Palette > Input` ↑ ↓ Tab Enter…
//! - Dialog / Popover / Select / Combobox (escape, enter, arrows in their own
//!   contexts): only active while one is open, and deeper than ours.
//! - Inspector ⌘⌥I (debug builds only): unused by Boa.
//!
//! # The ⌥ rule (App.tsx `isOptionShortcut`)
//!
//! ⌥-shortcuts must not eat characters a keyboard layout composes with Option
//! (German ⌥5 = "[", ⌥L = "@", ⌥N = "~"…), which the editor needs. On
//! US-style layouts ⌥ produces non-ASCII symbols or dead keys, so only those
//! are taken. GPUI matches bindings on the key (`alt-5`) and gives us the
//! composed character in `Keystroke::key_char` (the mac backend asks the
//! layout what ⌥+key types). So:
//!
//! 1. [`init`] installs a keystroke interceptor that remembers the keystroke
//!    being dispatched, and an observer that forgets it afterwards.
//! 2. Every ⌥ action handler first calls [`option_shortcut_applies`]; when the
//!    keystroke composed a printable ASCII character it returns false and the
//!    handler calls `cx.propagate()`. The key then stays unhandled, and the
//!    mac backend hands it to the text input system, which inserts the
//!    character into the focused input as if no binding existed.
//!
//! Arrows (⌥↑ ⌥↓) never compose text, so they are always shortcuts, as in the
//! TS. What GPUI does not let us do: decide before the binding matches (the
//! interceptor can only swallow a key, which would also drop the character),
//! so the check lives in the handlers.

use gpui::{App, Global, KeyBinding, Keystroke};

use crate::actions::*;

/// The main window's root view.
pub const WORKSPACE: &str = "Workspace";
/// The SQL editor pane (around gpui-component's `Input` context).
pub const SQL_EDITOR: &str = crate::editor::KEY_CONTEXT;
/// The result grid.
pub const RESULT_GRID: &str = crate::grid::CONTEXT;
/// The grid's inline cell input.
pub const GRID_CELL_EDITOR: &str = "GridCellEditor";
/// The value editor (⇧↵).
pub const VALUE_EDITOR: &str = crate::views::value_editor::CONTEXT;
/// The JSON viewer.
pub const JSON_VIEWER: &str = crate::views::json_viewer::CONTEXT;
/// A table tab's WHERE / ORDER BY bar (Phase 5).
pub const FILTER_BAR: &str = "FilterBar";
/// The Database Explorer.
pub const SIDEBAR: &str = "Sidebar";
/// The right panel.
pub const RIGHT_PANEL: &str = "RightPanel";
/// The command palette (Phase 7).
pub const PALETTE: &str = "Palette";
/// Query history (views::query_history).
pub const QUERY_HISTORY: &str = "QueryHistory";
/// The statement chooser (Phase 4).
pub const STATEMENT_CHOOSER: &str = "StatementChooser";
/// The scope picker (Phase 3).
pub const SCOPE_PICKER: &str = "ScopePicker";
/// The Settings view.
pub const SETTINGS: &str = "Settings";
/// The About window.
pub const ABOUT: &str = "About";
/// gpui-component's dialogs (its `Dialog` key context, set while
/// `close_on_escape` is on, the default; none of ours turns it off).
pub const DIALOG: &str = "Dialog";

#[derive(Debug)]
struct KeysBound;

impl Global for KeysBound {}

/// The keystroke being dispatched (set by the interceptor, cleared after).
#[derive(Debug, Default)]
struct CurrentKeystroke(Option<Keystroke>);

impl Global for CurrentKeystroke {}

/// Binds every key (once per app) and installs the ⌥-rule hooks. Call after
/// `gpui_component::init` so ours are bound later than gpui-component's.
pub fn init(cx: &mut App) {
    if cx.has_global::<KeysBound>() {
        return;
    }
    cx.set_global(KeysBound);
    cx.bind_keys(bindings());
    cx.set_global(CurrentKeystroke::default());
    cx.intercept_keystrokes(|event, _, cx| {
        cx.set_global(CurrentKeystroke(Some(event.keystroke.clone())));
    })
    .detach();
    cx.observe_keystrokes(|_, _, cx| {
        cx.set_global(CurrentKeystroke(None));
    })
    .detach();
}

/// Whether a keystroke with ⌥ (and neither ⌘ nor ⌃) composes a printable
/// ASCII character on the current layout (German ⌥L = "@"): then it is text,
/// not a shortcut. Ports the inverse of App.tsx `isOptionShortcut` for keys
/// that have ⌥.
pub fn option_composes_text(keystroke: &Keystroke) -> bool {
    let m = &keystroke.modifiers;
    if !m.alt || m.platform || m.control {
        return false;
    }
    let Some(text) = keystroke.key_char.as_deref() else {
        return false;
    };
    let mut chars = text.chars();
    matches!((chars.next(), chars.next()), (Some(c), None) if (' '..='~').contains(&c))
}

/// For ⌥ action handlers: false when the keystroke that triggered the action
/// composed a character (see the module docs); the handler should then call
/// `cx.propagate()` and do nothing. True for actions not triggered by a key.
pub fn option_shortcut_applies(cx: &App) -> bool {
    !cx.try_global::<CurrentKeystroke>()
        .and_then(|k| k.0.as_ref())
        .is_some_and(option_composes_text)
}

/// All of Boa's key bindings.
pub fn bindings() -> Vec<KeyBinding> {
    let workspace = Some(WORKSPACE);
    let dialog = Some(DIALOG);
    let mut keys = vec![
        // App (no context: work in every window).
        KeyBinding::new("cmd-q", Quit, None),
        KeyBinding::new("cmd-m", Minimize, None),
        KeyBinding::new("cmd-h", Hide, None),
        KeyBinding::new("cmd-alt-h", HideOthers, None),
        KeyBinding::new("ctrl-cmd-f", ToggleFullScreen, None),
        // App.tsx useShortcuts.
        KeyBinding::new("cmd-k", TogglePalette, workspace),
        KeyBinding::new("cmd-p", TogglePalette, workspace),
        KeyBinding::new("cmd-alt-e", ToggleQueryHistory, workspace),
        KeyBinding::new("cmd-o", NewConnection, workspace),
        KeyBinding::new("cmd-,", ToggleSettings, workspace),
        KeyBinding::new("cmd-t", NewQueryTab, workspace),
        KeyBinding::new("cmd-w", CloseTab, workspace),
        KeyBinding::new("alt-n", NewQueryConsole, workspace),
        KeyBinding::new("alt-up", PreviousDatabase, workspace),
        KeyBinding::new("alt-down", NextDatabase, workspace),
        KeyBinding::new("alt-b", ToggleSidebar, workspace),
        KeyBinding::new("alt-e", ToggleRightPanel, workspace),
        KeyBinding::new("alt-j", ToggleBottomPanel, workspace),
        // GuardDialogs / TxControls.
        KeyBinding::new("cmd-shift-enter", Commit, workspace),
        KeyBinding::new("cmd-shift-backspace", Rollback, workspace),
        // App.tsx handles these before its `overlay` check, so they work
        // over a dialog too. gpui-component draws its dialogs (connection
        // dialog, password prompt, guard dialogs, edit preview) outside the
        // `Workspace` context: bound again in theirs, they reach the app-wide
        // handlers (`app::register_app_actions`), which act on the main window.
        KeyBinding::new("cmd-k", TogglePalette, dialog),
        KeyBinding::new("cmd-p", TogglePalette, dialog),
        KeyBinding::new("cmd-alt-e", ToggleQueryHistory, dialog),
        KeyBinding::new("cmd-,", ToggleSettings, dialog),
    ];
    // Guard dialogs (GuardDialogs.tsx): Enter is Cancel (focused in the
    // source); Esc closes (gpui-component's Dialog). ⌘⇧↵ / ⌘⇧⌫ answer
    // nothing there (`useTxShortcuts` returns while a dialog is open).
    let guard = Some(crate::views::guard_dialogs::GUARD_DIALOG);
    keys.push(KeyBinding::new("enter", Dismiss, guard));
    const TAB_KEYS: [&str; 9] = [
        "alt-1", "alt-2", "alt-3", "alt-4", "alt-5", "alt-6", "alt-7", "alt-8", "alt-9",
    ];
    for (i, keystroke) in TAB_KEYS.into_iter().enumerate() {
        keys.push(KeyBinding::new(keystroke, ActivateTab(i), workspace));
    }

    // SQL editor (Workspace.tsx onKeyDown): ⌘↵ / Ctrl+↵ arm, then run;
    // Ctrl+Space asks for completion explicitly; Tab accepts the open
    // completion (whole word), else indents. While the completion popup or
    // the statement chooser is open the pane adds `SqlCompletion` /
    // `StatementChooser` to its context and their keys win over the Input's.
    let editor_input = Some("SqlEditor > Input");
    keys.extend([
        KeyBinding::new("cmd-enter", RunOrArm, editor_input),
        KeyBinding::new("ctrl-enter", RunOrArm, editor_input),
        KeyBinding::new("ctrl-space", TriggerCompletion, editor_input),
        KeyBinding::new("tab", AcceptCompletionOrIndent, editor_input),
    ]);
    let completing = Some("SqlCompletion > Input");
    keys.extend([
        KeyBinding::new("up", SelectPrevious, completing),
        KeyBinding::new("down", SelectNext, completing),
        KeyBinding::new("pageup", SelectPageUp, completing),
        KeyBinding::new("pagedown", SelectPageDown, completing),
        KeyBinding::new("enter", Confirm, completing),
        KeyBinding::new("escape", Dismiss, completing),
    ]);
    // The statement chooser: only ↑ ↓ Enter Esc (Workspace.tsx); any other
    // key goes to the editor and closes it (PgUp / PgDn move the caret).
    let armed = Some("StatementChooser > Input");
    keys.extend([
        KeyBinding::new("up", SelectPrevious, armed),
        KeyBinding::new("down", SelectNext, armed),
        KeyBinding::new("enter", Confirm, armed),
        KeyBinding::new("escape", Dismiss, armed),
    ]);

    // Result grid (DataGrid.tsx). The edit keys take ⌘ or Ctrl
    // (`e.metaKey || e.ctrlKey`), as do ⌘A and ⌘C. The variants come first:
    // the right-click menu shows an action's last binding as its hint, and
    // DataGrid.tsx shows ↵, ⌘C, ⌘⌥N, ⌘N, ⌘⌫ and ⌘↵ (no other keystroke is
    // bound twice here, so the order changes nothing else). PgUp / PgDn
    // (a page of rows) are a GPUI addition: harmless, see USAGE.md.
    let grid = Some(RESULT_GRID);
    let cell = Some("GridCellEditor > Input");
    keys.extend([
        KeyBinding::new("up", MoveUp, grid),
        KeyBinding::new("down", MoveDown, grid),
        KeyBinding::new("left", MoveLeft, grid),
        KeyBinding::new("right", MoveRight, grid),
        KeyBinding::new("shift-up", SelectUp, grid),
        KeyBinding::new("shift-down", SelectDown, grid),
        KeyBinding::new("shift-left", SelectLeft, grid),
        KeyBinding::new("shift-right", SelectRight, grid),
        KeyBinding::new("pageup", PageUp, grid),
        KeyBinding::new("pagedown", PageDown, grid),
        KeyBinding::new("ctrl-a", SelectAll, grid),
        KeyBinding::new("cmd-a", SelectAll, grid),
        KeyBinding::new("ctrl-c", CopyCells, grid),
        KeyBinding::new("cmd-c", CopyCells, grid),
        KeyBinding::new("escape", Collapse, grid),
        KeyBinding::new("f2", Edit, grid),
        KeyBinding::new("enter", Edit, grid),
        KeyBinding::new("shift-enter", EditLarge, grid),
        KeyBinding::new("ctrl-alt-n", SetNull, grid),
        KeyBinding::new("cmd-alt-n", SetNull, grid),
        // DataGrid.tsx `revert`: the cell if edited, else the row. The menu's
        // "Revert row" shows this key too (as the source's does).
        KeyBinding::new("ctrl-alt-z", Revert, grid),
        KeyBinding::new("cmd-alt-z", Revert, grid),
        KeyBinding::new("delete", DeleteRow, grid),
        KeyBinding::new("ctrl-backspace", DeleteRow, grid),
        KeyBinding::new("cmd-backspace", DeleteRow, grid),
        KeyBinding::new("alt-insert", AddRow, grid),
        KeyBinding::new("ctrl-n", AddRow, grid),
        KeyBinding::new("cmd-n", AddRow, grid),
        KeyBinding::new("ctrl-enter", SubmitEdits, grid),
        KeyBinding::new("cmd-enter", SubmitEdits, grid),
        // Wins over the Input's own escape / tab (same depth, bound later).
        KeyBinding::new("escape", CancelEdit, cell),
        KeyBinding::new("tab", CommitEditNext, cell),
        KeyBinding::new("shift-tab", CommitEditPrev, cell),
        // ⇧↵ in the cell input: the value editor instead (drops the input).
        KeyBinding::new("shift-enter", EditLarge, cell),
    ]);

    // Value editor (ValueEditor.tsx) and filter bar (TableFilterBar.tsx).
    keys.extend([
        KeyBinding::new("cmd-enter", SaveValue, Some("ValueEditor > Input")),
        KeyBinding::new("escape", Dismiss, Some("ValueEditor > Input")),
        KeyBinding::new("escape", Dismiss, Some(VALUE_EDITOR)),
        KeyBinding::new("escape", Dismiss, Some(JSON_VIEWER)),
        KeyBinding::new("enter", ApplyFilter, Some("FilterBar > Input")),
        KeyBinding::new("escape", ClearFilter, Some("FilterBar > Input")),
    ]);

    // Lists. The palette and the history have a search input focused, so
    // their keys are bound over the Input's arrows / enter / tab.
    for context in ["Palette > Input", PALETTE] {
        let c = Some(context);
        keys.extend([
            KeyBinding::new("up", SelectPrevious, c),
            KeyBinding::new("down", SelectNext, c),
            KeyBinding::new("shift-tab", SelectPrevious, c),
            KeyBinding::new("tab", SelectNext, c),
            KeyBinding::new("enter", Confirm, c),
            KeyBinding::new("escape", Dismiss, c),
        ]);
    }
    for context in ["QueryHistory > Input", QUERY_HISTORY] {
        let c = Some(context);
        keys.extend([
            KeyBinding::new("up", SelectPrevious, c),
            KeyBinding::new("down", SelectNext, c),
            KeyBinding::new("pageup", SelectPageUp, c),
            KeyBinding::new("pagedown", SelectPageDown, c),
            KeyBinding::new("enter", Confirm, c),
            KeyBinding::new("cmd-backspace", DeleteHistoryEntry, c),
            KeyBinding::new("cmd-enter", OpenHistoryInConsole, c),
            KeyBinding::new("cmd-c", CopyHistoryEntry, c),
            KeyBinding::new("escape", Dismiss, c),
        ]);
    }
    let chooser = Some(STATEMENT_CHOOSER);
    keys.extend([
        KeyBinding::new("up", SelectPrevious, chooser),
        KeyBinding::new("down", SelectNext, chooser),
        KeyBinding::new("enter", Confirm, chooser),
        KeyBinding::new("escape", Dismiss, chooser),
        KeyBinding::new("enter", ApplyScope, Some(SCOPE_PICKER)),
        KeyBinding::new("escape", Dismiss, Some(SCOPE_PICKER)),
        KeyBinding::new("enter", ApplyScope, Some("ScopePicker > Input")),
        KeyBinding::new("escape", Dismiss, Some("ScopePicker > Input")),
        // Database Explorer tree (Sidebar.tsx Tree onKeyDown).
        KeyBinding::new("up", SelectPrevious, Some(SIDEBAR)),
        KeyBinding::new("down", SelectNext, Some(SIDEBAR)),
        KeyBinding::new("right", ExpandNode, Some(SIDEBAR)),
        KeyBinding::new("left", CollapseNode, Some(SIDEBAR)),
        KeyBinding::new("enter", Confirm, Some(SIDEBAR)),
        KeyBinding::new("escape", ClearPanelSearch, Some("RightPanel > Input")),
    ]);

    // Settings (SettingsView.tsx): ⌘F focuses the search, Esc closes (from
    // the search field too, unless it has text: see views::settings).
    keys.extend([
        KeyBinding::new("cmd-f", FocusSettingsSearch, Some(SETTINGS)),
        KeyBinding::new("escape", CloseSettings, Some(SETTINGS)),
        KeyBinding::new("cmd-f", FocusSettingsSearch, Some("Settings > Input")),
        KeyBinding::new("escape", CloseSettings, Some("Settings > Input")),
        // About (AboutWindow.tsx): Return copies the version info, but not
        // on a held key (`!e.repeat`), which an action can't tell: handled
        // in `AboutView`'s key-down listener. ⌘W is Tauri's Close Window
        // (the menu shows it from this binding; in the main window
        // `Workspace`'s ⌘W closes the tab, or the window when no tab is
        // open: views::root).
        KeyBinding::new("cmd-w", CloseWindow, Some(ABOUT)),
    ]);
    keys
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(source: &'static str, key_char: Option<&str>) -> Keystroke {
        let mut k = Keystroke::parse(source).unwrap();
        k.key_char = key_char.map(str::to_owned);
        k
    }

    #[test]
    fn option_symbols_and_dead_keys_are_shortcuts() {
        // US layout: ⌥B = ∫, ⌥E = ´ (dead), ⌥J = ∆, ⌥1 = ¡.
        assert!(!option_composes_text(&key("alt-b", Some("∫"))));
        assert!(!option_composes_text(&key("alt-e", Some("´"))));
        assert!(!option_composes_text(&key("alt-j", Some("∆"))));
        assert!(!option_composes_text(&key("alt-1", Some("¡"))));
        assert!(!option_composes_text(&key("alt-n", Some(""))));
        assert!(!option_composes_text(&key("alt-n", None)));
    }

    #[test]
    fn option_composed_ascii_is_text() {
        // German: ⌥5 = [, ⌥L = @, ⌥N = ~; French ⌥⇧L = |.
        assert!(option_composes_text(&key("alt-5", Some("["))));
        assert!(option_composes_text(&key("alt-l", Some("@"))));
        assert!(option_composes_text(&key("alt-n", Some("~"))));
        assert!(option_composes_text(&key("alt-shift-l", Some("|"))));
    }

    #[test]
    fn arrows_and_other_modifiers_are_never_text() {
        assert!(!option_composes_text(&key("alt-up", None)));
        assert!(!option_composes_text(&key("alt-down", None)));
        assert!(!option_composes_text(&key("cmd-alt-e", Some("e"))));
        assert!(!option_composes_text(&key("ctrl-alt-e", Some("e"))));
        assert!(!option_composes_text(&key("b", Some("b"))));
    }

    /// GPUI ranks a binding without a context as the deepest match, so one
    /// would beat every view's binding of the same key (a context-less ⌘W
    /// would close the window instead of the tab).
    #[test]
    fn context_less_keys_are_bound_nowhere_else() {
        let keys = bindings();
        let global: Vec<_> = keys.iter().filter(|b| b.predicate().is_none()).collect();
        assert_eq!(global.len(), 5, "⌘Q ⌘M ⌘H ⌥⌘H ⌃⌘F");
        for g in global {
            let clash = keys
                .iter()
                .filter(|b| b.predicate().is_some() && b.keystrokes() == g.keystrokes())
                .count();
            assert_eq!(clash, 0, "{:?}", g.keystrokes());
        }
        // Close Window's ⌘W lives in About (the menu shows it from there).
        let wanted = Keystroke::parse("cmd-w").unwrap();
        assert!(keys.iter().any(|b| {
            b.action().partial_eq(&CloseWindow)
                && b.keystrokes()[0].key() == wanted.key
                && b.keystrokes()[0].modifiers() == &wanted.modifiers
                && b.predicate().is_some_and(|p| p.to_string() == ABOUT)
        }));
    }

    #[test]
    fn every_global_shortcut_of_app_tsx_is_bound_in_workspace() {
        let keys = bindings();
        let bound = |source: &'static str| {
            let wanted = Keystroke::parse(source).unwrap();
            keys.iter().any(|b| {
                b.keystrokes().len() == 1
                    && b.keystrokes()[0].modifiers() == &wanted.modifiers
                    && b.keystrokes()[0].key() == wanted.key
                    && b.predicate().is_some_and(|p| p.to_string() == WORKSPACE)
            })
        };
        for k in [
            "cmd-k",
            "cmd-p",
            "cmd-alt-e",
            "cmd-o",
            "cmd-,",
            "cmd-t",
            "cmd-w",
            "alt-n",
            "alt-1",
            "alt-9",
            "alt-up",
            "alt-down",
            "alt-b",
            "alt-e",
            "alt-j",
            "cmd-shift-enter",
            "cmd-shift-backspace",
        ] {
            assert!(bound(k), "{k} is not bound in Workspace");
        }
    }
}
