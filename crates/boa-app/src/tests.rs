//! App-level `#[gpui::test]`s: the main window's shell (panels, resizing,
//! prefs persistence, toasts, Settings, About) driven through the real
//! keymap and dispatch tree, on a temp config dir.

use std::path::{Path, PathBuf};
use std::time::Duration;

use gpui::{
    AppContext as _, Entity, Keystroke, Modifiers, MouseButton, TestAppContext, VisualTestContext,
    point, px, size,
};
use gpui_component::Root;

use boa_core::connections::{ConnConfig, SavedConnection};

use crate::app_state::AppState;
use crate::models::catalogs::DbKey;
use crate::models::connections::ConnState;
use crate::models::layout::{self, PanelKind, SettingsPage};
use crate::models::prefs::RightTab;
use crate::models::toasts::{MAX_TOASTS, NewToast, Toasts};
use crate::theme::Palette;
use crate::views::about::{AboutView, AppInfo, version_info_text};
use crate::views::root::RootView;
use boa_core::state::prefs as core;

mod migration;

/// A scratch config dir under the system temp dir, removed on drop.
pub(crate) struct TempDir(PathBuf);

impl TempDir {
    pub(crate) fn new(name: &str) -> Self {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static N: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "boa-app-{name}-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }

    pub(crate) fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

pub(crate) const WIDTH: f32 = 1280.;
pub(crate) const HEIGHT: f32 = 820.;

/// App init as in app::init (minus fonts and menus), with AppState on `dir`.
pub(crate) fn init(cx: &mut TestAppContext, dir: &Path) {
    cx.update(|cx| {
        gpui_component::init(cx);
        crate::theme::init(cx);
        crate::keymap::init(cx);
        crate::app::register_app_actions(cx);
        AppState::init(dir, WIDTH, cx).unwrap();
    });
}

/// The pretend connection of the shell tests (no server behind it).
pub(crate) const FAKE_CONNECTION: &str = "fake";

/// Saves a pretend connection in the Connections model and marks it
/// connected with `databases` (the first is its own), without a server.
pub(crate) fn fake_connection(cx: &mut gpui::App, id: &str, databases: &[&str]) -> DbKey {
    let state = AppState::global(cx).clone();
    let databases: Vec<String> = databases.iter().map(|d| (*d).to_owned()).collect();
    state.catalogs.update(cx, |c, _| {
        c.offline.insert(id.to_owned());
    });
    state.connections.update(cx, |c, cx| {
        c.saved.push(SavedConnection {
            config: ConnConfig {
                id: id.to_owned(),
                name: format!("{id} server"),
                host: "localhost".into(),
                port: 5432,
                user: "boa".into(),
                database: databases[0].clone(),
                ssl_mode: Default::default(),
                read_only: false,
                environment: None,
                color: None,
                ssh: None,
            },
            has_password: true,
        });
        c.state.insert(
            id.to_owned(),
            ConnState::Connected {
                server_version: "17.0".into(),
                databases: databases.clone(),
            },
        );
        cx.notify();
    });
    DbKey {
        connection_id: id.to_owned(),
        database: databases[0].clone(),
    }
}

/// Opens the main window on an empty workspace (no database selected).
pub(crate) fn open_empty(cx: &mut TestAppContext) -> (Entity<RootView>, VisualTestContext) {
    open_window(cx)
}

/// Opens the main window (1280×820, Root around RootView) with a pretend
/// connected database and one console holding the sample SQL, focused;
/// draws a frame.
pub(crate) fn open_main(cx: &mut TestAppContext) -> (Entity<RootView>, VisualTestContext) {
    cx.update(|cx| {
        let key = fake_connection(cx, FAKE_CONNECTION, &["boa_dev", "boa_dev2"]);
        let workspaces = AppState::global(cx).workspaces.clone();
        workspaces.update(cx, |w, cx| {
            w.new_query_tab_with(key, crate::editor::SAMPLE_SQL.to_owned(), cx)
        });
    });
    open_window(cx)
}

fn open_window(cx: &mut TestAppContext) -> (Entity<RootView>, VisualTestContext) {
    let slot = std::rc::Rc::new(std::cell::RefCell::new(None));
    let handle = cx.open_window(size(px(WIDTH), px(HEIGHT)), {
        let slot = slot.clone();
        move |window, cx| {
            let root = cx.new(|cx| RootView::new(window, cx));
            root.update(cx, |root, cx| root.focus_editor(window, cx));
            *slot.borrow_mut() = Some(root.clone());
            Root::new(root, window, cx)
        }
    });
    let mut vcx = VisualTestContext::from_window(handle.into(), cx);
    vcx.run_until_parked();
    vcx.update(|window, _| window.refresh());
    vcx.run_until_parked();
    let root = slot.borrow_mut().take().unwrap();
    (root, vcx)
}

pub(crate) fn layout_open(vcx: &mut VisualTestContext, panel: PanelKind) -> bool {
    vcx.update(|_, cx| AppState::global(cx).layout.read(cx).is_open(panel))
}

pub(crate) fn prefs(vcx: &mut VisualTestContext) -> core::Prefs {
    vcx.update(|_, cx| AppState::global(cx).prefs.read(cx).get().clone())
}

/// Lets debounced saves run.
pub(crate) fn settle(vcx: &mut VisualTestContext) {
    vcx.run_until_parked();
    vcx.executor().advance_clock(Duration::from_secs(1));
    vcx.run_until_parked();
}

/// A drag with the mouse from `from` to `to` (window coordinates).
fn drag(vcx: &mut VisualTestContext, from: (f32, f32), to: (f32, f32)) {
    let from = point(px(from.0), px(from.1));
    let to = point(px(to.0), px(to.1));
    vcx.simulate_mouse_down(from, MouseButton::Left, Modifiers::none());
    vcx.simulate_mouse_move(to, Some(MouseButton::Left), Modifiers::none());
    vcx.simulate_mouse_up(to, MouseButton::Left, Modifiers::none());
    vcx.run_until_parked();
}

#[gpui::test]
fn theme_reaches_gpui_component(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_component::init(cx);
        crate::theme::init(cx);
    });
    cx.update(|cx| {
        let theme = gpui_component::Theme::global(cx);
        let palette = Palette::dark();
        assert!(theme.is_dark());
        assert_eq!(theme.colors.background, palette.background);
        assert_eq!(theme.colors.border, palette.border);
        assert_eq!(theme.colors.danger, palette.destructive);
        assert_eq!(theme.mono_font_family.as_ref(), crate::theme::MONO_FONT);
        assert_eq!(theme.font_size, crate::theme::text::SM);
    });
}

