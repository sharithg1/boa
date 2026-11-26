//! The global shortcuts against the overlays, ⌘W and the windows, the app
//! menu keys and the quit hook, through the real keymap (App.tsx
//! `useShortcuts`, GuardDialogs.tsx `useTxShortcuts`, Tauri's default menu).

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use boa_core::grid::grid_edits::Editability;
use boa_core::model::{QueryResult, ResultColumn};
use boa_core::state::guards::CloseAction;
use boa_core::tx::TxState as CoreTxState;
use gpui::{
    AppContext as _, Entity, Focusable as _, Keystroke, TestAppContext, VisualTestContext, px, size,
};
use gpui_component::{Root, WindowExt as _};

use crate::app_state::AppState;
use crate::grid::selection::Cell;
use crate::grid::{GridContent, ResultGrid};
use crate::models::catalogs::DbKey;
use crate::models::console::ConsoleResult;
use crate::models::layout::PanelKind;
use crate::models::transactions::TxState;
use crate::tests::{FAKE_CONNECTION, TempDir, init, layout_open, open_main};
use crate::views::about::{AboutView, AppInfo};
use crate::views::guard_dialogs;
use crate::views::root::RootView;

fn draw(vcx: &mut VisualTestContext) {
    vcx.run_until_parked();
    vcx.update(|window, cx| window.draw(cx).clear(cx));
    vcx.run_until_parked();
}

fn fake_key() -> DbKey {
    DbKey {
        connection_id: FAKE_CONNECTION.into(),
        database: "boa_dev".into(),
    }
}

fn tab_count(vcx: &mut VisualTestContext) -> usize {
    vcx.update(|_, cx| {
        let ws = AppState::global(cx).workspaces.read(cx);
        ws.active_workspace().map_or(0, |w| w.tabs.len())
    })
}

fn open_tx(vcx: &mut VisualTestContext) {
    vcx.update(|_, cx| {
        let t = AppState::global(cx).transactions.clone();
        t.update(cx, |t, cx| {
            t.note_outcome(&fake_key(), Some(TxState::Active), cx)
        });
    });
}

fn tx_state(vcx: &mut VisualTestContext) -> TxState {
    vcx.update(|_, cx| {
        AppState::global(cx)
            .transactions
            .read(cx)
            .state(&fake_key())
    })
}

/// Presses `keys` and says whether that started a commit / rollback (read
/// before the backend can answer, so it doesn't race the pretend server).
fn starts_tx_end(vcx: &mut VisualTestContext, keys: &'static str) -> bool {
    let keystroke = Keystroke::parse(keys).unwrap();
    vcx.update(|window, cx| {
        window.dispatch_keystroke(keystroke, cx);
        AppState::global(cx)
            .transactions
            .read(cx)
            .is_busy(&fake_key())
    })
}

fn has_dialog(vcx: &mut VisualTestContext) -> bool {
    vcx.update(|window, cx| window.has_active_dialog(cx))
}

fn result(rows: usize) -> QueryResult {
    QueryResult {
        columns: vec![
            ResultColumn::new("id", "int4"),
            ResultColumn::new("name", "text"),
            ResultColumn::new("payload", "jsonb"),
        ],
        rows: (0..rows)
            .map(|i| {
                vec![
                    Some(i.to_string()),
                    Some(format!("name {i}")),
                    Some(format!("{{\"n\": {i}}}")),
                ]
            })
            .collect(),
        affected: None,
        truncated: false,
        pageable: true,
        ms: 1,
        tx: CoreTxState::default(),
    }
}

