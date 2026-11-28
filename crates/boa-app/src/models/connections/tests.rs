//! Connections model tests: the store-level cases of `state/reconnect.test.ts`
//! (on a scripted [`ConnApi`], time driven by the test executor), the
//! password prompt flow, connect failures, one status toast per connection,
//! save / delete / reorder, and live tests against the local Postgres
//! (`PGHOST` set).

use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;
use std::time::Duration;

use boa_core::backend::LinkEvent;
use boa_core::connections::{ConnConfig, SavedConnection, SslMode};
use boa_core::error::CmdError;
use boa_core::pg::{ConnectInfo, PingInfo};
use boa_core::state::reconnect::{RETRY_DELAYS_MS, RecoverOpts, RecoverReason, link_events_for};
use gpui::{AppContext as _, Entity, Task, TestAppContext};

use super::*;
use crate::models::prefs::Prefs;
use crate::models::toasts::{ToastKind, Toasts};

// ── A scripted backend ────────────────────────────────────────────────────

#[derive(Default)]
struct Script {
    saved: Vec<SavedConnection>,
    pings: VecDeque<Result<PingInfo, CmdError>>,
    ping_default: Option<Result<PingInfo, CmdError>>,
    ping_calls: usize,
    connects: VecDeque<Result<ConnectInfo, CmdError>>,
    connect_default: Option<Result<ConnectInfo, CmdError>>,
    connect_passwords: Vec<Option<String>>,
    disconnects: usize,
    saves: Vec<(ConnConfig, Option<String>, bool)>,
    deletes: Vec<String>,
}

#[derive(Default)]
struct FakeApi(RefCell<Script>);

fn info() -> ConnectInfo {
    ConnectInfo {
        server_version: "PostgreSQL 17".into(),
        databases: vec!["app".into()],
    }
}

fn unreachable() -> CmdError {
    CmdError::code("unreachable", "Connection refused")
}

fn not_connected() -> CmdError {
    CmdError::code("not_connected", "Not connected")
}

fn ok_ping() -> Result<PingInfo, CmdError> {
    Ok(PingInfo {
        lost_tx: Vec::new(),
    })
}

impl FakeApi {
    fn script(&self) -> std::cell::RefMut<'_, Script> {
        self.0.borrow_mut()
    }
    fn ping_calls(&self) -> usize {
        self.0.borrow().ping_calls
    }
    fn connect_calls(&self) -> usize {
        self.0.borrow().connect_passwords.len()
    }
}

impl ConnApi for FakeApi {
    fn list(&self) -> ApiFuture<Vec<SavedConnection>> {
        let list = self.0.borrow().saved.clone();
        Box::pin(std::future::ready(Ok(list)))
    }
    fn save(
        &self,
        config: ConnConfig,
        password: Option<String>,
        save_password: bool,
    ) -> ApiFuture<()> {
        self.script().saves.push((config, password, save_password));
        Box::pin(std::future::ready(Ok(())))
    }
    fn delete(&self, id: &str) -> ApiFuture<()> {
        self.script().deletes.push(id.to_owned());
        Box::pin(std::future::ready(Ok(())))
    }
    fn test(&self, _: ConnConfig, _: Option<String>) -> ApiFuture<String> {
        Box::pin(std::future::ready(Ok("PostgreSQL 17".to_owned())))
    }
    fn connect(&self, _: &str, password: Option<String>) -> ApiFuture<ConnectInfo> {
        let mut s = self.script();
        s.connect_passwords.push(password);
        let r = s
            .connects
            .pop_front()
            .or_else(|| s.connect_default.clone())
            .unwrap_or_else(|| Ok(info()));
        Box::pin(std::future::ready(r))
    }
    fn ping(&self, _: &str) -> ApiFuture<PingInfo> {
        let mut s = self.script();
        s.ping_calls += 1;
        let r = s
            .pings
            .pop_front()
            .or_else(|| s.ping_default.clone())
            .unwrap_or_else(ok_ping);
        Box::pin(std::future::ready(r))
    }
    fn disconnect(&self, _: &str) -> ApiFuture<()> {
        self.script().disconnects += 1;
        Box::pin(std::future::ready(Ok(())))
    }
}

fn config(id: &str, name: &str) -> ConnConfig {
    ConnConfig {
        kind: Default::default(),
        id: id.into(),
        name: name.into(),
        host: "localhost".into(),
        port: 5432,
        user: "me".into(),
        database: "app".into(),
        ssl_mode: SslMode::Prefer,
        read_only: false,
        environment: None,
        color: None,
        ssh: None,
    }
}

fn saved(id: &str, name: &str, has_password: bool) -> SavedConnection {
    SavedConnection {
        config: config(id, name),
        has_password,
    }
}

struct Env {
    conns: Entity<Connections>,
    toasts: Entity<Toasts>,
    api: Rc<FakeApi>,
    events: Rc<RefCell<Vec<ConnectionsEvent>>>,
}

/// A model on a fake backend with one saved connection "Local" (c1), in
/// `state` (`connected` / `failed`: wanted, as after a connect).
fn setup(cx: &mut TestAppContext, state: &str) -> Env {
    setup_with(cx, vec![saved("c1", "Local", true)], state, None)
}

fn setup_with(
    cx: &mut TestAppContext,
    list: Vec<SavedConnection>,
    state: &str,
    prefs: Option<Entity<Prefs>>,
) -> Env {
    let api = Rc::new(FakeApi::default());
    api.script().saved = list;
    let toasts = cx.new(|_| Toasts::new());
    let conns =
        cx.new(|cx| Connections::with_api(api.clone(), Some(toasts.clone()), prefs.clone(), cx));
    let events = Rc::new(RefCell::new(Vec::new()));
    {
        let events = events.clone();
        cx.update(|cx| {
            cx.subscribe(&conns, move |_, e: &ConnectionsEvent, _| {
                events.borrow_mut().push(e.clone())
            })
            .detach();
        });
    }
    cx.run_until_parked();
    if state == "connected" || state == "failed" {
        conns.update(cx, |c, cx| c.connect("c1", cx));
        cx.run_until_parked();
        assert!(conns.read_with(cx, |c, _| c.is_connected("c1")));
    }
    if state == "failed" {
        conns.update(cx, |c, _| {
            c.state.insert(
                "c1".into(),
                ConnState::Failed {
                    message: "Connection refused".into(),
                },
            );
        });
    }
    // Only what the test does counts.
    let list = api.0.borrow().saved.clone();
    *api.script() = Script {
        saved: list,
        ..Script::default()
    };
    events.borrow_mut().clear();
    Env {
        conns,
        toasts,
        api,
        events,
    }
}

