//! Connection dialog and password prompt tests: URL paste, validation,
//! saving to connections.json in a temp config dir (the password stays in
//! the file, no Keychain), editing, delete with confirmation, ⌘O, and the
//! password prompt opening / closing with the model.

use std::path::{Path, PathBuf};
use std::time::Duration;

use boa_core::connections::{self as store, ConnConfig, SshAuth, SslMode};
use gpui::{
    Context, Entity, IntoElement, Render, Subscription, TestAppContext, VisualTestContext, Window,
    div, px, size,
};
use gpui_component::Root;

use super::*;
use crate::app_state::AppState;
use crate::views::password_prompt;

struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> Self {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static N: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "boa-app-dialog-{name}-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }
    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// An empty window content that opens the password prompt like the root view does.
struct Blank {
    _prompt: Option<Subscription>,
}

impl Render for Blank {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size_full()
    }
}

fn init(cx: &mut TestAppContext, dir: &Path) -> VisualTestContext {
    cx.executor().allow_parking();
    cx.update(|cx| {
        gpui_component::init(cx);
        crate::theme::init(cx);
        crate::keymap::init(cx);
        AppState::init(dir, 1280., cx).unwrap();
    });
    let handle = cx.open_window(size(px(1280.), px(820.)), |window, cx| {
        let blank = cx.new(|cx| Blank {
            _prompt: password_prompt::subscribe(window, cx),
        });
        Root::new(blank, window, cx)
    });
    let vcx = VisualTestContext::from_window(handle.into(), cx);
    vcx.run_until_parked();
    vcx
}