/// Gives the active console a result and shows it in the Database Output
/// panel; returns that panel's grid (`editable`: as a table tab's).
fn show_result(
    root: &Entity<RootView>,
    vcx: &mut VisualTestContext,
    editable: bool,
) -> Entity<ResultGrid> {
    vcx.update(|_, cx| {
        let state = AppState::global(cx).clone();
        let (tab, console) = {
            let w = state.workspaces.read(cx);
            let tab = w.active_tab().unwrap();
            (tab.id.clone(), tab.console().cloned().unwrap())
        };
        console.update(cx, |c, cx| {
            c.result = Some(ConsoleResult {
                sql: "select * from t".into(),
                result: Arc::new(result(5)),
                offset: 0,
                page_size: 500,
                ms: 1,
                run: 1,
            });
            cx.notify();
        });
        state
            .workspaces
            .update(cx, |w, cx| w.set_bottom_view(Some(tab), cx));
    });
    draw(vcx);
    let grid = vcx.update(|_, cx| root.read(cx).bottom_panel().read(cx).grid().clone());
    assert!(vcx.update(|_, cx| grid.read(cx).page().is_some()), "shown");
    if editable {
        vcx.update(|_, cx| {
            grid.update(cx, |g, cx| {
                g.set_content(
                    GridContent {
                        sql: "select * from t".into(),
                        result: Arc::new(result(5)),
                        offset: 0,
                        page_size: 500,
                        editability: Editability::Editable {
                            pk: vec!["id".into()],
                        },
                    },
                    cx,
                )
            })
        });
        draw(vcx);
    }
    grid
}

/// Every App.tsx shortcut that returns early over a dialog, plus ⌘O and the
/// tx keys, pressed while an overlay has the focus: none of them may act.
fn assert_blocked(vcx: &mut VisualTestContext, what: &str) {
    let tabs = tab_count(vcx);
    for keys in [
        "cmd-w", "cmd-t", "alt-n", "alt-1", "alt-down", "alt-b", "alt-e", "alt-j",
    ] {
        vcx.simulate_keystrokes(keys);
        draw(vcx);
        assert_eq!(tab_count(vcx), tabs, "{keys} over {what}");
    }
    for panel in [PanelKind::Sidebar, PanelKind::RightPanel, PanelKind::Bottom] {
        assert!(layout_open(vcx, panel), "{panel:?} over {what}");
    }
    assert!(!starts_tx_end(vcx, "cmd-shift-enter"), "⌘⇧↵ over {what}");
    assert!(
        !starts_tx_end(vcx, "cmd-shift-backspace"),
        "⌘⇧⌫ over {what}"
    );
    assert_eq!(tx_state(vcx), TxState::Active);
    vcx.simulate_keystrokes("cmd-o");
    draw(vcx);
    assert!(!has_dialog(vcx), "⌘O over {what}");
}

/// The audit's live finding (P1-1): ⌘W with the value editor open closed
/// the tab under it and dropped the typed text.
#[gpui::test]
fn the_value_editor_blocks_cmd_w_and_the_other_global_shortcuts(cx: &mut TestAppContext) {
    let dir = TempDir::new("keys-value-editor");
    init(cx, dir.path());
    let (root, mut vcx) = open_main(cx);
    let grid = show_result(&root, &mut vcx, true);
    open_tx(&mut vcx);
    vcx.update(|window, cx| {
        grid.update(cx, |g, cx| g.open_value_editor(Cell::new(1, 1), window, cx))
    });
    draw(&mut vcx);
    vcx.simulate_keystrokes("cmd-a");
    vcx.simulate_input("typed");
    assert_blocked(&mut vcx, "the value editor");
    let editor = vcx.update(|_, cx| grid.read(cx).value_editor().cloned());
    let editor = editor.expect("still open");
    assert_eq!(vcx.update(|_, cx| editor.read(cx).text(cx)), "typed");
    // Esc closes it; then ⌘W closes the tab again.
    vcx.simulate_keystrokes("escape");
    draw(&mut vcx);
    assert!(vcx.update(|_, cx| grid.read(cx).value_editor().is_none()));
    vcx.simulate_keystrokes("cmd-w");
    assert_eq!(tab_count(&mut vcx), 0);
}