impl Env {
    fn state(&self, cx: &mut TestAppContext) -> &'static str {
        self.conns.read_with(cx, |c, _| c.state_of("c1").as_str())
    }
    fn titles(&self, cx: &mut TestAppContext) -> Vec<String> {
        self.toasts.read_with(cx, |t, _| {
            t.toasts().iter().map(|t| t.title.to_string()).collect()
        })
    }
    fn wanted(&self, cx: &mut TestAppContext) -> Vec<String> {
        self.conns.read_with(cx, |c, _| c.wanted_ids())
    }
}

/// Polls a task's result after the executor ran.
fn track<T: 'static>(cx: &mut TestAppContext, task: Task<T>) -> Rc<RefCell<Option<T>>> {
    let out = Rc::new(RefCell::new(None));
    let slot = out.clone();
    cx.foreground_executor()
        .spawn(async move {
            *slot.borrow_mut() = Some(task.await);
        })
        .detach();
    out
}

fn advance(cx: &mut TestAppContext, ms: u64) {
    cx.executor().advance_clock(Duration::from_millis(ms));
    cx.run_until_parked();
}

fn run_out_of_retries(cx: &mut TestAppContext) {
    for ms in RETRY_DELAYS_MS {
        advance(cx, ms);
    }
}

fn opts(reason: RecoverReason) -> RecoverOpts {
    RecoverOpts {
        reason,
        ..RecoverOpts::default()
    }
}

// ── describe('reconnecting') ──────────────────────────────────────────────

#[gpui::test]
fn keeps_a_connection_that_still_answers_without_showing_anything(cx: &mut TestAppContext) {
    let env = setup(cx, "connected");
    let done = env
        .conns
        .update(cx, |c, cx| c.recover("c1", opts(RecoverReason::Wake), cx));
    let done = track(cx, done);
    cx.run_until_parked();
    assert_eq!(*done.borrow(), Some(true));
    assert_eq!(env.api.ping_calls(), 1);
    assert_eq!(env.state(cx), "connected");
    assert!(env.titles(cx).is_empty());
}

#[gpui::test]
fn shows_reconnecting_while_the_server_is_away_and_says_when_it_is_back(cx: &mut TestAppContext) {
    let env = setup(cx, "connected");
    env.api.script().pings = VecDeque::from([Err(unreachable()), Err(unreachable())]);
    let done = env
        .conns
        .update(cx, |c, cx| c.recover("c1", RecoverOpts::default(), cx));
    let done = track(cx, done);
    cx.run_until_parked();
    assert_eq!(env.state(cx), "reconnecting");
    advance(cx, RETRY_DELAYS_MS[0]);
    assert_eq!(env.state(cx), "reconnecting");
    assert_eq!(
        env.conns.read_with(cx, |c, _| c.state_of("c1")),
        ConnState::Reconnecting { attempt: 2 }
    );
    advance(cx, RETRY_DELAYS_MS[1]);
    assert_eq!(*done.borrow(), Some(true));
    assert_eq!(env.api.ping_calls(), 3);
    assert_eq!(env.state(cx), "connected");
    assert_eq!(env.titles(cx), ["Reconnected to Local"]);
    // Connected again: the server version and databases are kept.
    assert_eq!(env.conns.read_with(cx, |c, _| c.databases("c1")), ["app"]);
}

#[gpui::test]
fn gives_up_after_the_last_retry_offering_to_retry(cx: &mut TestAppContext) {
    let env = setup(cx, "connected");
    env.api.script().ping_default = Some(Err(unreachable()));
    let done = env
        .conns
        .update(cx, |c, cx| c.recover("c1", RecoverOpts::default(), cx));
    let done = track(cx, done);
    cx.run_until_parked();
    run_out_of_retries(cx);
    assert_eq!(*done.borrow(), Some(false));
    assert_eq!(env.state(cx), "failed");
    let toast = env.toasts.read_with(cx, |t, _| t.toasts()[0].clone());
    assert_eq!(toast.title.as_ref(), "Lost connection to Local");
    assert_eq!(toast.body.as_deref(), Some("Connection refused"));
    assert_eq!(
        toast.action.map(|a| a.label.to_string()).as_deref(),
        Some("Retry")
    );
    assert_eq!(toast.key.as_deref(), Some("conn:c1"));
    // Until dismissed.
    advance(cx, 60_000);
    assert_eq!(env.titles(cx), ["Lost connection to Local"]);
}

#[gpui::test]
fn failing_again_replaces_the_lost_connection_toast_instead_of_stacking_another(
    cx: &mut TestAppContext,
) {
    let env = setup(cx, "connected");
    env.api.script().ping_default = Some(Err(unreachable()));
    for _ in 0..2 {
        let done = env
            .conns
            .update(cx, |c, cx| c.recover("c1", RecoverOpts::default(), cx));
        let done = track(cx, done);
        cx.run_until_parked();
        run_out_of_retries(cx);
        assert_eq!(*done.borrow(), Some(false));
    }
    assert_eq!(env.titles(cx), ["Lost connection to Local"]);
}

#[gpui::test]
fn a_background_retry_of_a_failed_connection_gives_up_without_a_new_toast(cx: &mut TestAppContext) {
    let env = setup(cx, "failed");
    env.api.script().ping_default = Some(Err(unreachable()));
    env.conns.update(cx, |c, cx| c.check_connections(cx));
    cx.run_until_parked();
    run_out_of_retries(cx);
    assert_eq!(env.api.ping_calls(), RETRY_DELAYS_MS.len() + 1);
    assert_eq!(env.state(cx), "failed");
    assert!(env.titles(cx).is_empty());
}

