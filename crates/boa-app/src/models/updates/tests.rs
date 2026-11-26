//! The updates model end to end against a local feed and a THROWAWAY key
//! (updater/tests.rs): check → download → verify → "Restart to update"
//! (pill, About window) → the open-transaction guard → install → quit;
//! tampered downloads, broken feeds, and no network without an endpoint in
//! debug builds.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use boa_core::state::guards::CloseAction;
use boa_core::state::updates::{
    INSTALL_FAILED_TITLE, about_status_line, update_pill_label, update_pill_title,
};
use gpui::{Entity, Modifiers, TestAppContext, VisualTestContext};

use super::*;
use crate::models::catalogs::DbKey;
use crate::models::transactions::TxState;
use crate::tests::{FAKE_CONNECTION, TempDir, init, open_main};
use crate::updater::tests::{PAYLOAD, SIG_V, TEST_PUBKEY, manifest, serve};
use crate::views::guard_dialogs::{self, GuardAnswer, GuardKind};

/// The version SIG_V was signed for (newer than this build).
const SIGNED: &str = "1.2.3";

fn draw(vcx: &mut VisualTestContext) {
    vcx.run_until_parked();
    vcx.update(|window, cx| window.draw(cx).clear(cx));
    vcx.run_until_parked();
}

fn wait_until(vcx: &mut VisualTestContext, what: &str, f: impl Fn(&mut VisualTestContext) -> bool) {
    let start = Instant::now();
    while !f(vcx) {
        assert!(
            start.elapsed() < Duration::from_secs(20),
            "timed out: {what}"
        );
        std::thread::sleep(Duration::from_millis(10));
        draw(vcx);
    }
}

/// A feed announcing SIGNED with \`payload\` as its download.
fn feed(payload: Vec<u8>) -> String {
    let files = serve(vec![("/Boa.app.tar.gz", 200, payload)]);
    let feed = serve(vec![(
        "/latest.json",
        200,
        manifest(&files.base, SIGNED, "Boa.app.tar.gz", SIG_V),
    )]);
    format!("{}/latest.json", feed.base)
}

fn updates(cx: &mut TestAppContext, endpoint: Option<String>) -> Entity<Updates> {
    cx.update(|cx| {
        let updates = AppState::global(cx).updates.clone();
        updates.update(cx, |u, _| {
            u.set_endpoint(endpoint);
            u.set_pubkey(TEST_PUBKEY);
        });
        updates
    })
}

fn check(cx: &mut TestAppContext, updates: &Entity<Updates>) -> Option<String> {
    updates.update(cx, |u, cx| u.check(true, cx));
    cx.run_until_parked();
    updates.read_with(cx, |u, _| about_status_line(u.status(), false))
}

/// What the installer was given (version, bytes); \`Ok\` without installing.
type Installed = Arc<Mutex<Vec<(String, Vec<u8>)>>>;

fn fake_installer(updates: &Entity<Updates>, cx: &mut TestAppContext) -> Installed {
    let seen: Installed = Arc::default();
    let log = seen.clone();
    updates.update(cx, |u, _| {
        u.set_installer(Some(Arc::new(move |v: &VerifiedUpdate| {
            log.lock()
                .unwrap()
                .push((v.version().to_owned(), v.bytes().to_vec()));
            Ok(())
        })))
    });
    seen
}

fn pill_text(vcx: &mut VisualTestContext) -> Option<String> {
    vcx.update(|_, cx| {
        let u = AppState::global(cx).updates.read(cx);
        let version = u.ready_version()?.to_owned();
        Some(update_pill_label(&version, u.pill_restarting()))
    })
}

fn click_pill(vcx: &mut VisualTestContext) {
    let bounds = vcx.debug_bounds("update-pill").expect("the pill shows");
    vcx.simulate_click(bounds.center(), Modifiers::none());
    draw(vcx);
}

fn guard_title(vcx: &mut VisualTestContext) -> Option<&'static str> {
    vcx.update(|_, cx| guard_dialogs::active(cx).map(|g| g.read(cx).kind().title()))
}

fn answer(vcx: &mut VisualTestContext, answer: GuardAnswer) {
    let g = vcx
        .update(|_, cx| guard_dialogs::active(cx))
        .expect("a guard dialog");
    vcx.update(|window, cx| g.update(cx, |d, cx| d.answer(answer, window, cx)));
    draw(vcx);
}

fn is_quitting(vcx: &mut VisualTestContext) -> bool {
    vcx.update(|_, cx| crate::app::is_quitting(cx))
}

