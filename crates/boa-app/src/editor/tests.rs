//! The editor in a real (test) window: keys through GPUI's keymap into
//! gpui-component's editor and back out as Boa behaviour (arm-then-run,
//! the statement chooser, our completion popup).

use std::cell::RefCell;
use std::rc::Rc;

use gpui::{
    Entity, KeyDownEvent, Keystroke, PlatformInput, Subscription, TestAppContext, VisualTestContext,
};
use gpui_component::Root;

use super::completion::tests::demo_schemas;
use super::*;

struct Fixture {
    editor: Entity<SqlEditor>,
    ran: Rc<RefCell<Vec<String>>>,
    _run: Subscription,
}

fn open(cx: &mut TestAppContext) -> (Fixture, &mut VisualTestContext) {
    cx.update(|cx| {
        gpui_component::init(cx);
        crate::theme::init(cx);
        super::init(cx);
    });
    let slot: Rc<RefCell<Option<Entity<SqlEditor>>>> = Rc::default();
    let (_, vcx) = cx.add_window_view({
        let slot = slot.clone();
        move |window, cx| {
            let editor = cx.new(|cx| SqlEditor::new(window, cx));
            *slot.borrow_mut() = Some(editor.clone());
            Root::new(editor, window, cx)
        }
    });
    let editor = slot.borrow_mut().take().expect("editor built");
    let ran: Rc<RefCell<Vec<String>>> = Rc::default();
    let run = vcx.update(|_, cx| {
        let ran = ran.clone();
        cx.subscribe(&editor, move |_, event: &RunStatement, _| {
            ran.borrow_mut().push(event.sql.clone())
        })
    });
    settle(vcx);
    (
        Fixture {
            editor,
            ran,
            _run: run,
        },
        vcx,
    )
}

/// Lets async work finish and draws a frame (focus events, the overlays and
/// the input handler need one), as the app does between keys.
fn settle(cx: &mut VisualTestContext) {
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));
    cx.run_until_parked();
}

/// Presses a key as the native window does: the key binding only. (GPUI's
/// `simulate_keystrokes` also types Enter's "\n" when an action propagates.)
fn press(cx: &mut VisualTestContext, key: &'static str) {
    let keystroke = Keystroke::parse(key).expect("keystroke");
    cx.update(move |window, cx| {
        window.dispatch_event(
            PlatformInput::KeyDown(KeyDownEvent {
                keystroke,
                is_held: false,
                prefer_character_input: false,
            }),
            cx,
        );
    });
    settle(cx);
}

impl Fixture {
    fn state(&self, cx: &mut VisualTestContext) -> Entity<EditorState> {
        cx.update(|_, cx| self.editor.read(cx).state().clone())
    }

    /// Sets the text and the selection (bytes), focused.
    fn set(&self, text: &str, selection: Range<usize>, cx: &mut VisualTestContext) {
        let editor = self.editor.clone();
        cx.update(|window, cx| {
            editor.update(cx, |editor, cx| {
                editor.set_text(text.to_owned(), window, cx);
                editor.state().update(cx, |state, cx| {
                    state.set_selected_range(selection, cx);
                    state.focus(window, cx);
                });
            });
        });
        settle(cx);
    }

    fn caret_to(&self, at: usize, cx: &mut VisualTestContext) {
        let state = self.state(cx);
        cx.update(|_, cx| state.update(cx, |s, cx| s.set_selected_range(at..at, cx)));
        settle(cx);
    }

    fn text(&self, cx: &mut VisualTestContext) -> String {
        cx.update(|_, cx| self.editor.read(cx).text(cx).to_string())
    }

    fn cursor(&self, cx: &mut VisualTestContext) -> usize {
        let state = self.state(cx);
        cx.update(|_, cx| state.read(cx).cursor())
    }

    fn menu(&self, cx: &mut VisualTestContext) -> Option<Vec<String>> {
        cx.update(|_, cx| {
            self.editor
                .read(cx)
                .completion()
                .map(|c| c.result.items.iter().map(|i| i.label.clone()).collect())
        })
    }