#[gpui::test]
fn retry_during_a_background_retry_shows_how_it_ends(cx: &mut TestAppContext) {
    let env = setup(cx, "failed");
    env.api.script().ping_default = Some(Err(unreachable()));
    env.conns.update(cx, |c, cx| c.check_connections(cx));
    cx.run_until_parked();
    let done = env
        .conns
        .update(cx, |c, cx| c.recover("c1", RecoverOpts::default(), cx));
    let done = track(cx, done);
    run_out_of_retries(cx);
    assert_eq!(*done.borrow(), Some(false));
    assert_eq!(env.titles(cx), ["Lost connection to Local"]);
}

#[gpui::test]
fn getting_back_replaces_the_lost_connection_toast(cx: &mut TestAppContext) {
    let env = setup(cx, "connected");
    env.api.script().ping_default = Some(Err(unreachable()));
    let lost = env
        .conns
        .update(cx, |c, cx| c.recover("c1", RecoverOpts::default(), cx));
    let lost = track(cx, lost);
    cx.run_until_parked();
    run_out_of_retries(cx);
    assert_eq!(*lost.borrow(), Some(false));
    {
        let mut s = env.api.script();
        s.ping_default = Some(ok_ping());
        s.pings = VecDeque::from([Err(unreachable())]);
    }
    env.conns.update(cx, |c, cx| c.check_connections(cx));
    cx.run_until_parked();
    advance(cx, RETRY_DELAYS_MS[0]);
    assert_eq!(env.state(cx), "connected");
    assert_eq!(env.titles(cx), ["Reconnected to Local"]);
}

#[gpui::test]
fn back_on_the_first_try_drops_the_lost_connection_toast(cx: &mut TestAppContext) {
    let env = setup(cx, "connected");
    env.api.script().ping_default = Some(Err(unreachable()));
    let lost = env
        .conns
        .update(cx, |c, cx| c.recover("c1", RecoverOpts::default(), cx));
    let lost = track(cx, lost);
    cx.run_until_parked();
    run_out_of_retries(cx);
    assert_eq!(*lost.borrow(), Some(false));
    env.api.script().ping_default = Some(ok_ping());
    env.conns.update(cx, |c, cx| c.check_connections(cx));
    cx.run_until_parked();
    assert_eq!(env.state(cx), "connected");
    assert!(env.titles(cx).is_empty());
}

#[gpui::test]
fn a_keyed_toast_replacing_a_timed_one_keeps_its_own_lifetime(cx: &mut TestAppContext) {
    let toasts = cx.new(|_| Toasts::new());
    let first = toasts.update(cx, |t, cx| {
        t.push_for(
            NewToast::success("one").key("k"),
            Some(Duration::from_millis(1000)),
            cx,
        )
    });
    let second = toasts.update(cx, |t, cx| {
        t.push_for(NewToast::error("two").key("k"), None, cx)
    });
    assert_eq!(second, first);
    advance(cx, 5000);
    let titles: Vec<String> = toasts.read_with(cx, |t, _| {
        t.toasts().iter().map(|t| t.title.to_string()).collect()
    });
    assert_eq!(titles, ["two"]);
}

#[gpui::test]
fn connects_again_when_the_backend_no_longer_has_the_connection_open(cx: &mut TestAppContext) {
    let env = setup(cx, "disconnected");
    env.api.script().ping_default = Some(Err(not_connected()));
    let done = env.conns.update(cx, |c, cx| {
        c.recover("c1", opts(RecoverReason::Restore), cx)
    });
    let done = track(cx, done);
    cx.run_until_parked();
    assert_eq!(*done.borrow(), Some(true));
    assert_eq!(env.api.connect_calls(), 1);
    assert_eq!(env.state(cx), "connected");
    assert!(env.titles(cx).is_empty());
}

#[gpui::test]
fn stops_on_a_refused_login_skipping_the_password_prompt_in_the_background(
    cx: &mut TestAppContext,
) {
    let env = setup_with(cx, vec![saved("c1", "Local", false)], "disconnected", None);
    {
        let mut s = env.api.script();
        s.ping_default = Some(Err(not_connected()));
        s.connect_default = Some(Err(CmdError::code("password_required", "no password")));
    }
    let done = env.conns.update(cx, |c, cx| {
        c.recover(
            "c1",
            RecoverOpts {
                reason: RecoverReason::Restore,
                quiet: true,
                background: false,
            },
            cx,
        )
    });
    let done = track(cx, done);
    cx.run_until_parked();
    assert_eq!(*done.borrow(), Some(false));
    assert_eq!(
        env.conns.read_with(cx, |c, _| c.password_prompt().cloned()),
        None
    );
    assert!(env.wanted(cx).is_empty());
    assert!(
        !env.events
            .borrow()
            .iter()
            .any(|e| matches!(e, ConnectionsEvent::PasswordPrompt { .. }))
    );
}

#[gpui::test]
fn warns_about_transactions_rolled_back_with_a_dropped_session(cx: &mut TestAppContext) {
    let env = setup(cx, "connected");
    env.api.script().ping_default = Some(Ok(PingInfo {
        lost_tx: vec!["app".into()],
    }));
    let done = env
        .conns
        .update(cx, |c, cx| c.recover("c1", opts(RecoverReason::Wake), cx));
    let _done = track(cx, done);
    cx.run_until_parked();
    assert_eq!(env.titles(cx), ["Transaction rolled back"]);
    let toast = env.toasts.read_with(cx, |t, _| t.toasts()[0].clone());
    assert_eq!(
        toast.body.as_deref(),
        Some(
            "The connection to Local (app) was lost, so the server rolled back the open \
             transaction. Uncommitted changes are gone."
        )
    );
    assert_eq!(toast.kind, ToastKind::Error);
    assert!(env.events.borrow().contains(&ConnectionsEvent::TxLost {
        id: "c1".into(),
        database: "app".into()
    }));
    // Until dismissed.
    advance(cx, 60_000);
    assert_eq!(env.titles(cx), ["Transaction rolled back"]);
}

#[gpui::test]
fn starts_reconnecting_when_a_call_fails_on_the_network(cx: &mut TestAppContext) {
    let env = setup(cx, "connected");
    env.api.script().ping_default = Some(Err(unreachable()));
    // What Backend::link_events sends for a run_query that failed with connection_lost.
    for event in link_events_for(Some("connection_lost"), "c1", "app") {
        env.conns.update(cx, |c, cx| c.on_link_event(&event, cx));
    }
    cx.run_until_parked();
    assert_eq!(env.state(cx), "reconnecting");
    // More failures meanwhile join the same recovery.
    env.conns.update(cx, |c, cx| {
        c.on_link_event(
            &LinkEvent::Lost {
                connection_id: "c1".into(),
            },
            cx,
        )
    });
    cx.run_until_parked();
    assert_eq!(env.api.ping_calls(), 1);
}

