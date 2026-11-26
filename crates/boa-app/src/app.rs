//! Application setup: app-wide actions, the menu bar, the main window, and
//! the screenshot hook (feature `screenshot`).

use std::rc::Rc;

use boa_core::state::guards::{CloseAction, QuitDecision};
use futures::StreamExt as _;
use gpui::{
    ActivationPolicy, AnyWindowHandle, App, AppContext as _, Bounds, Global, Menu, MenuItem,
    OsAction, SystemMenuType, Task, TitlebarOptions, Window, WindowBounds, WindowHandle,
    WindowOptions, point, px, size,
};
use gpui_component::Root;

use crate::actions::*;
use crate::app_state::{self, AppState};
use crate::assets::{self, Assets};
use crate::platform::terminate::{QuitGuard, TerminateReply};
use crate::views::root::RootView;

pub const APP_NAME: &str = "Boa";
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Main window size (Tauri's `tauri.conf.json`: 1280×820, min 720×480).
pub const WINDOW_SIZE: (f32, f32) = (1280., 820.);
pub const WINDOW_MIN_SIZE: (f32, f32) = (720., 480.);
/// Where the traffic lights sit, in GPUI's terms: the close button's left
/// edge and its top edge, from the window's top-left corner.
///
/// Tauri's `trafficLightPosition` is (14, 22), but tao reads y differently: it
/// grows the title-bar container to `button height + y` and leaves each button
/// at its stock offset from the container's bottom (6 px: a 16 px button frame
/// centred in the stock 28 px title bar), so the buttons' top lands at
/// 22 - 6 = 16. GPUI puts the top at exactly y, hence 16 here.
pub const TRAFFIC_LIGHTS: (f32, f32) = (14., 16.);

/// Starts the app. Never returns until the app quits.
pub fn run() {
    let started = std::time::Instant::now();
    init_tracing();
    gpui_platform::application()
        .with_assets(Assets)
        .run(move |cx: &mut App| {
            init(cx);
            if background() {
                // No Dock icon or menu bar, and never the frontmost app.
                cx.set_activation_policy(ActivationPolicy::Accessory);
            }
            let Some(dir) = app_state::config_dir() else {
                tracing::error!("no config dir (no home directory?)");
                cx.quit();
                return;
            };
            tracing::info!(config_dir = %dir.display(), "starting");
            // Once: the Tauri app's localStorage (prefs, session, CSV options).
            if let Some(home) = crate::models::session::import_home() {
                crate::models::session::import_local_storage(&dir, &home);
            }
            #[cfg(feature = "dev-hooks")]
            let dev_connection = crate::models::connections::dev::seed(&dir);
            let state = match AppState::init(&dir, WINDOW_SIZE.0, cx) {
                Ok(state) => state,
                Err(err) => {
                    tracing::error!("{err:#}");
                    cx.quit();
                    return;
                }
            };
            // Wake-ups and network changes check the open connections.
            state.connections.update(cx, |c, cx| c.watch(cx));
            // Dock → Quit, logout, restart: asked like ⌘Q (platform/terminate.rs).
            let handler = terminate_handler(cx);
            crate::platform::terminate::intercept(handler);
            // The connections open at the last quit.
            state.session.update(cx, |s, cx| s.reconnect(cx));
            // Release builds: check now and then (models/updates.rs).
            state
                .updates
                .update(cx, |u, cx| u.start_periodic_checks(cx));
            let window = match open_main_window(cx) {
                Ok(window) => window,
                Err(err) => {
                    tracing::error!("failed to open the main window: {err:#}");
                    cx.quit();
                    return;
                }
            };
            if !background() {
                cx.activate(true);
            }
            // Startup time, for docs (`BOA_LOG=info`).
            let _ = window.update(cx, |_, window, _| {
                window.on_next_frame(move |_, _| {
                    tracing::info!("first frame after {} ms", started.elapsed().as_millis());
                });
            });
            #[cfg(feature = "dev-hooks")]
            {
                crate::views::dev::apply(cx);
                crate::views::tx_controls::dev::apply(cx);
                crate::views::dev_input::start(window, cx);
                crate::updater::dev::apply(cx);
                if let Some(id) = dev_connection {
                    crate::models::connections::dev::connect_at_launch(
                        &state.connections,
                        &state.prefs,
                        id,
                        cx,
                    );
                }
                let _ = window.update(cx, |_, window, cx| {
                    // Not while the window's root is being updated.
                    window.defer(cx, crate::views::connection_dialog::dev::apply);
                });
            }
            #[cfg(feature = "screenshot")]
            if let Some(path) = std::env::var_os("BOA_SCREENSHOT") {
                screenshot::capture_and_quit(window, path.into(), cx);
            }
        });
}

/// The variable for [`background`].
pub const BACKGROUND_ENV: &str = "BOA_DEV_BACKGROUND";

/// `BOA_DEV_BACKGROUND=1` (feature `dev-hooks`): for scripted and agent runs
/// on someone's desktop, the app stays out of their way. It runs as an
/// accessory app (no Dock icon, no menu bar), never activates itself, opens
/// its windows hidden and skips [`activate_window`], so it never takes the
/// focus or shows up over other apps. Drive it with `BOA_DEV_INPUT` and capture
/// with `shot` / `BOA_SCREENSHOT`, which render offscreen. Native chrome
/// (traffic lights, menus) needs a normal run and `screencapture`. Always
/// false without `dev-hooks`.
pub fn background() -> bool {
    #[cfg(feature = "dev-hooks")]
    {
        static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        *ON.get_or_init(|| {
            std::env::var_os(BACKGROUND_ENV).is_some_and(|v| !v.is_empty() && v != "0")
        })
    }
    #[cfg(not(feature = "dev-hooks"))]
    {
        false
    }
}

/// Brings `window` to the front, except in [`background`] mode.
pub fn activate_window(window: &mut Window) {
    if !background() {
        window.activate_window();
    }
}

fn init_tracing() {
    use tracing_subscriber::EnvFilter;
    let filter = EnvFilter::try_from_env("BOA_LOG").unwrap_or_else(|_| EnvFilter::new("info"));
    let _ = tracing_subscriber::fmt().with_env_filter(filter).try_init();
}

/// Registers everything app-wide: components, fonts, theme, key bindings,
/// app actions, the menu bar. Call before opening windows.
pub fn init(cx: &mut App) {
    gpui_component::init(cx);
    if let Err(err) = assets::load_fonts(cx) {
        tracing::warn!("bundled fonts unavailable, using system fonts: {err:#}");
    }
    crate::theme::init(cx);
    crate::keymap::init(cx);
    register_app_actions(cx);
    cx.set_menus(app_menus());

    // Like the Tauri app: closing the last window quits.
    cx.on_window_closed(|cx, _| {
        if cx.windows().is_empty() {
            cx.quit();
        }
    })
    .detach();
}

/// Handlers of the actions that work in any window (menu bar items).
pub fn register_app_actions(cx: &mut App) {
    cx.on_action(|_: &Quit, cx| request_quit(cx));
    cx.on_action(|_: &About, cx| crate::views::about::open(cx));
    cx.on_action(|_: &CheckForUpdates, cx| {
        // updater.rs on_menu_event: open About, then check.
        crate::views::about::open(cx);
        if let Some(state) = AppState::try_global(cx) {
            let updates = state.updates.clone();
            updates.update(cx, |u, cx| u.check(true, cx));
        }
    });
    cx.on_action(|_: &ToggleSettings, cx| {
        let Some(state) = AppState::try_global(cx) else {
            return;
        };
        let layout = state.layout.clone();
        layout.update(cx, |l, cx| l.toggle_settings(cx));
        // From the menu with About in front: bring the main window up.
        if let Some(main) = main_window(cx) {
            main.update(cx, |_, window, _| activate_window(window)).ok();
        }
    });
    cx.on_action(|_: &Hide, cx| cx.hide());
    cx.on_action(|_: &HideOthers, cx| cx.hide_other_apps());
    cx.on_action(|_: &ShowAll, cx| cx.unhide_other_apps());
    cx.on_action(|_: &Minimize, cx| with_active_window(cx, |w| w.minimize_window()));
    cx.on_action(|_: &Zoom, cx| with_active_window(cx, |w| w.zoom_window()));
    cx.on_action(|_: &ToggleFullScreen, cx| with_active_window(cx, |w| w.toggle_fullscreen()));
    // ⌘K / ⌘P and ⌘⌥E from a gpui-component dialog (bound in its `Dialog`
    // context, see keymap): the main window's root isn't on the dialog's
    // dispatch path, so they end here. In the `Workspace` context the root
    // view takes them first. (⌘, ends in ToggleSettings above either way.)
    cx.on_action(|_: &TogglePalette, cx| {
        with_main_root(cx, |root, window, cx| root.toggle_palette(window, cx));
    });
    cx.on_action(|_: &ToggleQueryHistory, cx| {
        with_main_root(cx, |root, window, cx| root.toggle_query_history(window, cx));
    });
    cx.on_action(|_: &CloseWindow, cx| {
        if let Some(window) = cx.active_window() {
            // The window is being updated (key or menu dispatch): after it.
            cx.defer(move |cx| close_window(window, cx));
        }
    });
    // Tauri lib.rs `RunEvent::Exit`: the process exits without dropping the
    // backend (the AppState global lives on), so kill the SSH tunnels now.
    // Runs for every way out: ⌘Q, the menu, the last window, the Dock, logout.
    cx.on_app_quit(|cx| {
        close_tunnels(cx);
        std::future::ready(())
    })
    .detach();
}

/// Kills every SSH tunnel (`ssh -N` child) of the backend; at quit.
fn close_tunnels(cx: &mut App) {
    if let Some(state) = AppState::try_global(cx) {
        state.backend.close_tunnels();
    }
    cx.default_global::<QuitState>().tunnels_closed = true;
}

/// Whether the quit hook killed the SSH tunnels (tests).
pub fn tunnels_closed(cx: &App) -> bool {
    cx.try_global::<QuitState>()
        .is_some_and(|q| q.tunnels_closed)
}

/// Close Window (⌘W, File / Window menu) on `window`. The main window asks
/// like its close button (it is the last window: closing it quits), and
/// stays while a dialog is open in it, as a macOS window with a sheet: ⌘W in
/// a gpui-component dialog (outside the `Workspace` key context, so
/// unhandled) reaches the menu's ⌘W and ends here.
pub fn close_window(window: AnyWindowHandle, cx: &mut App) {
    let is_main = crate::views::guard_dialogs::guard_window(cx)
        .is_some_and(|main| main.window_id() == window.window_id());
    if is_main {
        if main_overlay_open(window, cx) {
            return;
        }
        if !main_window_should_close(cx) {
            return;
        }
    }
    window.update(cx, |_, w, _| w.remove_window()).ok();
}

/// A dialog-like overlay is open in the main window ([`RootView::overlay_open`]).
fn main_overlay_open(window: AnyWindowHandle, cx: &mut App) -> bool {
    window
        .update(cx, |view, window, cx| {
            let root = view.downcast::<Root>().ok()?;
            let main = root.read(cx).view().clone().downcast::<RootView>().ok()?;
            Some(main.update(cx, |main, cx| main.overlay_open(window, cx)))
        })
        .ok()
        .flatten()
        .unwrap_or(false)
}

// ── Quitting with transactions open (Tauri lib.rs `RunEvent::ExitRequested`,
// GuardDialogs.tsx `useQuitGuard` / `confirmQuit`) ─────────────────────────

/// The quit gate and whether the app is on its way out ([`QuitGuard`],
/// shared with the Dock / logout hook).
#[derive(Debug, Default)]
struct QuitState {
    guard: Rc<QuitGuard>,
    /// The quit hook ran (`close_tunnels`).
    tunnels_closed: bool,
}

impl Global for QuitState {}

fn epoch_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

/// ⌘Q / Boa → Quit Boa: with transactions open, asks first ("Quit with an
/// open transaction?" → roll back and quit, or cancel). A second request
/// within 3 s quits anyway (`QuitGate`), so a stuck UI can't keep the app
/// open; without the main window it quits at once.
///
/// macOS quits that don't go through the menu or ⌘Q (Dock → Quit, logging
/// out, restarting) end in [`terminate_handler`], which shares the gate.
pub fn request_quit(cx: &mut App) {
    let decision = quit_guard(cx).request(epoch_ms());
    if decision == QuitDecision::Quit || crate::views::guard_dialogs::guard_window(cx).is_none() {
        quit_now(cx);
        return;
    }
    confirm_quit(cx);
}

/// The main window's close button (it is the last window: closing it quits):
/// true to close now; false while asking about open transactions (then the
/// app quits if the user agrees).
pub fn main_window_should_close(cx: &mut App) -> bool {
    if is_quitting(cx) {
        return true;
    }
    let open = AppState::try_global(cx)
        .is_some_and(|s| !s.transactions.read(cx).open_transactions(None).is_empty());
    if !open {
        return true;
    }
    confirm_quit(cx);
    false
}

/// The app is quitting (asked and answered, or nothing to ask).
pub fn is_quitting(cx: &App) -> bool {
    cx.try_global::<QuitState>()
        .is_some_and(|q| q.guard.is_quitting())
}

fn quit_guard(cx: &mut App) -> Rc<QuitGuard> {
    cx.default_global::<QuitState>().guard.clone()
}

/// The answer to AppKit's terminate requests: Dock → Quit, logging out,
/// restarting (Tauri `RunEvent::ExitRequested`; installed by `run` through
/// [`crate::platform::terminate::intercept`]). Keeps the guard's
/// open-transaction flag in step with the Transactions model and returns a
/// function that answers without borrowing the `App`
/// ([`crate::platform::terminate::decide`]): Now with nothing open, once
/// quitting, or asked within 3 s; else Cancel, and then the app asks as ⌘Q
/// does ("Quit with an open transaction?" → roll back and quit).
pub fn terminate_handler(cx: &mut App) -> impl Fn() -> TerminateReply + use<> {
    let guard = quit_guard(cx);
    if let Some(state) = AppState::try_global(cx).cloned() {
        guard.set_open_tx(
            !state
                .transactions
                .read(cx)
                .open_transactions(None)
                .is_empty(),
        );
        let mirror = guard.clone();
        cx.observe(&state.transactions, move |transactions, cx| {
            let open = !transactions.read(cx).open_transactions(None).is_empty();
            mirror.set_open_tx(open);
        })
        .detach();
    }
    let (ask, mut asks) = futures::channel::mpsc::unbounded::<()>();
    cx.spawn(async move |cx| {
        while asks.next().await.is_some() {
            cx.update(|cx| {
                // (No main window holds a transaction open in practice: it
                // closes only with none open, or once quitting.)
                if crate::views::guard_dialogs::guard_window(cx).is_none() {
                    quit_now(cx);
                } else {
                    confirm_quit(cx);
                }
            });
        }
    })
    .detach();
    move || {
        let reply = guard.should_terminate(epoch_ms());
        if reply == TerminateReply::Cancel {
            let _ = ask.unbounded_send(());
        }
        reply
    }
}

/// Asks about open transactions (nothing open: no question), then rolls
/// them back and quits unless cancelled.
fn confirm_quit(cx: &mut App) {
    let ask = crate::views::guard_dialogs::confirm_close_tx(CloseAction::Quit, None, cx);
    cx.spawn(async move |cx| {
        if !ask.await {
            return;
        }
        let rollback = cx.update(roll_back_all);
        rollback.await;
        cx.update(quit_now);
    })
    .detach();
}

/// Rolls back every open transaction: closes the sessions of each
/// connection that has one (`backend.disconnect`; the connections stay
/// "wanted", so the next launch reconnects them).
pub(crate) fn roll_back_all(cx: &mut App) -> Task<()> {
    let Some(state) = AppState::try_global(cx).cloned() else {
        return Task::ready(());
    };
    let mut ids: Vec<String> = state
        .transactions
        .read(cx)
        .open_transactions(None)
        .into_iter()
        .map(|o| o.connection_id)
        .collect();
    ids.sort();
    ids.dedup();
    let calls: Vec<_> = ids.iter().map(|id| state.backend.disconnect(id)).collect();
    cx.foreground_executor().spawn(async move {
        futures::future::join_all(calls).await;
    })
}

pub(crate) fn quit_now(cx: &mut App) {
    quit_guard(cx).set_quitting();
    cx.quit();
}

/// Runs `f` on the active window, after the current dispatch: a menu item
/// or key is dispatched while that window is being updated, when updating
/// it again fails.
fn with_active_window(cx: &mut App, f: impl FnOnce(&mut gpui::Window) + 'static) {
    if let Some(window) = cx.active_window() {
        cx.defer(move |cx| {
            window.update(cx, |_, window, _| f(window)).ok();
        });
    }
}

/// Runs `f` on the main window's [`RootView`], after the current dispatch
/// (the window is being updated while a key or menu item is dispatched).
fn with_main_root(
    cx: &mut App,
    f: impl FnOnce(&mut RootView, &mut gpui::Window, &mut gpui::Context<RootView>) + 'static,
) {
    // Looked up after the dispatch too: the window being updated can't be read.
    cx.defer(move |cx| {
        let Some(main) = main_window(cx) else { return };
        // Through the untyped handle, so `Root` isn't leased (the dialog
        // layer reads it).
        let main: AnyWindowHandle = main.into();
        main.update(cx, |view, window, cx| {
            let root = view.downcast::<Root>().ok()?;
            let view = root.read(cx).view().clone().downcast::<RootView>().ok()?;
            view.update(cx, |view, cx| f(view, window, cx));
            Some(())
        })
        .ok();
    });
}

/// View › Enter Full Screen / Exit Full Screen: Tauri's menu item is
/// AppKit's `toggleFullScreen:`, which AppKit relabels by the key window's
/// state. GPUI's menu items are plain actions, so the main window's root
/// calls this when its bounds or activation change and the menu bar is set
/// again when the label must change.
pub fn sync_full_screen_menu(full_screen: bool, cx: &mut App) {
    let shown = cx.default_global::<MenuState>();
    if shown.full_screen == full_screen {
        return;
    }
    shown.full_screen = full_screen;
    cx.set_menus(app_menus_for(full_screen));
}

/// What the menu bar shows that depends on state.
#[derive(Debug, Default)]
struct MenuState {
    full_screen: bool,
}

impl Global for MenuState {}

/// The View menu's item for `full_screen`.
pub fn full_screen_label(full_screen: bool) -> &'static str {
    if full_screen {
        "Exit Full Screen"
    } else {
        "Enter Full Screen"
    }
}