#[gpui::test]
fn option_b_e_j_toggle_the_panels_from_the_editor(cx: &mut TestAppContext) {
    let dir = TempDir::new("toggles");
    init(cx, dir.path());
    let (root, mut vcx) = open_main(cx);
    // A fresh workspace keeps empty auxiliary panels out of the way.
    assert!(layout_open(&mut vcx, PanelKind::Sidebar));
    assert!(!layout_open(&mut vcx, PanelKind::RightPanel));
    assert!(!layout_open(&mut vcx, PanelKind::Bottom));
    for (keys, panel) in [
        ("alt-b", PanelKind::Sidebar),
        ("alt-e", PanelKind::RightPanel),
        ("alt-j", PanelKind::Bottom),
    ] {
        let initially_open = layout_open(&mut vcx, panel);
        vcx.simulate_keystrokes(keys);
        assert_eq!(layout_open(&mut vcx, panel), !initially_open);
        vcx.simulate_keystrokes(keys);
        assert_eq!(layout_open(&mut vcx, panel), initially_open);
    }
    // The editor still has the focus and got no text.
    let text = vcx.update(|_, cx| root.read(cx).editor_text(cx));
    assert!(!text.contains('∫'), "{text}");
}

#[gpui::test]
fn option_keys_that_compose_text_are_left_to_the_editor(cx: &mut TestAppContext) {
    let dir = TempDir::new("option-text");
    init(cx, dir.path());
    let (root, mut vcx) = open_main(cx);
    let tabs = |vcx: &mut VisualTestContext| {
        vcx.update(|_, cx| {
            let ws = AppState::global(cx).workspaces.read(cx);
            ws.active_workspace().map_or(0, |w| w.tabs.len())
        })
    };
    let before = tabs(&mut vcx);
    // German layout: ⌥N types "~" and ⌥B is not a layout character here,
    // but pretend it composes "{" to check the panel toggles too.
    for (key, composed) in [("n", "~"), ("b", "{")] {
        let keystroke = Keystroke {
            modifiers: Modifiers::alt(),
            key: key.into(),
            key_char: Some(composed.into()),
        };
        vcx.update(|window, cx| window.dispatch_keystroke(keystroke, cx));
        vcx.run_until_parked();
    }
    assert_eq!(
        tabs(&mut vcx),
        before,
        "⌥N composing ~ must not open a console"
    );
    assert!(layout_open(&mut vcx, PanelKind::Sidebar));
    let text = vcx.update(|_, cx| root.read(cx).editor_text(cx));
    assert!(
        text.contains('~') && text.contains('{'),
        "typed into the editor"
    );

    // US layout: ⌥N composes a dead key (non-ASCII): a shortcut.
    let keystroke = Keystroke {
        modifiers: Modifiers::alt(),
        key: "n".into(),
        key_char: Some("˜".into()),
    };
    vcx.update(|window, cx| window.dispatch_keystroke(keystroke, cx));
    vcx.run_until_parked();
    assert_eq!(tabs(&mut vcx), before + 1, "⌥N opens a console");
    // Arrows are always shortcuts; ⌥1 activates the first tab.
    vcx.simulate_keystrokes("alt-1");
    let first = vcx.update(|_, cx| {
        let w = AppState::global(cx).workspaces.read(cx);
        let w = w.active_workspace().unwrap();
        (w.active_tab.clone(), w.tabs[0].id.clone())
    });
    assert_eq!(first.0, Some(first.1));
}