    fn selected(&self, cx: &mut VisualTestContext) -> Option<usize> {
        cx.update(|_, cx| self.editor.read(cx).completion().map(|c| c.selected))
    }

    fn armed(&self, cx: &mut VisualTestContext) -> Option<Armed> {
        cx.update(|_, cx| self.editor.read(cx).armed().cloned())
    }

    fn ran(&self) -> Vec<String> {
        self.ran.borrow().clone()
    }

    fn with_demo_catalog(&self, cx: &mut VisualTestContext) {
        let editor = self.editor.clone();
        cx.update(|_, cx| editor.update(cx, |editor, cx| editor.set_schemas(demo_schemas(), cx)));
        settle(cx);
    }
}

const SCRIPT: &str = "select 1;\n\nselect name\nfrom customers\nwhere id = 7;\n\nselect 3;\n";
const STATEMENT: &str = "select name\nfrom customers\nwhere id = 7";

#[gpui::test]
fn cmd_enter_arms_then_runs_the_statement_under_the_caret(cx: &mut TestAppContext) {
    let (f, cx) = open(cx);
    let caret = SCRIPT.find("customers").unwrap() + 3;
    f.set(SCRIPT, caret..caret, cx);
    cx.simulate_keystrokes("cmd-enter");
    settle(cx);
    // The first ⌘↵ arms (fills) the statement; nothing runs yet.
    assert!(f.ran().is_empty());
    let start = SCRIPT.find("select name").unwrap();
    let armed = f.armed(cx).expect("armed");
    assert_eq!(armed.range(), start..start + STATEMENT.len());
    assert_eq!(armed.items.len(), 1);
    // ⌘↵ again runs it.
    cx.simulate_keystrokes("cmd-enter");
    settle(cx);
    assert_eq!(f.ran(), vec![STATEMENT.to_owned()]);
    assert_eq!(f.armed(cx), None);
    // ⌘↵ doesn't edit.
    assert_eq!(f.text(cx), SCRIPT);
}

#[gpui::test]
fn enter_runs_the_armed_statement_and_escape_or_a_caret_move_disarms(cx: &mut TestAppContext) {
    let (f, cx) = open(cx);
    let caret = SCRIPT.find("customers").unwrap();
    f.set(SCRIPT, caret..caret, cx);
    cx.simulate_keystrokes("cmd-enter");
    settle(cx);
    assert!(f.armed(cx).is_some());
    press(cx, "escape");
    assert_eq!(f.armed(cx), None);
    assert_eq!(f.text(cx), SCRIPT);
    // Arm again, move the caret: disarmed.
    cx.simulate_keystrokes("cmd-enter");
    settle(cx);
    assert!(f.armed(cx).is_some());
    press(cx, "right");
    assert_eq!(f.armed(cx), None);
    // Arm, Enter runs (and inserts no newline).
    cx.simulate_keystrokes("cmd-enter");
    settle(cx);
    press(cx, "enter");
    assert_eq!(f.ran(), vec![STATEMENT.to_owned()]);
    assert_eq!(f.text(cx), SCRIPT);
    // Typing while armed disarms and edits.
    cx.simulate_keystrokes("cmd-enter");
    settle(cx);
    cx.simulate_input("x");
    settle(cx);
    assert_eq!(f.armed(cx), None);
    assert_eq!(f.ran().len(), 1);
}

#[gpui::test]
fn a_selection_runs_at_once(cx: &mut TestAppContext) {
    let (f, cx) = open(cx);
    let from = SCRIPT.find("name").unwrap();
    f.set(SCRIPT, from..from + "name\nfrom customers".len(), cx);
    cx.simulate_keystrokes("cmd-enter");
    settle(cx);
    assert_eq!(f.ran(), vec!["name\nfrom customers".to_owned()]);
    assert_eq!(f.armed(cx), None);
}