#[gpui::test]
fn the_json_viewer_history_and_palette_block_them_too(cx: &mut TestAppContext) {
    let dir = TempDir::new("keys-overlays");
    init(cx, dir.path());
    let (root, mut vcx) = open_main(cx);
    let grid = show_result(&root, &mut vcx, false);
    open_tx(&mut vcx);

    // JSON viewer (read-only console result).
    let opened =
        vcx.update(|window, cx| grid.update(cx, |g, cx| g.open_json(Cell::new(0, 2), window, cx)));
    assert!(opened);
    draw(&mut vcx);
    assert_blocked(&mut vcx, "the JSON viewer");
    assert!(vcx.update(|_, cx| grid.read(cx).json_viewer().is_some()));
    vcx.simulate_keystrokes("escape");
    draw(&mut vcx);
    assert!(vcx.update(|_, cx| grid.read(cx).json_viewer().is_none()));

    // Query History (⌘⌥E still toggles it: App.tsx handles it first).
    vcx.simulate_keystrokes("cmd-alt-e");
    draw(&mut vcx);
    assert!(vcx.update(|_, cx| root.read(cx).query_history().is_some()));
    assert_blocked(&mut vcx, "query history");
    // A ⌥ key that is a shortcut elsewhere types its character here (US ⌥B = ∫).
    let alt_b = Keystroke {
        modifiers: gpui::Modifiers::alt(),
        key: "b".into(),
        key_char: Some("∫".into()),
    };
    vcx.update(|window, cx| window.dispatch_keystroke(alt_b, cx));
    draw(&mut vcx);
    assert!(layout_open(&mut vcx, PanelKind::Sidebar));
    let search = vcx.update(|_, cx| {
        let view = root.read(cx).query_history().unwrap().clone();
        view.read(cx).input().read(cx).value().to_string()
    });
    assert!(search.contains('∫'), "{search:?}");
    vcx.simulate_keystrokes("cmd-alt-e");
    draw(&mut vcx);
    assert!(vcx.update(|_, cx| root.read(cx).query_history().is_none()));

    // The command palette (⌘K again closes it).
    vcx.simulate_keystrokes("cmd-k");
    draw(&mut vcx);
    assert!(vcx.update(|_, cx| root.read(cx).palette().is_some()));
    assert_blocked(&mut vcx, "the palette");
    vcx.simulate_keystrokes("cmd-k");
    draw(&mut vcx);
    assert!(vcx.update(|_, cx| root.read(cx).palette().is_none()));

    // Nothing open: they work again.
    assert!(
        starts_tx_end(&mut vcx, "cmd-shift-backspace"),
        "⌘⇧⌫ rolls back"
    );
}

/// Settings is not a dialog: ⌘O and the tx keys work over it (App.tsx,
/// GuardDialogs.tsx); the tab keys don't (`state.settings`). With a dialog
/// open none of the tx keys answer.
#[gpui::test]
fn over_settings_cmd_o_and_the_tx_keys_work_and_cmd_w_does_nothing(cx: &mut TestAppContext) {
    let dir = TempDir::new("keys-settings");
    init(cx, dir.path());
    let (_root, mut vcx) = open_main(cx);
    open_tx(&mut vcx);
    vcx.simulate_keystrokes("cmd-,");
    draw(&mut vcx);
    vcx.simulate_keystrokes("cmd-w cmd-t");
    draw(&mut vcx);
    assert_eq!(tab_count(&mut vcx), 1);
    assert!(
        vcx.update(|_, cx| !cx.windows().is_empty()),
        "the window stays"
    );
    vcx.simulate_keystrokes("cmd-o");
    draw(&mut vcx);
    assert!(has_dialog(&mut vcx), "⌘O over Settings opens the dialog");
    // In the connection dialog: no tx keys, no second dialog.
    assert!(!starts_tx_end(&mut vcx, "cmd-shift-backspace"));
    vcx.simulate_keystrokes("escape");
    draw(&mut vcx);
    assert!(!has_dialog(&mut vcx));
    // Settings has the focus back: ⌘⇧⌫ rolls back over it.
    assert!(starts_tx_end(&mut vcx, "cmd-shift-backspace"));
}