#[gpui::test]
fn a_wake_up_retries_a_failed_wanted_connection_right_away(cx: &mut TestAppContext) {
    let env = setup(cx, "failed");
    env.conns.update(cx, |c, cx| c.check_connections(cx));
    cx.run_until_parked();
    assert_eq!(env.api.ping_calls(), 1);
    assert_eq!(env.state(cx), "connected");
}

#[gpui::test]
fn disconnecting_stops_reconnecting_and_forgets_the_connection_was_wanted(cx: &mut TestAppContext) {
    let env = setup(cx, "connected");
    env.api.script().ping_default = Some(Err(unreachable()));
    let done = env
        .conns
        .update(cx, |c, cx| c.recover("c1", RecoverOpts::default(), cx));
    let done = track(cx, done);
    cx.run_until_parked();
    env.conns.update(cx, |c, cx| c.disconnect("c1", cx));
    cx.run_until_parked();
    assert_eq!(*done.borrow(), Some(false));
    advance(cx, 60_000);
    assert_eq!(env.api.ping_calls(), 1);
    assert_eq!(env.state(cx), "disconnected");
    assert!(env.wanted(cx).is_empty());
    assert_eq!(env.api.0.borrow().disconnects, 1);
    assert!(
        env.events
            .borrow()
            .contains(&ConnectionsEvent::Disconnected { id: "c1".into() })
    );
}

#[gpui::test]
fn running_a_query_connects_first(cx: &mut TestAppContext) {
    let env = setup(cx, "disconnected");
    let ready = env.conns.update(cx, |c, cx| c.ensure_connected("c1", cx));
    let ready = track(cx, ready);
    cx.run_until_parked();
    assert!(matches!(*ready.borrow(), Some(Ok(()))));
    assert_eq!(env.state(cx), "connected");
    // Connected: ready at once, no second connect.
    let again = env.conns.update(cx, |c, cx| c.ensure_connected("c1", cx));
    let again = track(cx, again);
    cx.run_until_parked();
    assert!(matches!(*again.borrow(), Some(Ok(()))));
    assert_eq!(env.api.connect_calls(), 1);
    // Dropped meanwhile: the next run connects again first.
    env.conns.update(cx, |c, _| {
        c.state.insert("c1".into(), ConnState::Disconnected);
    });
    let third = env.conns.update(cx, |c, cx| c.ensure_connected("c1", cx));
    let third = track(cx, third);
    cx.run_until_parked();
    assert!(matches!(*third.borrow(), Some(Ok(()))));
    assert_eq!(env.state(cx), "connected");
    assert_eq!(env.api.connect_calls(), 2);
}

// ── Beyond the TS cases ───────────────────────────────────────────────────

#[gpui::test]
fn a_second_connect_joins_the_one_in_flight(cx: &mut TestAppContext) {
    let env = setup(cx, "disconnected");
    let (a, b) = env.conns.update(cx, |c, cx| {
        let a = c.ensure_connected("c1", cx);
        assert_eq!(c.state_of("c1"), ConnState::Connecting);
        assert!(c.is_connecting("c1"));
        (a, c.ensure_connected("c1", cx))
    });
    let (a, b) = (track(cx, a), track(cx, b));
    cx.run_until_parked();
    assert!(matches!(*a.borrow(), Some(Ok(()))));
    assert!(matches!(*b.borrow(), Some(Ok(()))));
    assert_eq!(env.api.connect_calls(), 1);
    assert_eq!(
        env.events.borrow().as_slice(),
        [ConnectionsEvent::Connected { id: "c1".into() }]
    );
    assert_eq!(env.wanted(cx), ["c1"]);
}

#[gpui::test]
fn ensure_connected_while_reconnecting_retries_now(cx: &mut TestAppContext) {
    let env = setup(cx, "connected");
    env.api.script().pings = VecDeque::from([Err(unreachable())]);
    env.conns.update(cx, |c, cx| {
        c.recover("c1", RecoverOpts::default(), cx).detach()
    });
    cx.run_until_parked();
    assert_eq!(env.state(cx), "reconnecting");
    let ready = env.conns.update(cx, |c, cx| c.ensure_connected("c1", cx));
    let ready = track(cx, ready);
    cx.run_until_parked();
    // No waiting for the 1 s timer.
    assert!(matches!(*ready.borrow(), Some(Ok(()))));
    assert_eq!(env.api.ping_calls(), 2);
    assert_eq!(env.state(cx), "connected");
    assert_eq!(env.titles(cx), ["Reconnected to Local"]);
}

#[gpui::test]
fn no_stored_password_opens_the_prompt_and_its_password_connects(cx: &mut TestAppContext) {
    let env = setup_with(cx, vec![saved("c1", "Local", false)], "disconnected", None);
    env.api.script().connects = VecDeque::from([Err(CmdError::code(
        "password_required",
        "The server asked for a password",
    ))]);
    let first = env.conns.update(cx, |c, cx| c.connect_with("c1", None, cx));
    let first = track(cx, first);
    cx.run_until_parked();
    assert!(
        matches!(&*first.borrow(), Some(Err(e)) if e.code.as_deref() == Some("password_required"))
    );
    assert_eq!(env.state(cx), "disconnected");
    assert_eq!(
        env.conns.read_with(cx, |c, _| c.password_prompt().cloned()),
        Some(PasswordPromptState {
            connection_id: "c1".into(),
            error: None
        })
    );
    assert!(
        env.events
            .borrow()
            .contains(&ConnectionsEvent::PasswordPrompt { id: "c1".into() })
    );
    // No toast for a refused login: the prompt is the answer.
    assert!(env.titles(cx).is_empty());

    let done = env.conns.update(cx, |c, cx| {
        c.submit_password("c1", "secret".into(), false, cx)
    });
    let done = track(cx, done);
    cx.run_until_parked();
    assert!(matches!(*done.borrow(), Some(Ok(()))));
    assert_eq!(
        env.api.0.borrow().connect_passwords,
        [None, Some("secret".to_owned())]
    );
    assert_eq!(
        env.conns.read_with(cx, |c, _| c.password_prompt().cloned()),
        None
    );
    assert_eq!(env.state(cx), "connected");
    // Not saved.
    assert!(env.api.0.borrow().saves.is_empty());
    assert!(
        !env.conns
            .read_with(cx, |c, _| c.get("c1").unwrap().has_password)
    );
}