#[gpui::test]
fn nothing_arms_on_a_blank_line_between_statements(cx: &mut TestAppContext) {
    let (f, cx) = open(cx);
    let blank = SCRIPT.find("\n\nselect 3").unwrap() + 1;
    f.set(SCRIPT, blank..blank, cx);
    cx.simulate_keystrokes("cmd-enter");
    settle(cx);
    assert_eq!(f.armed(cx), None);
    assert!(f.ran().is_empty());
}

#[gpui::test]
fn the_toolbar_button_runs_directly_when_there_is_one_statement(cx: &mut TestAppContext) {
    let (f, cx) = open(cx);
    let caret = SCRIPT.find("customers").unwrap();
    f.set(SCRIPT, caret..caret, cx);
    let editor = f.editor.clone();
    cx.update(|window, cx| editor.update(cx, |e, cx| e.run_or_arm(true, window, cx)));
    settle(cx);
    assert_eq!(f.ran(), vec![STATEMENT.to_owned()]);
}

const NESTED: &str = "select *\nfrom (select id from customers where id > 10) c\nwhere c.id < 20;";

#[gpui::test]
fn the_statement_chooser_offers_the_subquery(cx: &mut TestAppContext) {
    let (f, cx) = open(cx);
    let caret = NESTED.find("id from").unwrap() + 1;
    f.set(NESTED, caret..caret, cx);
    cx.simulate_keystrokes("cmd-enter");
    settle(cx);
    let armed = f.armed(cx).expect("armed");
    assert_eq!(armed.items.len(), 2, "{armed:?}");
    assert_eq!(armed.selected, 0);
    // ↓ highlights the subquery, ↑ goes back, ↓ again; arrows don't move the caret.
    press(cx, "down");
    assert_eq!(f.armed(cx).map(|a| a.selected), Some(1));
    press(cx, "up");
    assert_eq!(f.armed(cx).map(|a| a.selected), Some(0));
    press(cx, "down");
    assert_eq!(f.cursor(cx), caret);
    press(cx, "enter");
    assert_eq!(
        f.ran(),
        vec!["select id from customers where id > 10".to_owned()]
    );
    assert_eq!(f.armed(cx), None);
    // The whole statement: arm, Enter.
    cx.simulate_keystrokes("cmd-enter");
    settle(cx);
    press(cx, "enter");
    assert_eq!(f.ran()[1], NESTED.trim_end_matches(';'));
}

/// Workspace.tsx: while the chooser is open only ↑ ↓ Enter Esc are its
/// own; any other key closes it and goes to the editor (PgDn moves the
/// caret). PgUp / PgDn don't page through the chooser.
#[gpui::test]
fn page_keys_close_the_statement_chooser_and_move_the_caret(cx: &mut TestAppContext) {
    let (f, cx) = open(cx);
    let caret = NESTED.find("id from").unwrap() + 1;
    f.set(NESTED, caret..caret, cx);
    cx.simulate_keystrokes("cmd-enter");
    settle(cx);
    assert_eq!(f.armed(cx).map(|a| a.items.len()), Some(2));
    press(cx, "pagedown");
    assert_eq!(f.armed(cx), None);
    assert_ne!(f.cursor(cx), caret, "the caret moved");
    assert!(f.ran().is_empty());
}

#[gpui::test]
fn the_statement_under_the_caret_is_outlined(cx: &mut TestAppContext) {
    let (f, cx) = open(cx);
    let caret = SCRIPT.find("where").unwrap();
    f.set(SCRIPT, caret..caret, cx);
    let start = SCRIPT.find("select name").unwrap();
    let end = SCRIPT.find("7;").unwrap() + 1;
    let outlined = cx.update(|_, cx| f.editor.read(cx).outlined_statement());
    assert_eq!(outlined, Some(start..end));
    // Moving the caret onto a blank line clears it.
    let blank = SCRIPT.find("\n\nselect 3").unwrap() + 1;
    f.caret_to(blank, cx);
    let outlined = cx.update(|_, cx| f.editor.read(cx).outlined_statement());
    assert_eq!(outlined, None);
}