/// The main window (the one whose root is a [`RootView`]), if open.
pub fn main_window(cx: &App) -> Option<WindowHandle<Root>> {
    cx.windows().into_iter().find_map(|w| {
        let handle = w.downcast::<Root>()?;
        let is_main = handle
            .read(cx)
            .ok()
            .is_some_and(|root| root.view().clone().downcast::<RootView>().is_ok());
        is_main.then_some(handle)
    })
}

/// The menu bar, as updater.rs builds it (Tauri's default menu with About Boa
/// and Check for Updates… at the top of the app menu), plus Settings… (⌘,).
pub fn app_menus() -> Vec<Menu> {
    app_menus_for(false)
}

/// [`app_menus`] with View's item labelled for `full_screen`.
pub fn app_menus_for(full_screen: bool) -> Vec<Menu> {
    vec![
        Menu::new(APP_NAME).items([
            MenuItem::action("About Boa", About),
            MenuItem::action("Check for Updates…", CheckForUpdates),
            MenuItem::separator(),
            MenuItem::action("Settings…", ToggleSettings),
            MenuItem::separator(),
            MenuItem::os_submenu("Services", SystemMenuType::Services),
            MenuItem::separator(),
            MenuItem::action("Hide Boa", Hide),
            MenuItem::action("Hide Others", HideOthers),
            MenuItem::action("Show All", ShowAll),
            MenuItem::separator(),
            MenuItem::action("Quit Boa", Quit),
        ]),
        Menu::new("File").items([MenuItem::action("Close Window", CloseWindow)]),
        Menu::new("Edit").items([
            MenuItem::os_action("Undo", EditUndo, OsAction::Undo),
            MenuItem::os_action("Redo", EditRedo, OsAction::Redo),
            MenuItem::separator(),
            MenuItem::os_action("Cut", EditCut, OsAction::Cut),
            MenuItem::os_action("Copy", EditCopy, OsAction::Copy),
            MenuItem::os_action("Paste", EditPaste, OsAction::Paste),
            MenuItem::os_action("Select All", EditSelectAll, OsAction::SelectAll),
        ]),
        Menu::new("View").items([MenuItem::action(
            full_screen_label(full_screen),
            ToggleFullScreen,
        )]),
        Menu::new("Window").items([
            MenuItem::action("Minimize", Minimize),
            MenuItem::action("Zoom", Zoom),
            MenuItem::separator(),
            MenuItem::action("Close Window", CloseWindow),
        ]),
        Menu::new("Help").items([]),
    ]
}