/// GuardDialogs.tsx: Cancel is focused (Enter), Esc closes, and the tx
/// keys answer nothing (`useTxShortcuts` returns over a dialog).
#[gpui::test]
fn guard_dialogs_answer_enter_and_escape_but_not_the_tx_keys(cx: &mut TestAppContext) {
    let dir = TempDir::new("keys-guard");
    init(cx, dir.path());
    let (_root, mut vcx) = open_main(cx);
    open_tx(&mut vcx);
    for close in ["escape", "enter"] {
        let task =
            vcx.update(|_, cx| guard_dialogs::confirm_close_tx(CloseAction::Disconnect, None, cx));
        draw(&mut vcx);
        assert!(vcx.update(|_, cx| guard_dialogs::active(cx)).is_some());
        assert!(!starts_tx_end(&mut vcx, "cmd-shift-backspace"));
        assert!(!starts_tx_end(&mut vcx, "cmd-shift-enter"));
        vcx.simulate_keystrokes("cmd-w");
        draw(&mut vcx);
        assert!(
            vcx.update(|_, cx| guard_dialogs::active(cx)).is_some(),
            "still asking"
        );
        assert_eq!(tab_count(&mut vcx), 1);
        vcx.simulate_keystrokes(close);
        draw(&mut vcx);
        assert!(vcx.update(|_, cx| guard_dialogs::active(cx)).is_none());
        let answer = Rc::new(RefCell::new(None));
        vcx.update(|_, cx| {
            let answer = answer.clone();
            cx.spawn(async move |_| *answer.borrow_mut() = Some(task.await))
                .detach();
        });
        draw(&mut vcx);
        assert_eq!(*answer.borrow(), Some(false), "{close} is Cancel");
    }
}

/// Tauri's Close Window (⌘W): App.tsx only takes ⌘W when it closes a tab, so
/// with none open the window closes, asking first with a transaction open.
#[gpui::test]
fn cmd_w_without_a_tab_closes_the_main_window_asking_with_a_transaction_open(
    cx: &mut TestAppContext,
) {
    let dir = TempDir::new("keys-close-main");
    init(cx, dir.path());
    let (_root, mut vcx) = open_main(cx);
    vcx.update(|window, _| window.activate_window());
    vcx.simulate_keystrokes("cmd-w");
    draw(&mut vcx);
    assert_eq!(tab_count(&mut vcx), 0, "the first ⌘W closes the tab");
    assert_eq!(cx.read(|cx| cx.windows().len()), 1);

    open_tx(&mut vcx);
    vcx.simulate_keystrokes("cmd-w");
    draw(&mut vcx);
    let title = vcx.update(|_, cx| guard_dialogs::active(cx).map(|g| g.read(cx).kind().title()));
    assert_eq!(title, Some("Quit with an open transaction?"));
    assert_eq!(cx.read(|cx| cx.windows().len()), 1, "held");
    // Cancel; and the menu's Close Window does nothing while it asks.
    vcx.update(|window, cx| window.dispatch_action(Box::new(crate::actions::CloseWindow), cx));
    draw(&mut vcx);
    assert_eq!(cx.read(|cx| cx.windows().len()), 1, "not over a dialog");
    vcx.simulate_keystrokes("escape");
    draw(&mut vcx);
    assert!(vcx.update(|_, cx| guard_dialogs::active(cx)).is_none());

    // No transaction: ⌘W closes it.
    vcx.update(|_, cx| {
        let t = AppState::global(cx).transactions.clone();
        t.update(cx, |t, cx| {
            t.note_outcome(&fake_key(), Some(TxState::Idle), cx)
        });
    });
    vcx.simulate_keystrokes("cmd-w");
    cx.run_until_parked();
    assert_eq!(cx.read(|cx| cx.windows().len()), 0, "closed");
}

fn about_info() -> AppInfo {
    AppInfo {
        name: "boa-desktop".into(),
        version: "0.6.1".into(),
        arch: "aarch64".into(),
        commit: "abc123".into(),
        commit_date: "2026-11-20T10:00:00Z".into(),
        os: "macOS 15.4.1".into(),
    }
}