#[gpui::test]
fn a_refused_password_reopens_the_prompt_with_the_error(cx: &mut TestAppContext) {
    let env = setup(cx, "disconnected");
    let refused = CmdError::code("28P01", "password authentication failed for user \"me\"");
    env.api.script().connects = VecDeque::from([Err(refused.clone()), Err(refused)]);
    // The stored password is wrong: ask, with the error.
    env.conns.update(cx, |c, cx| c.connect("c1", cx));
    cx.run_until_parked();
    let prompt = env.conns.read_with(cx, |c, _| c.password_prompt().cloned());
    assert_eq!(
        prompt.and_then(|p| p.error).as_deref(),
        Some("password authentication failed for user \"me\"")
    );
    // A typed one is wrong too: still asking, with the error.
    let done = env.conns.update(cx, |c, cx| {
        c.submit_password("c1", "nope".into(), false, cx)
    });
    let _done = track(cx, done);
    cx.run_until_parked();
    let prompt = env.conns.read_with(cx, |c, _| c.password_prompt().cloned());
    assert!(prompt.is_some_and(|p| p.error.is_some()));
    assert!(env.titles(cx).is_empty());
    // 28000 (e.g. no pg_hba entry) is a refused login too (disconnected, not
    // failed), but with a stored password that wasn't refused as wrong, the
    // store showed the status toast rather than asking (connectNow).
    env.conns.update(cx, |c, cx| c.dismiss_password_prompt(cx));
    env.api.script().connects =
        VecDeque::from([Err(CmdError::code("28000", "no pg_hba.conf entry"))]);
    env.conns.update(cx, |c, cx| c.connect("c1", cx));
    cx.run_until_parked();
    assert!(
        env.conns
            .read_with(cx, |c, _| c.password_prompt().is_none())
    );
    assert_eq!(env.state(cx), "disconnected");
    assert_eq!(env.titles(cx), ["Could not connect to Local"]);
}

#[gpui::test]
fn saving_the_password_from_the_prompt_stores_it(cx: &mut TestAppContext) {
    let env = setup_with(cx, vec![saved("c1", "Local", false)], "disconnected", None);
    let done = env.conns.update(cx, |c, cx| {
        c.submit_password("c1", "secret".into(), true, cx)
    });
    let done = track(cx, done);
    cx.run_until_parked();
    assert!(matches!(*done.borrow(), Some(Ok(()))));
    let saves = env.api.0.borrow().saves.clone();
    assert_eq!(saves.len(), 1);
    assert_eq!(saves[0].1.as_deref(), Some("secret"));
    assert!(saves[0].2);
    assert!(
        env.conns
            .read_with(cx, |c, _| c.get("c1").unwrap().has_password)
    );
    assert_eq!(env.state(cx), "connected");
}

#[gpui::test]
fn a_failed_connect_shows_one_status_toast_per_connection(cx: &mut TestAppContext) {
    let env = setup_with(
        cx,
        vec![saved("c1", "Local", true), saved("c2", "Staging", true)],
        "disconnected",
        None,
    );
    env.api.script().connect_default = Some(Err(unreachable()));
    for _ in 0..3 {
        env.conns.update(cx, |c, cx| c.connect("c1", cx));
        cx.run_until_parked();
    }
    env.conns.update(cx, |c, cx| c.connect("c2", cx));
    cx.run_until_parked();
    assert_eq!(
        env.titles(cx),
        ["Could not connect to Local", "Could not connect to Staging"]
    );
    assert_eq!(env.state(cx), "failed");
    assert_eq!(
        env.conns.read_with(cx, |c, _| c.state_of("c1")),
        ConnState::Failed {
            message: "Connection refused".into()
        }
    );
    let keys: Vec<Option<String>> = env.toasts.read_with(cx, |t, _| {
        t.toasts()
            .iter()
            .map(|t| t.key.as_ref().map(|k| k.to_string()))
            .collect()
    });
    assert_eq!(keys, [Some("conn:c1".into()), Some("conn:c2".into())]);
    // Connecting then clears the failure toast.
    env.api.script().connect_default = Some(Ok(info()));
    env.conns.update(cx, |c, cx| c.connect("c1", cx));
    cx.run_until_parked();
    assert_eq!(env.titles(cx), ["Could not connect to Staging"]);
}

#[gpui::test]
fn tx_lost_and_commit_unknown_messages(cx: &mut TestAppContext) {
    let _ = cx;
    let tx_lost = CmdError {
        hint: Some("Uncommitted changes are gone. Run them again to redo them.".into()),
        ..CmdError::code(
            "tx_lost",
            "The connection to the server was lost, so the open transaction was rolled back. \
             The statement was not run.",
        )
    };
    let (message, note) = describe_error(&tx_lost);
    assert!(message.contains("rolled back"));
    assert_eq!(
        note.as_deref(),
        Some("Uncommitted changes are gone. Run them again to redo them.")
    );
    let unknown = CmdError::code(
        "connection_lost",
        "The connection to the server was lost while committing: the transaction may or may \
         not have been committed",
    );
    let (message, note) = describe_error(&unknown);
    assert!(message.contains("may or may not have been committed"));
    assert_eq!(
        note.as_deref(),
        Some("Check the data to see whether it was committed.")
    );
    let plain = CmdError {
        detail: Some("Key (id)=(1) already exists.".into()),
        ..CmdError::code("23505", "duplicate key value")
    };
    assert_eq!(
        describe_error(&plain),
        (
            "duplicate key value".to_owned(),
            Some("Key (id)=(1) already exists.".to_owned())
        )
    );
}