fn open_transaction(vcx: &mut VisualTestContext) {
    let key = DbKey {
        connection_id: FAKE_CONNECTION.into(),
        database: "boa_dev".into(),
    };
    vcx.update(|_, cx| {
        let t = AppState::global(cx).transactions.clone();
        t.update(cx, |t, cx| t.note_outcome(&key, Some(TxState::Active), cx));
    });
}

#[test]
fn the_release_feed_is_the_default_only_in_release_builds() {
    assert_eq!(endpoint_for(false, None).as_deref(), Some(UPDATE_ENDPOINT));
    assert_eq!(endpoint_for(true, None), None, "no network in debug builds");
    let local = Some("http://127.0.0.1:8765/latest.json".to_owned());
    assert_eq!(endpoint_for(true, local.clone()), local);
    assert_eq!(endpoint_for(false, local.clone()), local);
    // Blank means unset.
    assert_eq!(endpoint_for(true, Some("  ".into())), None);
    assert_eq!(
        endpoint_for(false, Some(String::new())).as_deref(),
        Some(UPDATE_ENDPOINT)
    );
    assert_eq!(
        UPDATE_ENDPOINT,
        "https://github.com/sharithg1/boa/releases/latest/download/latest.json"
    );
}

#[gpui::test]
fn without_an_endpoint_a_check_reports_and_fetches_nothing(cx: &mut TestAppContext) {
    let dir = TempDir::new("updates-none");
    init(cx, dir.path());
    let updates = updates(cx, None);
    updates.update(cx, |u, cx| u.check(true, cx));
    // Answered at once: nothing was spawned.
    let (line, busy) = updates.read_with(cx, |u, _| {
        (about_status_line(u.status(), false), u.check.is_some())
    });
    assert_eq!(line, Some(format!("Update check failed: {NOT_AVAILABLE}")));
    assert!(!busy);
    // The About view shows the same line.
    let line = cx.update(|cx| {
        let view = cx
            .new(|cx| crate::views::about::AboutView::new(crate::views::about::current_info(), cx));
        view.read(cx).status_line(cx)
    });
    assert_eq!(line, Some(format!("Update check failed: {NOT_AVAILABLE}")));
}

#[gpui::test]
fn a_newer_release_is_downloaded_verified_and_offered(cx: &mut TestAppContext) {
    let dir = TempDir::new("updates-ready");
    init(cx, dir.path());
    let updates = updates(cx, Some(feed(PAYLOAD.to_vec())));
    // Progress is reported while it downloads.
    let seen = Arc::new(Mutex::new(Vec::new()));
    let _watch = cx.update(|cx| {
        let seen = seen.clone();
        cx.observe(&updates, move |u, cx| {
            if let Some(p) = u.read(cx).download_progress() {
                seen.lock().unwrap().push(p);
            }
        })
    });
    assert_eq!(
        check(cx, &updates).as_deref(),
        Some("Boa 1.2.3 is ready to install.")
    );
    let len = PAYLOAD.len() as u64;
    assert_eq!(seen.lock().unwrap().last(), Some(&(len, Some(len))));
    let (ready, status, progress) = updates.read_with(cx, |u, _| {
        (
            u.ready_version().map(str::to_owned),
            u.status().cloned().unwrap(),
            u.download_progress(),
        )
    });
    assert_eq!(ready.as_deref(), Some(SIGNED));
    assert!(status.manual);
    assert_eq!(status.current, crate::app::VERSION);
    assert_eq!(progress, None, "done downloading");
    // A later check reports it again without fetching.
    updates.update(cx, |u, _| {
        u.set_endpoint(Some("http://127.0.0.1:9/nothing".into()))
    });
    assert_eq!(
        check(cx, &updates).as_deref(),
        Some("Boa 1.2.3 is ready to install.")
    );
    // The status bar pill, as StatusBar.tsx words it.
    let (_root, mut vcx) = open_main(cx);
    draw(&mut vcx);
    assert!(vcx.debug_bounds("update-pill").is_some(), "the pill shows");
    assert_eq!(
        pill_text(&mut vcx).as_deref(),
        Some("Restart to update to 1.2.3")
    );
    assert_eq!(
        update_pill_title(SIGNED),
        "Boa 1.2.3 is downloaded. Restart to install it; your tabs come back."
    );
}

