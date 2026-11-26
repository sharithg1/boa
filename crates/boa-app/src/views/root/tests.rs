//! Integration tests of the main window: focus and the global shortcuts.

use gpui::{Focusable as _, Modifiers, TestAppContext, VisualTestContext, point, px};

use crate::app_state::AppState;
use crate::models::layout::PanelKind;
use crate::tests::{TempDir, init, layout_open, open_main};

fn tab_count(vcx: &mut VisualTestContext) -> usize {
    vcx.update(|_, cx| {
        let ws = AppState::global(cx).workspaces.read(cx);
        ws.active_workspace().map_or(0, |w| w.tabs.len())
    })
}

/// Hiding the explorer while it has the focus (⌥B after a click in the
/// tree) left nothing focused, so no key reached the `Workspace` context:
/// ⌥B could not bring the explorer back and ⌘T did nothing. The same
/// happened after a dialog opened from a context menu closed (the menu it
/// gives the focus back to is gone).
#[gpui::test]
fn the_shortcuts_survive_the_focused_view_going_away(cx: &mut TestAppContext) {
    let dir = TempDir::new("focus-lost");
    init(cx, dir.path());
    let (root, mut vcx) = open_main(cx);
    let editor_focused = |vcx: &mut VisualTestContext| {
        vcx.update(|window, cx| {
            let editor = root.read(cx).workspace().read(cx).editor(cx);
            editor.is_some_and(|e| e.focus_handle(cx).is_focused(window))
        })
    };
    assert!(editor_focused(&mut vcx));
    // A click on the explorer's first row (the connection) focuses the tree.
    vcx.simulate_click(point(px(90.), px(91.)), Modifiers::none());
    vcx.run_until_parked();
    assert!(!editor_focused(&mut vcx), "the explorer took the focus");

    vcx.simulate_keystrokes("alt-b");
    assert!(!layout_open(&mut vcx, PanelKind::Sidebar));
    vcx.update(|window, _| window.refresh());
    vcx.run_until_parked();
    assert!(
        vcx.update(|window, cx| window.focused(cx).is_some()),
        "something keeps the focus"
    );
    assert!(editor_focused(&mut vcx), "the active console got it back");

    vcx.simulate_keystrokes("alt-b");
    assert!(layout_open(&mut vcx, PanelKind::Sidebar), "⌥B still works");
    let before = tab_count(&mut vcx);
    vcx.simulate_keystrokes("cmd-t");
    assert_eq!(tab_count(&mut vcx), before + 1, "⌘T still works");
}