#[gpui::test]
fn cmd_w_closes_the_about_window(cx: &mut TestAppContext) {
    let dir = TempDir::new("keys-about");
    init(cx, dir.path());
    let (_root, _main) = open_main(cx);
    let handle = cx.open_window(size(px(400.), px(330.)), |window, cx| {
        let view = cx.new(|cx| AboutView::new(about_info(), cx));
        window.focus(&view.focus_handle(cx), cx);
        Root::new(view, window, cx)
    });
    let mut about = VisualTestContext::from_window(handle.into(), cx);
    // The key window (what the menu and `cx.active_window()` act on).
    about.update(|window, _| window.activate_window());
    about.run_until_parked();
    assert_eq!(cx.read(|cx| cx.windows().len()), 2);
    about.simulate_keystrokes("cmd-w");
    cx.run_until_parked();
    let left: Vec<_> = cx.read(|cx| cx.windows().iter().map(|w| w.window_id()).collect());
    assert_eq!(left.len(), 1, "About closed, the main window stays");
    assert_ne!(left[0], handle.window_id());
}

#[gpui::test]
fn the_app_menu_keys_dispatch_their_actions(cx: &mut TestAppContext) {
    use crate::actions::{Hide, HideOthers, Minimize};
    let dir = TempDir::new("keys-menu");
    init(cx, dir.path());
    let (_root, mut vcx) = open_main(cx);
    // Hide / Minimize aren't implemented by the test platform: count them
    // in a listener that runs first (registered later) and stops there.
    let hits = Rc::new(RefCell::new(Vec::new()));
    vcx.update(|_, cx| {
        let h = hits.clone();
        cx.on_action(move |_: &Minimize, _| h.borrow_mut().push("minimize"));
        let h = hits.clone();
        cx.on_action(move |_: &Hide, _| h.borrow_mut().push("hide"));
        let h = hits.clone();
        cx.on_action(move |_: &HideOthers, _| h.borrow_mut().push("hide others"));
    });
    vcx.simulate_keystrokes("cmd-m cmd-h cmd-alt-h");
    assert_eq!(*hits.borrow(), ["minimize", "hide", "hide others"]);
    // ⌃⌘F goes all the way (the window is updated after the dispatch).
    vcx.update(|window, _| window.activate_window());
    assert!(!vcx.update(|window, _| window.is_fullscreen()));
    vcx.simulate_keystrokes("ctrl-cmd-f");
    draw(&mut vcx);
    assert!(vcx.update(|window, _| window.is_fullscreen()));
}

/// Tauri lib.rs: `RunEvent::Exit` kills the SSH tunnels. Here the quit
/// hook does (the backend's `ssh -N` children would outlive the app: the
/// AppState global is never dropped).
#[gpui::test]
fn quitting_closes_the_ssh_tunnels(cx: &mut TestAppContext) {
    let dir = TempDir::new("keys-quit");
    init(cx, dir.path());
    assert!(!cx.read(crate::app::tunnels_closed));
    cx.quit();
    assert!(cx.read(crate::app::tunnels_closed));
}