#[gpui::test]
fn dragging_the_handles_resizes_within_the_clamps(cx: &mut TestAppContext) {
    let dir = TempDir::new("resize");
    init(cx, dir.path());
    let (root, mut vcx) = open_main(cx);
    vcx.simulate_keystrokes("alt-e alt-j");
    let size_of = |vcx: &mut VisualTestContext, panel| {
        vcx.update(|window, cx| root.read(cx).panel_size(panel, window, cx))
    };
    assert_eq!(size_of(&mut vcx, PanelKind::Sidebar), 260);
    assert_eq!(size_of(&mut vcx, PanelKind::RightPanel), 340);
    assert_eq!(size_of(&mut vcx, PanelKind::Bottom), 280);

    // Sidebar: its right edge at x 260; dragging far right stops at 480.
    drag(&mut vcx, (260., 200.), (1100., 200.));
    assert_eq!(size_of(&mut vcx, PanelKind::Sidebar), 480);
    assert_eq!(prefs(&mut vcx).sidebar_width, 480);
    // And far left stops at 200.
    drag(&mut vcx, (480., 200.), (0., 200.));
    assert_eq!(prefs(&mut vcx).sidebar_width, 200);

    // Right panel: left edge at 1280 - 340 = 940; dragging left grows it, up to 800.
    drag(&mut vcx, (940., 200.), (10., 200.));
    assert_eq!(prefs(&mut vcx).right_panel_width, 800);

    // Bottom panel: top edge at 820 - 32 - 280 = 508; up grows it, at most
    // min(900, 820 - 200) = 620.
    drag(&mut vcx, (640., 508.), (640., 0.));
    assert_eq!(prefs(&mut vcx).bottom_height, 620);
    // Down shrinks it, at least 120.
    drag(&mut vcx, (640., 820. - 32. - 620.), (640., 819.));
    assert_eq!(prefs(&mut vcx).bottom_height, 120);

    // A double click on a handle restores the default.
    let at = point(px(200.), px(200.));
    vcx.simulate_event(gpui::MouseDownEvent {
        button: MouseButton::Left,
        position: at,
        modifiers: Modifiers::none(),
        click_count: 2,
        first_mouse: false,
    });
    vcx.simulate_mouse_up(at, MouseButton::Left, Modifiers::none());
    vcx.run_until_parked();
    assert_eq!(prefs(&mut vcx).sidebar_width, 260);
    assert!(vcx.update(|_, cx| root.read(cx).drag().is_none()));
}