#[gpui::test]
fn a_tampered_download_is_rejected(cx: &mut TestAppContext) {
    let dir = TempDir::new("updates-tampered");
    init(cx, dir.path());
    let mut tampered = PAYLOAD.to_vec();
    tampered[3] ^= 1;
    let updates = updates(cx, Some(feed(tampered)));
    let line = check(cx, &updates).unwrap();
    assert!(
        line.starts_with("Update check failed: the update's signature is not valid"),
        "{line}"
    );
    assert!(updates.read_with(cx, |u, _| u.ready_version().is_none()));
    // Signed with another key: rejected too.
    updates.update(cx, |u, _| {
        u.set_endpoint(Some(feed(PAYLOAD.to_vec())));
        u.set_pubkey(crate::updater::PRODUCTION_PUBKEY);
    });
    let line = check(cx, &updates).unwrap();
    assert!(
        line.starts_with("Update check failed: the update's signature is not valid"),
        "{line}"
    );
    let (_root, mut vcx) = open_main(cx);
    draw(&mut vcx);
    assert!(vcx.debug_bounds("update-pill").is_none(), "no pill");
}

#[gpui::test]
fn the_same_version_and_no_content_are_up_to_date(cx: &mut TestAppContext) {
    let dir = TempDir::new("updates-same");
    init(cx, dir.path());
    let server = serve(vec![
        (
            "/same.json",
            200,
            manifest("https://example.test", crate::app::VERSION, "x", "s"),
        ),
        ("/none.json", 204, Vec::new()),
    ]);
    let updates = updates(cx, Some(format!("{}/same.json", server.base)));
    assert_eq!(check(cx, &updates).as_deref(), Some("Boa is up to date."));
    updates.update(cx, |u, _| {
        u.set_endpoint(Some(format!("{}/none.json", server.base)))
    });
    assert_eq!(check(cx, &updates).as_deref(), Some("Boa is up to date."));
    let (_root, mut vcx) = open_main(cx);
    draw(&mut vcx);
    assert!(vcx.debug_bounds("update-pill").is_none());
}

#[gpui::test]
fn broken_feeds_fail(cx: &mut TestAppContext) {
    let dir = TempDir::new("updates-bad");
    init(cx, dir.path());
    let server = serve(vec![
        ("/bad.json", 200, b"{\"version\":".to_vec()),
        (
            "/noversion.json",
            200,
            manifest("https://example.test", "one.two", "x", "s"),
        ),
        (
            "/insecure.json",
            200,
            manifest("http://example.test", "99.0.0", "x", "s"),
        ),
    ]);
    let base = server.base.clone();
    let updates = updates(cx, Some(format!("{base}/bad.json")));
    let line = check(cx, &updates).unwrap();
    assert!(
        line.starts_with("Update check failed: invalid update manifest: "),
        "{line}"
    );
    let mut line_for = |path: &str| {
        updates.update(cx, |u, _| u.set_endpoint(Some(format!("{base}{path}"))));
        check(cx, &updates).unwrap()
    };
    let line = line_for("/noversion.json");
    assert!(
        line.starts_with("Update check failed: invalid version"),
        "{line}"
    );
    assert_eq!(
        line_for("/gone.json"),
        format!("Update check failed: {base}/gone.json answered HTTP 404")
    );
    assert_eq!(
        line_for("/insecure.json"),
        "Update check failed: refusing to update over plain HTTP: http://example.test/x"
    );
}