/// The Edit menu (undo, redo, cut, copy, paste, select all) dispatches to
/// the focused element: a plain text field (the palette's search) and the
/// value editor get it as the console editor does (`tests.rs`).
#[gpui::test]
fn edit_menu_actions_reach_a_focused_text_field(cx: &mut TestAppContext) {
    use crate::actions::{EditCopy, EditCut, EditPaste, EditRedo, EditSelectAll, EditUndo};
    let dir = TempDir::new("keys-edit-menu");
    init(cx, dir.path());
    let (root, mut vcx) = open_main(cx);
    let menu = |vcx: &mut VisualTestContext, action: Box<dyn gpui::Action>| {
        vcx.update(|window, cx| window.dispatch_action(action, cx));
        draw(vcx);
    };
    let clipboard = |vcx: &mut VisualTestContext| {
        vcx.update(|_, cx| cx.read_from_clipboard().and_then(|c| c.text()))
            .unwrap_or_default()
    };

    vcx.simulate_keystrokes("cmd-k");
    draw(&mut vcx);
    vcx.simulate_input("orders");
    draw(&mut vcx);
    let search = |vcx: &mut VisualTestContext| {
        vcx.update(|_, cx| {
            let palette = root.read(cx).palette().unwrap().clone();
            palette.read(cx).input().read(cx).value().to_string()
        })
    };
    assert_eq!(search(&mut vcx), "orders");
    menu(&mut vcx, Box::new(EditSelectAll));
    menu(&mut vcx, Box::new(EditCut));
    assert_eq!(search(&mut vcx), "");
    assert_eq!(clipboard(&mut vcx), "orders");
    menu(&mut vcx, Box::new(EditUndo));
    assert_eq!(search(&mut vcx), "orders", "Undo");
    menu(&mut vcx, Box::new(EditRedo));
    assert_eq!(search(&mut vcx), "", "Redo");
    menu(&mut vcx, Box::new(EditPaste));
    menu(&mut vcx, Box::new(EditPaste));
    assert_eq!(search(&mut vcx), "ordersorders", "Paste");
    vcx.simulate_keystrokes("escape");
    draw(&mut vcx);

    // The value editor.
    let grid = show_result(&root, &mut vcx, true);
    vcx.update(|window, cx| {
        grid.update(cx, |g, cx| g.open_value_editor(Cell::new(2, 1), window, cx))
    });
    draw(&mut vcx);
    let editor = vcx.update(|_, cx| grid.read(cx).value_editor().cloned().unwrap());
    let text = |vcx: &mut VisualTestContext| vcx.update(|_, cx| editor.read(cx).text(cx));
    assert_eq!(text(&mut vcx), "name 2");
    menu(&mut vcx, Box::new(EditSelectAll));
    menu(&mut vcx, Box::new(EditCopy));
    assert_eq!(clipboard(&mut vcx), "name 2");
    menu(&mut vcx, Box::new(EditCut));
    assert_eq!(text(&mut vcx), "");
    menu(&mut vcx, Box::new(EditPaste));
    assert_eq!(text(&mut vcx), "name 2");
}

fn palette_open(root: &Entity<RootView>, vcx: &mut VisualTestContext) -> bool {
    vcx.update(|_, cx| root.read(cx).palette().is_some())
}