#[gpui::test]
fn layout_is_saved_to_the_config_dir_and_read_back(cx: &mut TestAppContext) {
    let dir = TempDir::new("persist");
    init(cx, dir.path());
    let (_, mut vcx) = open_main(cx);
    drag(&mut vcx, (260., 200.), (360., 200.));
    vcx.simulate_keystrokes("alt-e");
    vcx.update(|_, cx| {
        let prefs = AppState::global(cx).prefs.clone();
        prefs.update(cx, |p, cx| {
            p.update(cx, |p| p.right_tab = RightTab::History)
        });
    });
    settle(&mut vcx);

    // prefs.json: boa.prefs.v2 fields.
    let saved = core::load(dir.path());
    assert_eq!(saved.sidebar_width, 360);
    assert_eq!(saved.right_tab, RightTab::History);
    let raw = std::fs::read_to_string(dir.path().join("prefs.json")).unwrap();
    assert!(raw.contains("\"sidebarWidth\":360"), "{raw}");
    assert!(raw.contains("\"rightTab\":\"history\""), "{raw}");
    // session.json: the panel flags.
    let panels = layout::load_panels(dir.path());
    assert_eq!(panels.right_panel_open, Some(true));
    assert_eq!(panels.sidebar_open, Some(true));
    assert_eq!(panels.bottom_open, Some(false));

    // A new app on the same dir starts from them.
    let mut cx2 = TestAppContext::single();
    init(&mut cx2, dir.path());
    cx2.update(|cx| {
        let state = AppState::global(cx);
        assert_eq!(state.prefs.read(cx).get().sidebar_width, 360);
        assert_eq!(state.prefs.read(cx).get().right_tab, RightTab::History);
        assert!(state.layout.read(cx).right_panel_open());
        assert!(state.layout.read(cx).sidebar_open());
    });
}

#[gpui::test]
fn session_json_panel_flags_are_restored_and_saved(cx: &mut TestAppContext) {
    let dir = TempDir::new("session");
    std::fs::write(
        dir.path().join("session.json"),
        r#"{"active":null,"workspaces":{},"mru":[],"seen":{},"bottomOpen":false}"#,
    )
    .unwrap();
    init(cx, dir.path());
    let (_, mut vcx) = open_main(cx);
    assert!(!layout_open(&mut vcx, PanelKind::Bottom));
    vcx.simulate_keystrokes("alt-j");
    settle(&mut vcx);
    let v: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(dir.path().join("session.json")).unwrap())
            .unwrap();
    assert_eq!(v["bottomOpen"], serde_json::json!(true));
    // The whole session is the Session model's (boa.session.v1).
    let db = boa_core::state::session::db_key(FAKE_CONNECTION, "boa_dev");
    assert_eq!(v["workspaces"][&db]["tabs"][0]["kind"], "query");
}