#[gpui::test]
fn the_outline_starts_left_of_column_0_clear_of_the_line_numbers(cx: &mut TestAppContext) {
    let (f, cx) = open(cx);
    let text = "select 1;\n\nselect a, b, c\n  from t\n  where b;";
    f.set(text, 0..0, cx);
    let start = text.find("select a").unwrap();
    f.caret_to(start, cx);
    let range = cx
        .update(|_, cx| f.editor.read(cx).outlined_statement())
        .unwrap();
    let state = f.state(cx);
    let (rows, text_left) = cx
        .update(|_, cx| outline::rows(state.read(cx), &range))
        .unwrap();
    assert_eq!(rows.len(), 3);
    // Column 0 is where the text area starts (what the editor clips to)…
    assert_eq!(rows[0].left, text_left);
    let caret = cx
        .update(|_, cx| state.read(cx).range_to_bounds(&(start..start)))
        .unwrap();
    assert_eq!(caret.left(), text_left);
    // …and the frame is drawn left of it, so the caret there is inside.
    let left = outline::outline_points(&rows)
        .iter()
        .map(|p| p.x)
        .fold(text_left, gpui::Pixels::min);
    assert!(left < text_left - px(1.), "{left:?} vs {text_left:?}");
    // lib/statements.ts `rangePath`: every line spans its own text.
    let first_end = cx
        .update(|_, cx| state.read(cx).range_to_bounds(&(start..start + 14)))
        .unwrap();
    assert_eq!(rows[0].right, first_end.right());
    let second_end = cx
        .update(|_, cx| state.read(cx).range_to_bounds(&(start + 15..start + 23)))
        .unwrap();
    assert_eq!(rows[1].right, second_end.right());
    assert!(rows[1].right < rows[0].right);
    assert_eq!(rows[1].left, text_left);
    let end = cx
        .update(|_, cx| state.read(cx).range_to_bounds(&(range.end..range.end)))
        .unwrap();
    assert_eq!(rows[2].right, end.left());

    // A last line wider than the rest juts out; a blank line inside the
    // statement is one cell wide.
    let text = "select a\n\n  from t\n  where b = 'a long condition';";
    f.set(text, 0..0, cx);
    let range = cx
        .update(|_, cx| f.editor.read(cx).outlined_statement())
        .unwrap();
    let (rows, _) = cx
        .update(|_, cx| outline::rows(state.read(cx), &range))
        .unwrap();
    assert_eq!(rows.len(), 4);
    let first_end = cx
        .update(|_, cx| state.read(cx).range_to_bounds(&(0..8)))
        .unwrap();
    assert_eq!(rows[0].right, first_end.right());
    let cell = (first_end.right() - first_end.left()) / 8.;
    assert_eq!(rows[1].right, rows[1].left + cell);
    assert!(rows[2].right < rows[0].right + cell);
    assert!(rows[3].right > rows[0].right);
}

#[gpui::test]
fn the_text_starts_after_the_56_px_gutter(cx: &mut TestAppContext) {
    // Workspace.tsx: a `w-14` gutter (its divider included), then `pl-3`.
    // The editor is the window's root here, so its left edge is x = 0.
    let (f, cx) = open(cx);
    let state = f.state(cx);
    let text_left = cx.update(|_, cx| {
        let s = state.read(cx);
        s.input_bounds().left() + s.text_origin_offset().unwrap()
    });
    assert!(
        (text_left - (GUTTER_WIDTH + TEXT_PAD_LEFT)).abs() < px(0.5),
        "{text_left:?}"
    );
}

#[gpui::test]
fn the_caret_is_reported_one_based(cx: &mut TestAppContext) {
    let (f, cx) = open(cx);
    let caret = SCRIPT.find("customers").unwrap();
    f.set(SCRIPT, caret..caret, cx);
    let pos = cx.update(|_, cx| f.editor.read(cx).caret());
    assert_eq!(pos, Some(CaretPos { line: 4, col: 6 }));
    assert_eq!(pos.unwrap().to_string(), "4:6");
    assert_eq!(caret_pos("é\nab", 6), CaretPos { line: 2, col: 3 });
}