/// App.tsx takes ⌘K / ⌘P, ⌘⌥E and ⌘, before its overlay check, so they
/// work over a dialog: the palette and History (Radix dialogs) open on top
/// of it and give it the focus back when closed; Settings (`z-40`) opens
/// under it and the dialog keeps the focus. gpui-component's dialogs are
/// outside the `Workspace` context.
#[gpui::test]
fn cmd_k_cmd_alt_e_and_cmd_comma_work_over_a_dialog(cx: &mut TestAppContext) {
    let dir = TempDir::new("keys-over-dialog");
    init(cx, dir.path());
    let (root, mut vcx) = open_main(cx);
    vcx.simulate_keystrokes("cmd-o");
    draw(&mut vcx);
    assert!(has_dialog(&mut vcx), "the connection dialog");

    for keys in ["cmd-k", "cmd-p"] {
        vcx.simulate_keystrokes(keys);
        draw(&mut vcx);
        assert!(palette_open(&root, &mut vcx), "{keys} over the dialog");
        assert!(vcx.update(|_, cx| root.read(cx).palette_over_dialog()));
        assert!(has_dialog(&mut vcx), "the dialog stays");
        let focused = vcx.update(|window, cx| {
            let palette = root.read(cx).palette().unwrap().clone();
            let input = palette.read(cx).input().clone();
            input.focus_handle(cx).is_focused(window)
        });
        assert!(focused, "the palette's search has the focus");
        // Esc closes the palette only.
        vcx.simulate_keystrokes("escape");
        draw(&mut vcx);
        assert!(!palette_open(&root, &mut vcx));
        assert!(
            has_dialog(&mut vcx),
            "Esc closed the palette, not the dialog"
        );
    }
    // ⌘K toggles: from the palette (over the dialog) it closes it again.
    vcx.simulate_keystrokes("cmd-k");
    draw(&mut vcx);
    vcx.simulate_keystrokes("cmd-k");
    draw(&mut vcx);
    assert!(!palette_open(&root, &mut vcx));
    assert!(has_dialog(&mut vcx));

    // ⌘⌥E: History over the dialog; again closes it.
    vcx.simulate_keystrokes("cmd-alt-e");
    draw(&mut vcx);
    assert!(vcx.update(|_, cx| root.read(cx).query_history().is_some()));
    vcx.simulate_keystrokes("cmd-alt-e");
    draw(&mut vcx);
    assert!(vcx.update(|_, cx| root.read(cx).query_history().is_none()));
    assert!(has_dialog(&mut vcx));

    // ⌘,: Settings opens under the dialog, which keeps the focus: Esc
    // closes the dialog, then Settings.
    vcx.simulate_keystrokes("cmd-,");
    draw(&mut vcx);
    let settings_open = |vcx: &mut VisualTestContext| {
        vcx.update(|_, cx| AppState::global(cx).layout.read(cx).settings().is_some())
    };
    assert!(settings_open(&mut vcx));
    assert!(has_dialog(&mut vcx));
    vcx.simulate_keystrokes("escape");
    draw(&mut vcx);
    assert!(!has_dialog(&mut vcx));
    assert!(settings_open(&mut vcx));
    let in_settings = vcx.update(|window, cx| {
        let settings = root.read(cx).settings().focus_handle(cx);
        settings.contains_focused(window, cx)
    });
    assert!(in_settings, "Settings has the focus back");
    vcx.simulate_keystrokes("escape");
    draw(&mut vcx);
    assert!(!settings_open(&mut vcx));

    // A guard dialog too.
    open_tx(&mut vcx);
    let _task =
        vcx.update(|_, cx| guard_dialogs::confirm_close_tx(CloseAction::Disconnect, None, cx));
    draw(&mut vcx);
    assert!(vcx.update(|_, cx| guard_dialogs::active(cx)).is_some());
    vcx.simulate_keystrokes("cmd-k");
    draw(&mut vcx);
    assert!(palette_open(&root, &mut vcx), "⌘K over a guard dialog");
    vcx.simulate_keystrokes("escape");
    draw(&mut vcx);
    assert!(vcx.update(|_, cx| guard_dialogs::active(cx)).is_some());
}

/// Tauri's View item is AppKit's toggleFullScreen:, which reads "Exit Full
/// Screen" while the window is in full screen.
#[gpui::test]
fn the_view_menu_says_exit_full_screen_in_full_screen(cx: &mut TestAppContext) {
    let dir = TempDir::new("keys-full-screen");
    init(cx, dir.path());
    let (_root, mut vcx) = open_main(cx);
    vcx.update(|window, _| window.activate_window());
    let label = |cx: &mut TestAppContext| {
        cx.read(|cx| cx.get_menus())
            .unwrap_or_default()
            .into_iter()
            .find(|m| m.name.as_ref() == "View")
            .and_then(|m| match m.items.first() {
                Some(gpui::OwnedMenuItem::Action { name, .. }) => Some(name.to_string()),
                _ => None,
            })
    };
    vcx.simulate_keystrokes("ctrl-cmd-f");
    draw(&mut vcx);
    assert!(vcx.update(|window, _| window.is_fullscreen()));
    assert_eq!(label(cx).as_deref(), Some("Exit Full Screen"));
    vcx.simulate_keystrokes("ctrl-cmd-f");
    draw(&mut vcx);
    assert!(!vcx.update(|window, _| window.is_fullscreen()));
    assert_eq!(label(cx).as_deref(), Some("Enter Full Screen"));
}