#[gpui::test]
fn the_list_follows_the_saved_order_and_reorder_saves_it(cx: &mut TestAppContext) {
    let p = boa_core::state::prefs::Prefs {
        connection_order: vec!["c3".into(), "c1".into()],
        ..Default::default()
    };
    let prefs = cx.new(|_| Prefs::in_memory(p));
    let env = setup_with(
        cx,
        vec![
            saved("c1", "One", true),
            saved("c2", "Two", true),
            saved("c3", "Three", true),
        ],
        "disconnected",
        Some(prefs.clone()),
    );
    let ids = |cx: &mut TestAppContext| -> Vec<String> {
        env.conns.read_with(cx, |c, _| {
            c.saved.iter().map(|s| s.config.id.clone()).collect()
        })
    };
    assert!(env.conns.read_with(cx, |c, _| c.is_loaded()));
    assert_eq!(ids(cx), ["c3", "c1", "c2"]);
    env.conns.update(cx, |c, cx| {
        c.reorder(vec!["c2".into(), "c3".into(), "c1".into()], cx)
    });
    cx.run_until_parked();
    assert_eq!(ids(cx), ["c2", "c3", "c1"]);
    assert_eq!(
        prefs.read_with(cx, |p, _| p.get().connection_order.clone()),
        ["c2", "c3", "c1"]
    );
    // A change of the pref from elsewhere (Settings) re-sorts too.
    prefs.update(cx, |p, cx| {
        p.update(cx, |p| p.connection_order = vec!["c1".into()])
    });
    cx.run_until_parked();
    assert_eq!(ids(cx), ["c1", "c2", "c3"]);
}

#[gpui::test]
fn the_first_listed_connection_follows_the_backend_order_not_the_sidebar(cx: &mut TestAppContext) {
    // StatusBar.tsx falls back on store.ts `connectionOrder[0]`: the order
    // `listConnections` returned (new ones appended), whatever the sidebar's.
    let p = boa_core::state::prefs::Prefs {
        connection_order: vec!["c3".into(), "c2".into()],
        ..Default::default()
    };
    let prefs = cx.new(|_| Prefs::in_memory(p));
    let env = setup_with(
        cx,
        vec![
            saved("c1", "One", true),
            saved("c2", "Two", true),
            saved("c3", "Three", true),
        ],
        "disconnected",
        Some(prefs),
    );
    let first = |cx: &mut TestAppContext| {
        env.conns
            .read_with(cx, |c, _| c.first_listed().map(|s| s.config.id.clone()))
    };
    let sidebar_first = env.conns.read_with(cx, |c, _| c.saved[0].config.id.clone());
    assert_eq!(sidebar_first, "c3");
    assert_eq!(first(cx).as_deref(), Some("c1"));
    // A new connection goes last; deleting the first moves on to the next.
    let done = env
        .conns
        .update(cx, |c, cx| c.save(config("c4", "Four"), None, false, cx));
    let done = track(cx, done);
    cx.run_until_parked();
    assert!(matches!(*done.borrow(), Some(Ok(()))));
    let done = env.conns.update(cx, |c, cx| c.delete("c1", cx));
    let done = track(cx, done);
    cx.run_until_parked();
    assert!(matches!(*done.borrow(), Some(Ok(()))));
    assert_eq!(first(cx).as_deref(), Some("c2"));
    for id in ["c2", "c3"] {
        let done = env.conns.update(cx, |c, cx| c.delete(id, cx));
        track(cx, done);
        cx.run_until_parked();
    }
    assert_eq!(first(cx).as_deref(), Some("c4"));
}

#[gpui::test]
fn saving_adds_or_updates_and_an_open_connection_reconnects(cx: &mut TestAppContext) {
    let env = setup(cx, "connected");
    // A new one: appended, password stored.
    let done = env.conns.update(cx, |c, cx| {
        c.save(config("c2", "New"), Some("pw".into()), true, cx)
    });
    let done = track(cx, done);
    cx.run_until_parked();
    assert!(matches!(*done.borrow(), Some(Ok(()))));
    assert!(
        env.conns
            .read_with(cx, |c, _| c.get("c2").unwrap().has_password)
    );
    assert!(env.events.borrow().contains(&ConnectionsEvent::Saved {
        id: "c2".into(),
        created: true
    }));
    assert_eq!(env.api.connect_calls(), 0);
    // The open one, edited without a new password: keeps the stored one, reconnects.
    let mut edited = config("c1", "Local (edited)");
    edited.read_only = true;
    let done = env.conns.update(cx, |c, cx| c.save(edited, None, true, cx));
    let done = track(cx, done);
    cx.run_until_parked();
    assert!(matches!(*done.borrow(), Some(Ok(()))));
    let c1 = env
        .conns
        .read_with(cx, |c, _| c.get("c1").cloned().unwrap());
    assert!(c1.has_password);
    assert!(c1.config.read_only);
    assert_eq!(c1.config.name, "Local (edited)");
    assert_eq!(env.api.0.borrow().connect_passwords, [None]);
    assert_eq!(env.state(cx), "connected");
    // Not saving the password forgets it.
    let done = env
        .conns
        .update(cx, |c, cx| c.save(config("c2", "New"), None, false, cx));
    let _done = track(cx, done);
    cx.run_until_parked();
    assert!(
        !env.conns
            .read_with(cx, |c, _| c.get("c2").unwrap().has_password)
    );
}

#[gpui::test]
fn the_close_guard_can_keep_a_connection_open(cx: &mut TestAppContext) {
    let env = setup(cx, "connected");
    let asked = Rc::new(RefCell::new(Vec::new()));
    {
        let asked = asked.clone();
        env.conns.update(cx, |c, _| {
            c.set_close_guard(Some(Rc::new(move |action, id: &str, _cx| {
                asked.borrow_mut().push((action, id.to_owned()));
                Task::ready(false)
            })));
        });
    }
    let done = env.conns.update(cx, |c, cx| c.disconnect_task("c1", cx));
    let done = track(cx, done);
    cx.run_until_parked();
    assert_eq!(*done.borrow(), Some(false));
    assert_eq!(env.state(cx), "connected");
    assert_eq!(
        asked.borrow().as_slice(),
        [(CloseAction::Disconnect, "c1".to_owned())]
    );
    // Declined reconnect after an edit: the new settings apply next time.
    let done = env
        .conns
        .update(cx, |c, cx| c.save(config("c1", "Renamed"), None, true, cx));
    let _done = track(cx, done);
    cx.run_until_parked();
    assert_eq!(env.api.connect_calls(), 0);
    assert_eq!(asked.borrow().len(), 2);
    assert_eq!(asked.borrow()[1].0, CloseAction::Reconnect);
}