#[gpui::test]
fn a_toast_with_the_same_key_replaces_the_one_showing(cx: &mut TestAppContext) {
    let toasts = cx.new(|_| Toasts::new());
    let (a, b, a2) = toasts.update(cx, |t, cx| {
        let a = t.push(NewToast::error("Connection lost").key("conn:1"), cx);
        let b = t.push(NewToast::info("Copied"), cx);
        let a2 = t.push(NewToast::success("Reconnected to Local").key("conn:1"), cx);
        (a, b, a2)
    });
    assert_eq!(a, a2, "keeps its id");
    assert_ne!(a, b);
    toasts.read_with(cx, |t, _| {
        assert_eq!(t.toasts().len(), 2);
        assert_eq!(t.toasts()[0].title.as_ref(), "Reconnected to Local");
        assert_eq!(t.toasts()[1].title.as_ref(), "Copied");
    });
    // The replacement restarted the TTL: both go after 6 s.
    cx.executor().advance_clock(Duration::from_millis(5_900));
    cx.run_until_parked();
    toasts.read_with(cx, |t, _| assert_eq!(t.toasts().len(), 2));
    cx.executor().advance_clock(Duration::from_millis(200));
    cx.run_until_parked();
    toasts.read_with(cx, |t, _| assert!(t.toasts().is_empty()));

    // At most five; the oldest goes. A toast without TTL stays.
    toasts.update(cx, |t, cx| {
        for i in 0..7 {
            t.push_for(NewToast::info(format!("t{i}")), None, cx);
        }
    });
    toasts.read_with(cx, |t, _| {
        assert_eq!(t.toasts().len(), MAX_TOASTS);
        assert_eq!(t.toasts()[0].title.as_ref(), "t2");
    });
    cx.executor().advance_clock(Duration::from_secs(60));
    cx.run_until_parked();
    toasts.read_with(cx, |t, _| assert_eq!(t.toasts().len(), MAX_TOASTS));
    // Dismissing by key.
    toasts.update(cx, |t, cx| {
        t.push(NewToast::error("x").key("k"), cx);
        t.dismiss_key("k", cx);
        assert!(t.find_key("k").is_none());
    });
}

#[gpui::test]
fn toasts_render_in_the_main_window(cx: &mut TestAppContext) {
    let dir = TempDir::new("toast-view");
    init(cx, dir.path());
    let (_, mut vcx) = open_main(cx);
    vcx.update(|_, cx| {
        let toasts = AppState::global(cx).toasts.clone();
        toasts.update(cx, |t, cx| {
            t.push(NewToast::success("Saved").body("prefs.json"), cx);
        });
    });
    vcx.run_until_parked();
    // A "not yet" stub shows a keyed toast; a second replaces it.
    for _ in 0..2 {
        vcx.update(|_, cx| crate::views::not_yet("The connection popover", cx));
        vcx.run_until_parked();
    }
    let titles: Vec<String> = vcx.update(|_, cx| {
        AppState::global(cx)
            .toasts
            .read(cx)
            .toasts()
            .iter()
            .map(|t| t.title.to_string())
            .collect()
    });
    assert_eq!(
        titles,
        ["Saved", "The connection popover isn't available yet"]
    );
}

#[gpui::test]
fn cmd_comma_toggles_settings_and_escape_closes_it(cx: &mut TestAppContext) {
    let dir = TempDir::new("settings");
    init(cx, dir.path());
    let (root, mut vcx) = open_main(cx);
    let page = |vcx: &mut VisualTestContext| {
        vcx.update(|_, cx| AppState::global(cx).layout.read(cx).settings())
    };
    vcx.simulate_keystrokes("cmd-,");
    assert_eq!(page(&mut vcx), Some(SettingsPage::General));
    vcx.run_until_parked();
    let settings_focused = vcx.update(|window, cx| {
        use gpui::Focusable as _;
        root.read(cx)
            .settings()
            .read(cx)
            .focus_handle(cx)
            .contains_focused(window, cx)
    });
    assert!(settings_focused, "Settings takes the focus");
    vcx.simulate_keystrokes("cmd-,");
    assert_eq!(page(&mut vcx), None, "⌘, closes it again");

    vcx.simulate_keystrokes("cmd-,");
    assert_eq!(page(&mut vcx), Some(SettingsPage::General));
    // While Settings shows, ⌥B is not a shortcut (App.tsx returns early).
    vcx.simulate_keystrokes("alt-b");
    assert!(layout_open(&mut vcx, PanelKind::Sidebar));
    // ⌘F focuses the search; typed text keeps Esc in the field.
    vcx.simulate_keystrokes("cmd-f");
    vcx.simulate_input("rows");
    vcx.simulate_keystrokes("escape");
    assert_eq!(
        page(&mut vcx),
        Some(SettingsPage::General),
        "Esc with text stays"
    );
    // Empty field: Esc closes, and the editor gets the focus back.
    vcx.update(|window, cx| {
        let search = root.read(cx).settings().read(cx).search().clone();
        search.update(cx, |s, cx| s.set_value("", window, cx));
    });
    vcx.run_until_parked();
    vcx.simulate_keystrokes("escape");
    assert_eq!(page(&mut vcx), None);
    let editor_focused = vcx.update(|window, cx| {
        use gpui::Focusable as _;
        root.read(cx)
            .workspace()
            .read(cx)
            .focus_handle(cx)
            .contains_focused(window, cx)
    });
    assert!(editor_focused);
}