/// Waits (really) for the backend's file IO.
fn wait_for(vcx: &mut VisualTestContext, what: &str, f: impl Fn(&mut VisualTestContext) -> bool) {
    for _ in 0..500 {
        vcx.run_until_parked();
        if f(vcx) {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("timed out waiting for {what}");
}

fn open(vcx: &mut VisualTestContext, edit: Option<&str>) -> Entity<ConnectionDialog> {
    let edit = edit.map(str::to_owned);
    let view = vcx.update(|window, cx| open_view(edit, window, cx).unwrap());
    vcx.run_until_parked();
    view
}

fn value(vcx: &mut VisualTestContext, view: &Entity<ConnectionDialog>, field: Field) -> String {
    vcx.update(|_, cx| view.read(cx).input(field).read(cx).value().to_string())
}

fn type_into(
    vcx: &mut VisualTestContext,
    view: &Entity<ConnectionDialog>,
    field: Field,
    text: &str,
) {
    vcx.update(|window, cx| view.update(cx, |v, cx| v.set_field_text(field, text, window, cx)));
    vcx.run_until_parked();
}

fn has_dialog(vcx: &mut VisualTestContext) -> bool {
    vcx.update(|window, cx| window.has_active_dialog(cx))
}

fn saved_json(dir: &Path) -> serde_json::Value {
    serde_json::from_slice(&std::fs::read(dir.join("connections.json")).unwrap()).unwrap()
}

fn seed(dir: &Path, id: &str, password: Option<&str>) -> ConnConfig {
    let cfg = ConnConfig {
        id: id.into(),
        name: "Seeded".into(),
        host: "127.0.0.1".into(),
        port: 1,
        user: "me".into(),
        database: "app".into(),
        ssl_mode: SslMode::Require,
        read_only: false,
        environment: None,
        color: None,
        ssh: None,
    };
    store::upsert(
        dir,
        cfg.clone(),
        password.map(str::to_owned),
        password.is_some(),
    )
    .unwrap();
    cfg
}

#[gpui::test]
fn cmd_o_opens_the_dialog_in_the_main_window(cx: &mut TestAppContext) {
    let dir = TempDir::new("cmd-o");
    cx.executor().allow_parking();
    cx.update(|cx| {
        gpui_component::init(cx);
        crate::theme::init(cx);
        crate::keymap::init(cx);
        crate::app::register_app_actions(cx);
        AppState::init(dir.path(), 1280., cx).unwrap();
    });
    let handle = cx.open_window(size(px(1280.), px(820.)), |window, cx| {
        let root = cx.new(|cx| crate::views::root::RootView::new(window, cx));
        root.update(cx, |root, cx| root.focus_editor(window, cx));
        Root::new(root, window, cx)
    });
    let mut vcx = VisualTestContext::from_window(handle.into(), cx);
    vcx.run_until_parked();
    vcx.update(|window, _| window.refresh());
    vcx.run_until_parked();
    assert!(!has_dialog(&mut vcx));
    vcx.simulate_keystrokes("cmd-o");
    vcx.run_until_parked();
    assert!(has_dialog(&mut vcx));
    // Esc closes it; the root view's prompt subscription is live too.
    vcx.simulate_keystrokes("escape");
    vcx.run_until_parked();
    assert!(!has_dialog(&mut vcx));
    let conns = vcx.update(|_, cx| AppState::global(cx).connections.clone());
    seed(dir.path(), "c1", None);
    vcx.update(|_, cx| conns.update(cx, |c, cx| c.reload(cx)));
    wait_for(&mut vcx, "the list", |vcx| {
        vcx.update(|_, cx| conns.read(cx).get("c1").is_some())
    });
    vcx.update(|_, cx| conns.update(cx, |c, cx| c.ask_password("c1", None, cx)));
    vcx.run_until_parked();
    assert!(has_dialog(&mut vcx));
}

#[gpui::test]
fn the_new_form_starts_blank_and_focuses_the_name(cx: &mut TestAppContext) {
    let dir = TempDir::new("blank");
    let mut vcx = init(cx, dir.path());
    let view = open(&mut vcx, None);
    assert!(has_dialog(&mut vcx));
    let cfg = vcx.update(|_, cx| view.read(cx).config().clone());
    assert_eq!(
        (
            cfg.host.as_str(),
            cfg.port,
            cfg.user.as_str(),
            cfg.database.as_str()
        ),
        ("localhost", 5432, "postgres", "postgres")
    );
    assert_eq!(cfg.ssl_mode, SslMode::Prefer);
    assert!(vcx.update(|_, cx| view.read(cx).save_password()));
    assert!(vcx.update(|_, cx| view.read(cx).valid()));
    let name_focused = vcx.update(|window, cx| {
        view.read(cx)
            .input(Field::Name)
            .read(cx)
            .focus_handle(cx)
            .is_focused(window)
    });
    assert!(name_focused);
    // The Name placeholder follows database@host.
    type_into(&mut vcx, &view, Field::Host, "db.example.com");
    let placeholder = vcx.update(|_, cx| view.read(cx).name_placeholder());
    assert_eq!(placeholder, "postgres@db.example.com");
}

#[gpui::test]
fn pasting_a_url_fills_the_fields(cx: &mut TestAppContext) {
    let dir = TempDir::new("url");
    let mut vcx = init(cx, dir.path());
    let view = open(&mut vcx, None);
    // Typed for real into the focused URL field.
    let url_focus = vcx.update(|_, cx| view.read(cx).input(Field::Url).read(cx).focus_handle(cx));
    vcx.update(|window, cx| window.focus(&url_focus, cx));
    vcx.simulate_input("postgresql://ann:p%40ss@db.example.com:6543/shop?sslmode=verify-ca");
    vcx.run_until_parked();
    let cfg = vcx.update(|_, cx| view.read(cx).config().clone());
    assert_eq!(cfg.host, "db.example.com");
    assert_eq!(cfg.port, 6543);
    assert_eq!(cfg.user, "ann");
    assert_eq!(cfg.database, "shop");
    assert_eq!(cfg.ssl_mode, SslMode::VerifyFull);
    assert_eq!(value(&mut vcx, &view, Field::Host), "db.example.com");
    assert_eq!(value(&mut vcx, &view, Field::Port), "6543");
    assert_eq!(value(&mut vcx, &view, Field::User), "ann");
    assert_eq!(value(&mut vcx, &view, Field::Database), "shop");
    assert_eq!(value(&mut vcx, &view, Field::Password), "p@ss");
    assert_eq!(
        vcx.update(|_, cx| view.read(cx).typed_password()),
        Some("p@ss".to_owned())
    );
    // Something that is not such a URL changes nothing.
    type_into(&mut vcx, &view, Field::Url, "mysql://root@other/x");
    assert_eq!(
        vcx.update(|_, cx| view.read(cx).config().host.clone()),
        "db.example.com"
    );
}

#[gpui::test]
fn validation_disables_test_and_save(cx: &mut TestAppContext) {
    let dir = TempDir::new("valid");
    let mut vcx = init(cx, dir.path());
    let view = open(&mut vcx, None);
    let valid = |vcx: &mut VisualTestContext| vcx.update(|_, cx| view.read(cx).valid());
    type_into(&mut vcx, &view, Field::Host, "  ");
    assert!(!valid(&mut vcx));
    type_into(&mut vcx, &view, Field::Host, "localhost");
    type_into(&mut vcx, &view, Field::User, "");
    assert!(!valid(&mut vcx));
    type_into(&mut vcx, &view, Field::User, "me");
    assert!(valid(&mut vcx));
    // The port takes digits only; empty or out of range is invalid.
    type_into(&mut vcx, &view, Field::Port, "54a32");
    assert_eq!(value(&mut vcx, &view, Field::Port), "5432");
    type_into(&mut vcx, &view, Field::Port, "");
    assert!(!valid(&mut vcx));
    type_into(&mut vcx, &view, Field::Port, "70000");
    assert!(!valid(&mut vcx));
    type_into(&mut vcx, &view, Field::Port, "5433");
    assert!(valid(&mut vcx));
    // An enabled tunnel needs a host, and a key for key-file auth.
    vcx.update(|_, cx| view.update(cx, |v, cx| v.set_ssh_enabled(true, cx)));
    assert!(!valid(&mut vcx));
    type_into(&mut vcx, &view, Field::SshHost, "bastion");
    assert!(valid(&mut vcx));
    vcx.update(|_, cx| view.update(cx, |v, cx| v.set_ssh_auth(SshAuth::KeyFile, cx)));
    assert!(!valid(&mut vcx));
    type_into(&mut vcx, &view, Field::SshKey, "~/.ssh/id_ed25519");
    assert!(valid(&mut vcx));
    // Saving while invalid does nothing.
    type_into(&mut vcx, &view, Field::Host, "");
    vcx.update(|window, cx| view.update(cx, |v, cx| v.submit(window, cx)));
    vcx.run_until_parked();
    assert!(has_dialog(&mut vcx));
    assert!(!dir.path().join("connections.json").exists());
}

#[gpui::test]
fn saving_writes_connections_json_with_the_password(cx: &mut TestAppContext) {
    let dir = TempDir::new("save");
    let mut vcx = init(cx, dir.path());
    let view = open(&mut vcx, None);
    type_into(&mut vcx, &view, Field::Host, "127.0.0.1");
    type_into(&mut vcx, &view, Field::Port, "1");
    type_into(&mut vcx, &view, Field::User, "ann");
    type_into(&mut vcx, &view, Field::Password, "s3cret");
    type_into(&mut vcx, &view, Field::Database, "shop");
    vcx.update(|_, cx| {
        view.update(cx, |v, cx| {
            v.set_read_only(true, cx);
            v.set_environment(Some(boa_core::connections::Environment::Staging), cx);
            v.set_color(Some(ConnectionColor::Purple), cx);
        })
    });
    let id = vcx.update(|_, cx| view.read(cx).config().id.clone());
    vcx.update(|window, cx| view.update(cx, |v, cx| v.submit(window, cx)));
    wait_for(&mut vcx, "the dialog to close", |vcx| !has_dialog(vcx));
    let json = saved_json(dir.path());
    let saved = &json.as_array().unwrap()[0];
    assert_eq!(saved["id"], id.as_str());
    // Name left empty: database@host.
    assert_eq!(saved["name"], "shop@127.0.0.1");
    assert_eq!(saved["host"], "127.0.0.1");
    assert_eq!(saved["port"], 1);
    assert_eq!(saved["user"], "ann");
    assert_eq!(saved["database"], "shop");
    assert_eq!(saved["sslMode"], "prefer");
    assert_eq!(saved["readOnly"], true);
    assert_eq!(saved["environment"], "staging");
    assert_eq!(saved["color"], "purple");
    // In the file, as the Tauri app keeps it (no Keychain).
    assert_eq!(saved["password"], "s3cret");
    assert!(saved.get("ssh").is_none());
    // The model has it, expanded in the explorer, and it connects.
    vcx.update(|_, cx| {
        let state = AppState::global(cx);
        let c = state.connections.read(cx).get(&id).cloned().unwrap();
        assert!(c.has_password);
        assert!(state.prefs.read(cx).get().is_expanded(&id));
    });
    wait_for(&mut vcx, "the connect attempt", |vcx| {
        vcx.update(|_, cx| {
            AppState::global(cx)
                .connections
                .read(cx)
                .state_of(&id)
                .as_str()
                == "failed"
        })
    });
}

#[gpui::test]
fn not_saving_the_password_leaves_it_out(cx: &mut TestAppContext) {
    let dir = TempDir::new("nosave");
    let mut vcx = init(cx, dir.path());
    let view = open(&mut vcx, None);
    type_into(&mut vcx, &view, Field::Host, "127.0.0.1");
    type_into(&mut vcx, &view, Field::Port, "1");
    type_into(&mut vcx, &view, Field::Password, "s3cret");
    vcx.update(|_, cx| view.update(cx, |v, cx| v.set_save_password(false, cx)));
    vcx.update(|_, cx| view.update(cx, |v, cx| v.set_ssh_enabled(true, cx)));
    type_into(&mut vcx, &view, Field::SshHost, " bastion ");
    vcx.update(|window, cx| view.update(cx, |v, cx| v.submit(window, cx)));
    wait_for(&mut vcx, "the dialog to close", |vcx| !has_dialog(vcx));
    let saved = saved_json(dir.path())[0].clone();
    assert!(saved.get("password").is_none());
    // The tunnel is cleaned (trimmed, empty optional fields left out).
    assert_eq!(
        saved["ssh"],
        serde_json::json!({"enabled": true, "host": "bastion", "user": "", "auth": "agent"})
    );
}

#[gpui::test]
fn editing_keeps_the_stored_password(cx: &mut TestAppContext) {
    let dir = TempDir::new("edit");
    seed(dir.path(), "c1", Some("pw"));
    let mut vcx = init(cx, dir.path());
    wait_for(&mut vcx, "the list", |vcx| {
        vcx.update(|_, cx| {
            AppState::global(cx)
                .connections
                .read(cx)
                .get("c1")
                .is_some()
        })
    });
    let view = open(&mut vcx, Some("c1"));
    let placeholder = vcx.update(|_, cx| view.read(cx).password_placeholder());
    assert_eq!(placeholder, "<stored>");
    assert_eq!(value(&mut vcx, &view, Field::Name), "Seeded");
    assert_eq!(vcx.update(|_, cx| view.read(cx).typed_password()), None);
    assert!(vcx.update(|_, cx| view.read(cx).editing().is_some()));
    type_into(&mut vcx, &view, Field::Name, "Renamed");
    vcx.update(|window, cx| view.update(cx, |v, cx| v.submit(window, cx)));
    wait_for(&mut vcx, "the dialog to close", |vcx| !has_dialog(vcx));
    let saved = saved_json(dir.path())[0].clone();
    assert_eq!(saved["name"], "Renamed");
    assert_eq!(saved["password"], "pw");
    assert_eq!(saved["sslMode"], "require");
    // Editing doesn't connect.
    assert_eq!(
        vcx.update(|_, cx| AppState::global(cx)
            .connections
            .read(cx)
            .state_of("c1")
            .as_str()),
        "disconnected"
    );
}

#[gpui::test]
fn delete_asks_with_a_second_click(cx: &mut TestAppContext) {
    let dir = TempDir::new("delete");
    seed(dir.path(), "c1", Some("pw"));
    seed(dir.path(), "c2", None);
    let mut vcx = init(cx, dir.path());
    wait_for(&mut vcx, "the list", |vcx| {
        vcx.update(|_, cx| AppState::global(cx).connections.read(cx).saved.len() == 2)
    });
    let view = open(&mut vcx, Some("c1"));
    vcx.update(|window, cx| view.update(cx, |v, cx| v.remove(window, cx)));
    vcx.run_until_parked();
    assert!(vcx.update(|_, cx| view.read(cx).confirming_delete()));
    assert_eq!(saved_json(dir.path()).as_array().unwrap().len(), 2);
    vcx.update(|window, cx| view.update(cx, |v, cx| v.remove(window, cx)));
    wait_for(&mut vcx, "the dialog to close", |vcx| !has_dialog(vcx));
    let json = saved_json(dir.path());
    let ids: Vec<&str> = json
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, ["c2"]);
    // The explorer's confirm_delete asks in an alert first.
    vcx.update(|window, cx| confirm_delete("c2".into(), window, cx));
    vcx.run_until_parked();
    assert!(has_dialog(&mut vcx));
    assert_eq!(saved_json(dir.path()).as_array().unwrap().len(), 1);
}

#[gpui::test]
fn the_password_prompt_follows_the_model(cx: &mut TestAppContext) {
    let dir = TempDir::new("prompt");
    seed(dir.path(), "c1", None);
    let mut vcx = init(cx, dir.path());
    wait_for(&mut vcx, "the list", |vcx| {
        vcx.update(|_, cx| {
            AppState::global(cx)
                .connections
                .read(cx)
                .get("c1")
                .is_some()
        })
    });
    let conns = vcx.update(|_, cx| AppState::global(cx).connections.clone());
    vcx.update(|_, cx| conns.update(cx, |c, cx| c.ask_password("c1", Some("bad".into()), cx)));
    vcx.run_until_parked();
    assert!(has_dialog(&mut vcx));
    // Cancel dismisses it in the model too.
    vcx.simulate_keystrokes("escape");
    vcx.run_until_parked();
    assert!(!has_dialog(&mut vcx));
    assert!(vcx.update(|_, cx| conns.read(cx).password_prompt().is_none()));
    // Opened again, then the model no longer asks (connected elsewhere): it closes.
    vcx.update(|_, cx| conns.update(cx, |c, cx| c.ask_password("c1", None, cx)));
    vcx.run_until_parked();
    assert!(has_dialog(&mut vcx));
    vcx.update(|_, cx| conns.update(cx, |c, cx| c.dismiss_password_prompt(cx)));
    vcx.run_until_parked();
    assert!(!has_dialog(&mut vcx));
}

#[gpui::test]
fn the_prompt_connects_with_the_typed_password(cx: &mut TestAppContext) {
    let dir = TempDir::new("prompt-submit");
    seed(dir.path(), "c1", None);
    let mut vcx = init(cx, dir.path());
    wait_for(&mut vcx, "the list", |vcx| {
        vcx.update(|_, cx| {
            AppState::global(cx)
                .connections
                .read(cx)
                .get("c1")
                .is_some()
        })
    });
    let prompt = vcx.update(|window, cx| {
        let conns = AppState::global(cx).connections.clone();
        conns.update(cx, |c, cx| c.ask_password("c1", None, cx));
        password_prompt::open("c1".into(), window, cx).unwrap()
    });
    vcx.run_until_parked();
    let focus = vcx.update(|_, cx| prompt.read(cx).input().read(cx).focus_handle(cx));
    assert!(vcx.update(|window, _| focus.is_focused(window)));
    vcx.simulate_input("typed");
    vcx.update(|_, cx| prompt.update(cx, |p, cx| p.set_save(true, cx)));
    vcx.update(|window, cx| prompt.update(cx, |p, cx| p.submit(window, cx)));
    // Saved first (the password lands in connections.json), then the
    // connect fails (nothing listens on port 1) with a status toast.
    wait_for(&mut vcx, "the save", |_| {
        saved_json(dir.path())[0].get("password").is_some()
    });
    assert_eq!(saved_json(dir.path())[0]["password"], "typed");
    wait_for(&mut vcx, "the connect attempt", |vcx| {
        vcx.update(|_, cx| {
            AppState::global(cx)
                .connections
                .read(cx)
                .state_of("c1")
                .as_str()
                == "failed"
        })
    });
}

/// ConnectionDialog.tsx: Enter in a field submits the form, and Enter in
/// the password prompt connects (both through the real keymap).
#[gpui::test]
fn enter_saves_the_dialog_and_connects_from_the_prompt(cx: &mut TestAppContext) {
    let dir = TempDir::new("enter");
    seed(dir.path(), "c1", None);
    let mut vcx = init(cx, dir.path());
    let view = open(&mut vcx, None);
    type_into(&mut vcx, &view, Field::Host, "127.0.0.1");
    type_into(&mut vcx, &view, Field::Port, "1");
    type_into(&mut vcx, &view, Field::Database, "shop");
    // The name field has the focus (new form).
    vcx.simulate_keystrokes("enter");
    wait_for(&mut vcx, "the dialog to close", |vcx| !has_dialog(vcx));
    wait_for(&mut vcx, "the save", |_| {
        saved_json(dir.path())
            .as_array()
            .is_some_and(|a| a.iter().any(|c| c["database"] == "shop"))
    });

    wait_for(&mut vcx, "the list", |vcx| {
        vcx.update(|_, cx| {
            AppState::global(cx)
                .connections
                .read(cx)
                .get("c1")
                .is_some()
        })
    });
    let prompt = vcx.update(|window, cx| {
        let conns = AppState::global(cx).connections.clone();
        conns.update(cx, |c, cx| c.ask_password("c1", None, cx));
        password_prompt::open("c1".into(), window, cx).unwrap()
    });
    vcx.run_until_parked();
    let focus = vcx.update(|_, cx| prompt.read(cx).input().read(cx).focus_handle(cx));
    assert!(vcx.update(|window, _| focus.is_focused(window)));
    vcx.simulate_input("typed");
    vcx.simulate_keystrokes("enter");
    wait_for(&mut vcx, "the connect attempt", |vcx| {
        vcx.update(|_, cx| {
            AppState::global(cx)
                .connections
                .read(cx)
                .state_of("c1")
                .as_str()
                == "failed"
        })
    });
}