#[gpui::test]
fn deleting_forgets_the_connection(cx: &mut TestAppContext) {
    let env = setup(cx, "failed");
    env.api.script().connect_default = Some(Err(unreachable()));
    env.conns.update(cx, |c, cx| c.connect("c1", cx));
    cx.run_until_parked();
    assert_eq!(env.titles(cx), ["Could not connect to Local"]);
    let done = env.conns.update(cx, |c, cx| c.delete("c1", cx));
    let done = track(cx, done);
    cx.run_until_parked();
    assert!(matches!(*done.borrow(), Some(Ok(()))));
    assert_eq!(env.api.0.borrow().deletes, ["c1"]);
    assert!(env.conns.read_with(cx, |c, _| c.get("c1").is_none()));
    assert!(env.wanted(cx).is_empty());
    assert!(env.titles(cx).is_empty());
    assert!(
        env.events
            .borrow()
            .contains(&ConnectionsEvent::Deleted { id: "c1".into() })
    );
}

#[gpui::test]
fn restore_reconnects_quietly(cx: &mut TestAppContext) {
    let env = setup_with(
        cx,
        vec![saved("c1", "Local", true), saved("c2", "NoPw", false)],
        "disconnected",
        None,
    );
    {
        let mut s = env.api.script();
        s.ping_default = Some(Err(not_connected()));
        s.connects = VecDeque::from([
            Ok(info()),
            Err(CmdError::code("password_required", "no password")),
        ]);
    }
    env.conns.update(cx, |c, cx| {
        c.restore(vec!["c1".into(), "c2".into(), "gone".into()], None, cx)
    });
    cx.run_until_parked();
    assert_eq!(env.state(cx), "connected");
    assert_eq!(
        env.conns.read_with(cx, |c, _| c.state_of("c2").as_str()),
        "disconnected"
    );
    assert_eq!(
        env.conns.read_with(cx, |c, _| c.password_prompt().cloned()),
        None
    );
    assert_eq!(env.wanted(cx), ["c1"]);
    assert!(env.titles(cx).is_empty());
}

#[gpui::test]
fn a_network_change_checks_once_it_settles(cx: &mut TestAppContext) {
    let env = setup(cx, "failed");
    env.conns.update(cx, |c, cx| {
        c.on_platform_event(crate::platform::PlatformEvent::NetworkChanged, cx);
        c.on_platform_event(crate::platform::PlatformEvent::NetworkChanged, cx);
    });
    cx.run_until_parked();
    assert_eq!(env.api.ping_calls(), 0);
    advance(cx, NETWORK_SETTLE.as_millis() as u64);
    assert_eq!(env.api.ping_calls(), 1);
    assert_eq!(env.state(cx), "connected");
    // A wake-up checks at once.
    env.conns.update(cx, |c, cx| {
        c.on_platform_event(crate::platform::PlatformEvent::Woke, cx)
    });
    cx.run_until_parked();
    assert_eq!(env.api.ping_calls(), 2);
}

#[gpui::test]
fn connection_string_has_no_password(cx: &mut TestAppContext) {
    let mut c = saved("c1", "Local", true);
    c.config.user = "me@corp".into();
    let env = setup_with(cx, vec![c], "disconnected", None);
    assert_eq!(
        env.conns
            .read_with(cx, |c, _| c.connection_string("c1", Some("other"))),
        Some("postgresql://me%40corp@localhost:5432/other".to_owned())
    );
}

// ── Live (PGHOST set) ─────────────────────────────────────────────────────

mod live {
    use super::*;
    use boa_core::backend::Backend;

    fn pg() -> Option<(String, u16)> {
        let host = std::env::var("PGHOST").ok().filter(|h| !h.is_empty())?;
        let port = std::env::var("PGPORT")
            .ok()
            .and_then(|p| p.parse().ok())
            .unwrap_or(5432);
        Some((host, port))
    }

    struct Dir(std::path::PathBuf);