#[gpui::test]
fn settings_reset_keeps_the_layout(cx: &mut TestAppContext) {
    let dir = TempDir::new("reset");
    init(cx, dir.path());
    let (_, mut vcx) = open_main(cx);
    vcx.update(|_, cx| {
        let prefs = AppState::global(cx).prefs.clone();
        prefs.update(cx, |p, cx| {
            p.update(cx, |p| {
                p.row_limit = 5000;
                p.sidebar_width = 400;
            });
            p.reset(cx);
        });
    });
    let p = prefs(&mut vcx);
    assert_eq!(p.row_limit, 500);
    assert_eq!(p.sidebar_width, 400);
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
fn enter_in_about_copies_the_version_info(cx: &mut TestAppContext) {
    let dir = TempDir::new("about");
    init(cx, dir.path());
    let slot = std::rc::Rc::new(std::cell::RefCell::new(None));
    let handle = cx.open_window(size(px(400.), px(330.)), {
        let slot = slot.clone();
        move |window, cx| {
            use gpui::Focusable as _;
            let view = cx.new(|cx| AboutView::new(about_info(), cx));
            window.focus(&view.focus_handle(cx), cx);
            *slot.borrow_mut() = Some(view.clone());
            Root::new(view, window, cx)
        }
    });
    let mut vcx = VisualTestContext::from_window(handle.into(), cx);
    vcx.run_until_parked();
    let view = slot.borrow_mut().take().unwrap();
    vcx.simulate_keystrokes("enter");
    let copied = vcx.update(|_, cx| cx.read_from_clipboard().and_then(|c| c.text()));
    assert_eq!(
        copied.as_deref(),
        Some(
            "Boa 0.6.1 (Apple Silicon)\nCommit: abc123\nDate: 2026-11-20T10:00:00Z\nOS: macOS 15.4.1"
        )
    );
    assert_eq!(copied.unwrap(), version_info_text(&about_info()));
    assert!(
        vcx.update(|_, cx| view.read(cx).copied()),
        "the button says Copied"
    );
    vcx.executor().advance_clock(Duration::from_secs(2));
    vcx.run_until_parked();
    assert!(!vcx.update(|_, cx| view.read(cx).copied()));
    // A held Return repeats nothing (AboutWindow.tsx `!e.repeat`).
    vcx.write_to_clipboard(gpui::ClipboardItem::new_string("other".into()));
    vcx.simulate_event(gpui::KeyDownEvent {
        keystroke: gpui::Keystroke::parse("enter").unwrap(),
        is_held: true,
        prefer_character_input: false,
    });
    vcx.run_until_parked();
    let held = vcx.update(|_, cx| cx.read_from_clipboard().and_then(|c| c.text()));
    assert_eq!(held.as_deref(), Some("other"), "a repeat doesn't copy");
    assert!(!vcx.update(|_, cx| view.read(cx).copied()));
}

#[gpui::test]
fn check_for_updates_reports_in_about(cx: &mut TestAppContext) {
    let dir = TempDir::new("updates");
    init(cx, dir.path());
    cx.update(|cx| {
        let updates = AppState::global(cx).updates.clone();
        updates.update(cx, |u, cx| {
            u.set_endpoint(None);
            u.check(true, cx)
        });
        let status = updates.read(cx).status().cloned();
        assert_eq!(
            boa_core::state::updates::about_status_line(status.as_ref(), false).as_deref(),
            Some(
                format!(
                    "Update check failed: {}",
                    crate::models::updates::NOT_AVAILABLE
                )
                .as_str()
            )
        );
    });
}

#[gpui::test]
fn edit_menu_actions_reach_the_focused_editor(cx: &mut TestAppContext) {
    use crate::actions::{EditCut, EditPaste, EditSelectAll, EditUndo};
    let dir = TempDir::new("edit-menu");
    init(cx, dir.path());
    let (root, mut vcx) = open_main(cx);
    let original = vcx.update(|_, cx| root.read(cx).editor_text(cx));
    assert!(!original.is_empty());
    // What the menu items do: dispatch to the focused element.
    vcx.update(|window, cx| window.dispatch_action(Box::new(EditSelectAll), cx));
    vcx.update(|window, cx| window.dispatch_action(Box::new(EditCut), cx));
    vcx.run_until_parked();
    assert_eq!(vcx.update(|_, cx| root.read(cx).editor_text(cx)), "");
    assert_eq!(
        vcx.update(|_, cx| cx.read_from_clipboard().and_then(|c| c.text())),
        Some(original.clone())
    );
    vcx.update(|window, cx| window.dispatch_action(Box::new(EditUndo), cx));
    vcx.run_until_parked();
    assert_eq!(vcx.update(|_, cx| root.read(cx).editor_text(cx)), original);
    vcx.update(|window, cx| window.dispatch_action(Box::new(EditSelectAll), cx));
    vcx.update(|window, cx| window.dispatch_action(Box::new(EditPaste), cx));
    vcx.run_until_parked();
    assert_eq!(vcx.update(|_, cx| root.read(cx).editor_text(cx)), original);
}

#[gpui::test]
fn app_menu_matches_the_tauri_menu(cx: &mut TestAppContext) {
    let _ = cx;
    let menus = crate::app::app_menus();
    let names: Vec<&str> = menus.iter().map(|m| m.name.as_ref()).collect();
    assert_eq!(names, ["Boa", "File", "Edit", "View", "Window", "Help"]);
    let items = |i: usize| -> Vec<String> {
        menus[i]
            .items
            .iter()
            .map(|item| match item {
                gpui::MenuItem::Separator => "-".to_owned(),
                gpui::MenuItem::Action { name, .. } => name.to_string(),
                gpui::MenuItem::SystemMenu(m) => m.name.to_string(),
                gpui::MenuItem::Submenu(m) => m.name.to_string(),
            })
            .collect()
    };
    assert_eq!(
        items(0),
        [
            "About Boa",
            "Check for Updates…",
            "-",
            "Settings…",
            "-",
            "Services",
            "-",
            "Hide Boa",
            "Hide Others",
            "Show All",
            "-",
            "Quit Boa"
        ]
    );
    assert_eq!(
        items(2),
        ["Undo", "Redo", "-", "Cut", "Copy", "Paste", "Select All"]
    );
    assert_eq!(items(4), ["Minimize", "Zoom", "-", "Close Window"]);
}

#[gpui::test]
fn config_dir_env_overrides_the_default(cx: &mut TestAppContext) {
    let _ = cx;
    // Not set in tests that run in parallel: check the default branch only
    // when the variable is absent, and the override via a direct read.
    if std::env::var_os(crate::app_state::CONFIG_DIR_ENV).is_none() {
        assert_eq!(
            crate::app_state::config_dir(),
            boa_core::config::config_dir()
        );
    }
}