#[gpui::test]
fn typing_opens_completion_and_tab_replaces_the_whole_word(cx: &mut TestAppContext) {
    let (f, cx) = open(cx);
    f.with_demo_catalog(cx);
    let text = "select * from cstomers";
    f.set(text, 15..15, cx);
    cx.simulate_input("u");
    settle(cx);
    let items = f.menu(cx).expect("completion popup open");
    assert_eq!(items.first().map(String::as_str), Some("customers"));
    cx.simulate_keystrokes("tab");
    settle(cx);
    assert_eq!(f.text(cx), "select * from customers");
    assert_eq!(f.cursor(cx), "select * from customers".len());
    assert_eq!(f.menu(cx), None);
}

#[gpui::test]
fn enter_inserts_before_the_rest_of_the_word(cx: &mut TestAppContext) {
    let (f, cx) = open(cx);
    f.with_demo_catalog(cx);
    let text = "select * from cstomers";
    f.set(text, 15..15, cx);
    cx.simulate_input("u");
    settle(cx);
    assert!(f.menu(cx).is_some());
    press(cx, "enter");
    assert_eq!(f.text(cx), "select * from customersstomers");
    assert_eq!(f.menu(cx), None);
}

#[gpui::test]
fn arrows_and_pages_move_in_the_list_and_escape_closes(cx: &mut TestAppContext) {
    let (f, cx) = open(cx);
    f.with_demo_catalog(cx);
    // Explicit with an empty prefix after select: many items.
    f.set("select ", 7..7, cx);
    cx.simulate_keystrokes("ctrl-space");
    settle(cx);
    let n = f.menu(cx).expect("items").len();
    assert!(n > 12, "{n} items");
    press(cx, "pagedown");
    assert_eq!(f.selected(cx), Some(9));
    press(cx, "down");
    assert_eq!(f.selected(cx), Some(10));
    press(cx, "pageup");
    assert_eq!(f.selected(cx), Some(1));
    press(cx, "up");
    press(cx, "up");
    assert_eq!(f.selected(cx), Some(n - 1), "↑ wraps");
    // The caret stays put while the list has the arrows.
    assert_eq!(f.cursor(cx), 7);
    press(cx, "escape");
    assert_eq!(f.menu(cx), None);
    assert_eq!(f.text(cx), "select ");
}

#[gpui::test]
fn ctrl_space_offers_foreign_key_joins_and_stays_open_while_erasing(cx: &mut TestAppContext) {
    let (f, cx) = open(cx);
    f.with_demo_catalog(cx);
    let text = "select * from orders o join customers c on ";
    f.set(text, text.len()..text.len(), cx);
    cx.simulate_keystrokes("ctrl-space");
    settle(cx);
    let items = f.menu(cx).expect("completion popup open");
    assert_eq!(
        items.first().map(String::as_str),
        Some("c.id = o.customer_id")
    );
    // Sticky explicit mode: type a letter, erase it, the list stays.
    cx.simulate_input("c");
    settle(cx);
    assert!(f.menu(cx).is_some());
    press(cx, "backspace");
    assert_eq!(f.text(cx), text);
    let items = f.menu(cx).expect("still open after erasing");
    assert_eq!(
        items.first().map(String::as_str),
        Some("c.id = o.customer_id")
    );
    press(cx, "enter");
    assert_eq!(f.text(cx), format!("{text}c.id = o.customer_id"));
}

#[gpui::test]
fn without_ctrl_space_erasing_the_prefix_closes_the_list(cx: &mut TestAppContext) {
    let (f, cx) = open(cx);
    f.with_demo_catalog(cx);
    f.set("select * from ", 14..14, cx);
    cx.simulate_input("c");
    settle(cx);
    assert!(f.menu(cx).is_some());
    press(cx, "backspace");
    assert_eq!(f.menu(cx), None);
}