    impl Dir {
        fn new(name: &str) -> Self {
            let dir =
                std::env::temp_dir().join(format!("boa-app-conn-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }
    }

    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn live_config(id: &str, user: &str) -> ConnConfig {
        let (host, port) = pg().unwrap();
        ConnConfig {
            kind: Default::default(),
            host,
            port,
            user: user.into(),
            database: "boa_dev".into(),
            ..config(id, id)
        }
    }

    /// Waits (really) until `f` holds: the backend answers on its own threads.
    fn wait_for(cx: &mut TestAppContext, what: &str, f: impl Fn(&mut TestAppContext) -> bool) {
        for _ in 0..1000 {
            cx.run_until_parked();
            if f(cx) {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("timed out waiting for {what}");
    }

    fn model(cx: &mut TestAppContext, backend: &Backend) -> (Entity<Connections>, Entity<Toasts>) {
        cx.executor().allow_parking();
        let toasts = cx.new(|_| Toasts::new());
        let conns =
            cx.new(|cx| Connections::with_backend(backend.clone(), Some(toasts.clone()), None, cx));
        (conns, toasts)
    }

    fn save(backend: &Backend, cfg: ConnConfig, pw: &str) {
        futures::executor::block_on(backend.save_connection(cfg, Some(pw.into()), true)).unwrap();
    }

    #[gpui::test]
    fn connects_to_boa_dev(cx: &mut TestAppContext) {
        if pg().is_none() {
            return;
        }
        let dir = Dir::new("connect");
        let backend = Backend::with_config_dir(&dir.0).unwrap();
        save(&backend, live_config("live", "boa"), "boa");
        let (conns, _) = model(cx, &backend);
        wait_for(cx, "the list", |cx| {
            conns.read_with(cx, |c, _| c.get("live").is_some())
        });
        conns.update(cx, |c, cx| c.connect("live", cx));
        wait_for(cx, "connected", |cx| {
            conns.read_with(cx, |c, _| c.is_connected("live"))
        });
        let (dbs, version) = conns.read_with(cx, |c, _| {
            (
                c.databases("live"),
                c.server_version("live").map(str::to_owned),
            )
        });
        assert!(dbs.contains(&"boa_dev".to_owned()), "{dbs:?}");
        assert!(version.unwrap().starts_with("PostgreSQL "));
        assert!(conns.read_with(cx, |c, _| c.latency_ms("live").is_some()));
        conns.update(cx, |c, cx| c.disconnect("live", cx));
        wait_for(cx, "disconnected", |cx| {
            conns.read_with(cx, |c, _| c.state_of("live") == ConnState::Disconnected)
        });
    }

    /// The local server trusts localhost (pg_hba), so a wrong password can't be
    /// refused here (28P01 is covered on the fake backend). A role that
    /// doesn't exist is refused with 28000: with no stored password that
    /// opens the prompt.
    #[gpui::test]
    fn a_refused_login_opens_the_prompt(cx: &mut TestAppContext) {
        if pg().is_none() {
            return;
        }
        let dir = Dir::new("refused");
        let backend = Backend::with_config_dir(&dir.0).unwrap();
        futures::executor::block_on(backend.save_connection(
            live_config("live", "boa_no_such_role"),
            None,
            false,
        ))
        .unwrap();
        let (conns, toasts) = model(cx, &backend);
        wait_for(cx, "the list", |cx| {
            conns.read_with(cx, |c, _| c.get("live").is_some())
        });
        conns.update(cx, |c, cx| c.connect("live", cx));
        wait_for(cx, "the prompt", |cx| {
            conns.read_with(cx, |c, _| c.password_prompt().is_some())
        });
        assert_eq!(
            conns.read_with(cx, |c, _| c.state_of("live")),
            ConnState::Disconnected
        );
        assert!(toasts.read_with(cx, |t, _| t.toasts().is_empty()));
        // A typed password that is refused shows the server's message.
        conns.update(cx, |c, cx| {
            c.submit_password("live", "x".into(), false, cx).detach()
        });
        wait_for(cx, "the error", |cx| {
            conns.read_with(cx, |c, _| {
                c.password_prompt().is_some_and(|p| p.error.is_some())
            })
        });
        let prompt = conns.read_with(cx, |c, _| c.password_prompt().cloned().unwrap());
        assert!(
            prompt
                .error
                .as_deref()
                .unwrap()
                .contains("boa_no_such_role"),
            "{prompt:?}"
        );
    }

    #[gpui::test]
    fn a_terminated_session_reconnects(cx: &mut TestAppContext) {
        if pg().is_none() {
            return;
        }
        let dir = Dir::new("terminate");
        let backend = Backend::with_config_dir(&dir.0).unwrap();
        save(&backend, live_config("victim", "boa"), "boa");
        save(&backend, live_config("killer", "boa"), "boa");
        let (conns, _) = model(cx, &backend);
        wait_for(cx, "the list", |cx| {
            conns.read_with(cx, |c, _| c.saved.len() == 2)
        });
        conns.update(cx, |c, cx| {
            c.connect("victim", cx);
            c.connect("killer", cx);
        });
        wait_for(cx, "connected", |cx| {
            conns.read_with(cx, |c, _| {
                c.is_connected("victim") && c.is_connected("killer")
            })
        });
        let run = |sql: &str, id: &str| {
            futures::executor::block_on(backend.run_query(id, "boa_dev", sql, 10, 0))
        };
        let pid = run("select pg_backend_pid()::text", "victim").unwrap().rows[0][0]
            .clone()
            .unwrap();
        // A statement in flight when the session dies fails with
        // connection_lost (an idle dead session is reopened silently).
        let links = backend.link_events();
        let slow = backend.run_query("victim", "boa_dev", "select pg_sleep(5)", 10, 0);
        std::thread::sleep(Duration::from_millis(300));
        // Only our own session: other agents share this server.
        run(&format!("select pg_terminate_backend({pid})"), "killer").unwrap();
        let err = futures::executor::block_on(slow).unwrap_err();
        assert!(
            matches!(err.code.as_deref(), Some("connection_lost" | "unreachable")),
            "{err:?}"
        );
        // The backend reported it (the model listens to the same channel) …
        assert_eq!(
            futures::executor::block_on(links.recv()).unwrap(),
            LinkEvent::Lost {
                connection_id: "victim".into()
            }
        );
        // … which starts a recovery; ping reopens the dropped session.
        wait_for(cx, "the recovery", |cx| {
            conns.read_with(cx, |c, _| {
                !c.is_recovering("victim") && c.is_connected("victim")
            })
        });
        let pid2 = run("select pg_backend_pid()::text", "victim").unwrap().rows[0][0]
            .clone()
            .unwrap();
        assert_ne!(pid, pid2);
        assert!(conns.read_with(cx, |c, _| c.is_wanted("victim")));
    }

    #[gpui::test]
    fn a_read_only_connection_is_enforced_by_the_server(cx: &mut TestAppContext) {
        if pg().is_none() {
            return;
        }
        let dir = Dir::new("read-only");
        let backend = Backend::with_config_dir(&dir.0).unwrap();
        let mut ro = live_config("ro", "boa");
        ro.read_only = true;
        save(&backend, ro, "boa");
        save(&backend, live_config("role", "boa_ro"), "boa_ro");
        let (conns, _) = model(cx, &backend);
        wait_for(cx, "the list", |cx| {
            conns.read_with(cx, |c, _| c.saved.len() == 2)
        });
        conns.update(cx, |c, cx| {
            c.connect("ro", cx);
            c.connect("role", cx);
        });
        wait_for(cx, "connected", |cx| {
            conns.read_with(cx, |c, _| c.is_connected("ro") && c.is_connected("role"))
        });
        let run = |sql: &str, id: &str| {
            futures::executor::block_on(backend.run_query(id, "boa_dev", sql, 10, 0))
        };
        // Reads work.
        assert!(run("select count(*) from public.customers", "ro").is_ok());
        assert!(run("select count(*) from boa_fx.products", "role").is_ok());
        // Writes are refused by the server: read-only transaction (25006) …
        let err = run("create table boa_fx.boa_ro_probe (x int)", "ro").unwrap_err();
        assert_eq!(err.code.as_deref(), Some("25006"), "{err:?}");
        // … also for the read-only role (its default; or no privilege, 42501).
        let err = run("update boa_fx.products set name = name where false", "role").unwrap_err();
        assert!(
            matches!(err.code.as_deref(), Some("25006" | "42501")),
            "{err:?}"
        );
    }
}