#[gpui::test]
fn restart_with_an_open_transaction_asks_first(cx: &mut TestAppContext) {
    let dir = TempDir::new("updates-restart-tx");
    init(cx, dir.path());
    let updates = updates(cx, Some(feed(PAYLOAD.to_vec())));
    let installed = fake_installer(&updates, cx);
    check(cx, &updates);
    let (_root, mut vcx) = open_main(cx);
    open_transaction(&mut vcx);
    draw(&mut vcx);

    // The pill asks; it says "Restarting…" meanwhile.
    click_pill(&mut vcx);
    assert_eq!(
        guard_title(&mut vcx),
        Some("Restart to update with an open transaction?")
    );
    vcx.update(|_, cx| {
        let g = guard_dialogs::active(cx).unwrap();
        let GuardKind::CloseTx { action, open } = g.read(cx).kind() else {
            panic!("a close prompt");
        };
        assert_eq!(*action, CloseAction::Update);
        assert_eq!(action.confirm_label(), "Roll back and restart");
        assert_eq!(open.len(), 1);
    });
    assert_eq!(pill_text(&mut vcx).as_deref(), Some("Restarting…"));
    // A second click does nothing while it asks.
    vcx.update(|_, cx| updates.update(cx, |u, cx| u.restart_to_update(true, cx)));
    draw(&mut vcx);

    // Cancel: nothing installed, the pill is back.
    answer(&mut vcx, GuardAnswer::Cancel);
    assert!(installed.lock().unwrap().is_empty());
    assert!(!is_quitting(&mut vcx));
    assert_eq!(
        pill_text(&mut vcx).as_deref(),
        Some("Restart to update to 1.2.3")
    );
    assert!(!vcx.update(|_, cx| updates.read(cx).restarting()));

    // Again, from the About window this time: "Roll back and restart"
    // rolls back, installs the verified download and quits.
    vcx.update(|_, cx| {
        let view = cx
            .new(|cx| crate::views::about::AboutView::new(crate::views::about::current_info(), cx));
        view.update(cx, |v, cx| v.install(cx));
        assert_eq!(
            view.read(cx).status_line(cx).as_deref(),
            Some("Restarting… Check the Boa window if it asks about open transactions.")
        );
    });
    draw(&mut vcx);
    assert_eq!(
        guard_title(&mut vcx),
        Some("Restart to update with an open transaction?")
    );
    // The pill keeps its own label (it was not clicked).
    assert_eq!(
        pill_text(&mut vcx).as_deref(),
        Some("Restart to update to 1.2.3")
    );
    vcx.executor().allow_parking();
    answer(&mut vcx, GuardAnswer::Confirm);
    wait_until(&mut vcx, "the quit", is_quitting);
    let installed = installed.lock().unwrap().clone();
    assert_eq!(installed, vec![(SIGNED.to_owned(), PAYLOAD.to_vec())]);
}

#[gpui::test]
fn a_failed_install_says_so_and_can_be_retried(cx: &mut TestAppContext) {
    let dir = TempDir::new("updates-install-fails");
    init(cx, dir.path());
    let updates = updates(cx, Some(feed(PAYLOAD.to_vec())));
    check(cx, &updates);
    let (_root, mut vcx) = open_main(cx);
    draw(&mut vcx);
    // The real installer: a test binary is not in an .app bundle. Nothing
    // open: no question.
    click_pill(&mut vcx);
    wait_until(&mut vcx, "the install", |vcx| {
        !vcx.update(|_, cx| updates.read(cx).restarting())
    });
    assert!(guard_title(&mut vcx).is_none());
    assert!(!is_quitting(&mut vcx));
    let toast = vcx.update(|_, cx| {
        AppState::global(cx)
            .toasts
            .read(cx)
            .toasts()
            .last()
            .cloned()
            .expect("a toast")
    });
    assert_eq!(toast.title.as_ref(), INSTALL_FAILED_TITLE);
    let body = toast.body.unwrap_or_default();
    assert!(
        body.starts_with("install failed: Boa is not running from an app bundle"),
        "{body}"
    );
    // Still ready; another try works.
    assert_eq!(
        pill_text(&mut vcx).as_deref(),
        Some("Restart to update to 1.2.3")
    );
    let installed = fake_installer(&updates, &mut vcx);
    click_pill(&mut vcx);
    wait_until(&mut vcx, "the quit", is_quitting);
    assert_eq!(installed.lock().unwrap().len(), 1);
}

#[gpui::test]
fn a_pretend_download_has_nothing_to_install(cx: &mut TestAppContext) {
    let dir = TempDir::new("updates-pretend");
    init(cx, dir.path());
    let updates = updates(cx, Some(feed(Vec::new())));
    updates.update(cx, |u, _| {
        u.set_downloader(Some(std::rc::Rc::new(|_, _| Task::ready(Ok(())))));
    });
    assert_eq!(
        check(cx, &updates).as_deref(),
        Some("Boa 1.2.3 is ready to install.")
    );
    let (_root, mut vcx) = open_main(cx);
    draw(&mut vcx);
    click_pill(&mut vcx);
    wait_until(&mut vcx, "the answer", |vcx| {
        !vcx.update(|_, cx| updates.read(cx).restarting())
    });
    let body = vcx.update(|_, cx| {
        AppState::global(cx)
            .toasts
            .read(cx)
            .toasts()
            .last()
            .and_then(|t| t.body.clone())
    });
    assert_eq!(body.as_deref(), Some("no update has been downloaded"));
    assert!(!is_quitting(&mut vcx));
}