#[gpui::test]
fn functions_put_the_caret_inside_and_schemas_reopen_on_their_tables(cx: &mut TestAppContext) {
    let (f, cx) = open(cx);
    f.with_demo_catalog(cx);
    f.set("select co", 9..9, cx);
    cx.simulate_input("u");
    settle(cx);
    let i = cx.update(|_, cx| {
        let open = f.editor.read(cx).completion().cloned().expect("functions");
        open.result
            .items
            .iter()
            .position(|c| c.insert.starts_with("count("))
            .expect("count")
    });
    let editor = f.editor.clone();
    cx.update(|window, cx| editor.update(cx, |e, cx| e.accept_completion(i, false, window, cx)));
    settle(cx);
    assert_eq!(f.text(cx), "select count()");
    assert_eq!(f.cursor(cx), "select count(".len(), "count(|)");

    f.set("select * from bi", 16..16, cx);
    cx.simulate_input("l");
    settle(cx);
    let items = f.menu(cx).expect("schemas");
    assert_eq!(items.first().map(String::as_str), Some("billing"));
    press(cx, "enter");
    assert_eq!(f.text(cx), "select * from billing.");
    let items = f.menu(cx).expect("re-opened on the schema's tables");
    assert_eq!(items, vec!["invoices".to_owned()]);
}

#[gpui::test]
fn tab_indents_without_a_menu_and_undo_redo_work(cx: &mut TestAppContext) {
    let (f, cx) = open(cx);
    f.set("select 1", 0..0, cx);
    cx.simulate_keystrokes("tab");
    settle(cx);
    assert_eq!(f.text(cx), "    select 1");
    cx.simulate_input("x");
    settle(cx);
    assert_eq!(f.text(cx), "    xselect 1");
    cx.simulate_keystrokes("cmd-z");
    settle(cx);
    assert_eq!(f.text(cx), "    select 1");
    cx.simulate_keystrokes("cmd-shift-z");
    settle(cx);
    assert_eq!(f.text(cx), "    xselect 1");
}

#[gpui::test]
fn the_demo_script_runs_its_statements(cx: &mut TestAppContext) {
    let (f, cx) = open(cx);
    let state = f.state(cx);
    let focused = cx.update(|window, cx| state.read(cx).focus_handle(cx).is_focused(window));
    assert!(focused, "the editor takes the focus when it opens");
    // The editor starts on the first statement of the sample.
    cx.simulate_keystrokes("cmd-enter");
    settle(cx);
    cx.simulate_keystrokes("cmd-enter");
    settle(cx);
    let ran = f.ran();
    assert_eq!(ran.len(), 1);
    assert!(ran[0].starts_with("select id, name"), "{ran:?}");
    assert!(SAMPLE_SQL.contains("select * from public.orders;"));
}

#[gpui::test]
fn ime_composition_commits_into_the_buffer(cx: &mut TestAppContext) {
    let (f, cx) = open(cx);
    f.set("select ''", 8..8, cx);
    let state = f.state(cx);
    // Compose (marked text), then commit, as an input method does.
    cx.update(|window, cx| {
        state.update(cx, |s, cx| {
            s.replace_and_mark_text_in_range(None, "ㅎ", None, window, cx);
        })
    });
    settle(cx);
    assert_eq!(f.text(cx), "select 'ㅎ'");
    cx.update(|window, cx| {
        state.update(cx, |s, cx| {
            s.replace_and_mark_text_in_range(None, "하", None, window, cx);
            s.replace_text_in_range(None, "한", window, cx);
        })
    });
    settle(cx);
    assert_eq!(f.text(cx), "select '한'");
    // A string: no completion popup; ⌘↵ arms, ⌘↵ runs.
    assert_eq!(f.menu(cx), None);
    cx.simulate_keystrokes("cmd-enter");
    settle(cx);
    cx.simulate_keystrokes("cmd-enter");
    settle(cx);
    assert_eq!(f.ran(), vec!["select '한'".to_owned()]);
}