/// Window options for the main window (HANDOFF §5): 1280×820, min 720×480,
/// transparent title bar with the traffic lights at Tauri's place. The app
/// draws the title row itself and moves the window from its empty parts
/// (`app_owns_titlebar_drag`; see `views::chrome::WindowDrag`).
pub fn main_window_options(cx: &App) -> WindowOptions {
    let (w, h) = WINDOW_SIZE;
    let (min_w, min_h) = WINDOW_MIN_SIZE;
    let (light_x, light_y) = TRAFFIC_LIGHTS;
    WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
            None,
            size(px(w), px(h)),
            cx,
        ))),
        window_min_size: Some(size(px(min_w), px(min_h))),
        titlebar: Some(TitlebarOptions {
            // Hidden by the transparent title bar; still names the window in
            // the Window menu and Mission Control.
            title: Some(APP_NAME.into()),
            appears_transparent: true,
            traffic_light_position: Some(point(px(light_x), px(light_y))),
        }),
        app_owns_titlebar_drag: true,
        app_id: Some("dev.boa.desktop".into()),
        focus: !background(),
        show: !background(),
        ..Default::default()
    }
}

/// Opens the main window: a [`Root`] (gpui-component's layer for dialogs,
/// sheets and notifications) around the [`RootView`]. Needs [`AppState`].
pub fn open_main_window(cx: &mut App) -> anyhow::Result<WindowHandle<Root>> {
    let options = main_window_options(cx);
    cx.open_window(options, |window, cx| {
        let root = cx.new(|cx| RootView::new(window, cx));
        root.update(cx, |root, cx| root.focus_editor(window, cx));
        cx.new(|cx| Root::new(root, window, cx))
    })
}

#[cfg(feature = "screenshot")]
mod screenshot {
    use std::path::PathBuf;
    use std::time::Duration;

    use gpui::{App, AppContext as _, WindowHandle};
    use gpui_component::Root;

    /// Waits for a few frames (`BOA_SCREENSHOT_DELAY_MS`, default 1500),
    /// writes the window's last frame to `path`, quits.
    pub fn capture_and_quit(window: WindowHandle<Root>, path: PathBuf, cx: &mut App) {
        let delay = std::env::var("BOA_SCREENSHOT_DELAY_MS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(1500);
        cx.spawn(async move |cx| {
            cx.background_executor()
                .timer(Duration::from_millis(delay))
                .await;
            // BOA_SCREENSHOT_WINDOW=about: the About window instead.
            let about = std::env::var("BOA_SCREENSHOT_WINDOW").is_ok_and(|w| w == "about");
            let window = if about {
                cx.update(|cx| crate::views::about::window(cx))
                    .unwrap_or(window)
            } else {
                window
            };
            // A fresh frame: a hidden or occluded window draws none by itself.
            // Not `WindowHandle::update`: that leases the root view, and
            // drawing needs it.
            let result = cx.update_window(window.into(), |_, window, cx| {
                window.refresh();
                window.draw(cx).clear(cx);
                window.render_to_image()
            });
            match result {
                Ok(Ok(image)) => match image.save(&path) {
                    Ok(()) => tracing::info!("screenshot written to {}", path.display()),
                    Err(err) => tracing::error!("saving screenshot: {err}"),
                },
                Ok(Err(err)) => tracing::error!("render_to_image: {err:#}"),
                Err(err) => tracing::error!("window gone: {err:#}"),
            }
            cx.update(|cx| cx.quit());
        })
        .detach();
    }
}
