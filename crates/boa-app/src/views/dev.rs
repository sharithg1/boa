//! Development hooks for the chrome (feature `dev-hooks`, implied by
//! `screenshot`): script the window's state from the environment before a
//! capture. Never compiled into release builds.
//!
//! - `BOA_DEV_PANELS=all|none|sidebar,right,bottom`: which panels show
//!   (overrides session.json for this run; not saved).
//! - `BOA_DEV_SETTINGS=general|connections|keyboard`: open Settings on that page.
//! - `BOA_DEV_TOAST=kind|title|body` (kind: info, success, error; `;` separates
//!   several toasts, `action=Label` as a fourth field adds a button).
//! - `BOA_DEV_ABOUT=1|check`: open the About window too (`check`: and check
//!   for updates). `BOA_DEV_UPDATE=ready`: pretend the update on
//!   BOA_UPDATE_ENDPOINT downloaded (the status bar pill).
//! - `BOA_DEV_SETTINGS=search:<text>`: Settings with <text> searched.
//! - `BOA_DEV_HISTORY=overlay|panel`: see [`crate::views::query_history::dev`].
//! - `BOA_DEV_EXPORT=menu|progress|done`: see [`crate::views::csv_export::dev`].
//! - `BOA_DEV_PALETTE=open|<query>`: see [`crate::views::command_palette::dev`].

use gpui::App;

use crate::app_state::AppState;
use crate::models::layout::{PanelKind, SettingsPage};
use crate::models::toasts::{NewToast, ToastKind};

fn env(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.trim().is_empty())
}

/// Applies the `BOA_DEV_*` variables (call once the main window is open).
pub fn apply(cx: &mut App) {
    let Some(state) = AppState::try_global(cx).cloned() else {
        return;
    };
    if let Some(panels) = env("BOA_DEV_PANELS") {
        let panels = panels.to_lowercase();
        let want = |name: &str| panels == "all" || panels.split(',').any(|p| p.trim() == name);
        state.layout.update(cx, |l, cx| {
            for (panel, name) in [
                (PanelKind::Sidebar, "sidebar"),
                (PanelKind::RightPanel, "right"),
                (PanelKind::Bottom, "bottom"),
            ] {
                l.set_open(panel, want(name), cx);
            }
        });
        // A scripted layout is not the user's: don't save it.
        state.session.update(cx, |s, _| s.keep_saved_panels());
    }
    if let Some(page) = env("BOA_DEV_SETTINGS") {
        // `search:<text>`: General with <text> in the search field.
        let search = page.trim().strip_prefix("search:").map(str::to_owned);
        let page = match page.trim() {
            "connections" => SettingsPage::Connections,
            "keyboard" => SettingsPage::Keyboard,
            _ => SettingsPage::General,
        };
        state
            .layout
            .update(cx, |l, cx| l.set_settings(Some(page), cx));
        if let Some(text) = search
            && let Some(main) = crate::app::main_window(cx)
        {
            main.update(cx, |root, window, cx| {
                if let Ok(view) = root
                    .view()
                    .clone()
                    .downcast::<crate::views::root::RootView>()
                {
                    let search = view.read(cx).settings().read(cx).search().clone();
                    search.update(cx, |s, cx| s.set_value(text, window, cx));
                }
            })
            .ok();
        }
    }
    // `BOA_DEV_UPDATE=ready`: a pretend downloaded update (with
    // BOA_UPDATE_ENDPOINT on a newer latest.json), for the status bar pill.
    if env("BOA_DEV_UPDATE").as_deref() == Some("ready") {
        state.updates.update(cx, |u, cx| {
            u.set_downloader(Some(std::rc::Rc::new(|_, _| gpui::Task::ready(Ok(())))));
            u.check(false, cx);
        });
    }
    if let Some(toasts) = env("BOA_DEV_TOAST") {
        state.toasts.update(cx, |t, cx| {
            for spec in toasts.split(';') {
                let mut fields = spec.split('|');
                let kind = match fields.next().unwrap_or("").trim() {
                    "success" => ToastKind::Success,
                    "error" => ToastKind::Error,
                    _ => ToastKind::Info,
                };
                let mut toast = NewToast::new(kind, fields.next().unwrap_or("Toast").to_owned());
                if let Some(body) = fields.next().filter(|b| !b.is_empty()) {
                    toast = toast.body(body.to_owned());
                }
                if let Some(label) = fields.next().and_then(|a| a.strip_prefix("action=")) {
                    toast = toast.action(label.to_owned(), |_, _| {});
                }
                t.push_for(toast, None, cx);
            }
        });
    }
    if let Some(about) = env("BOA_DEV_ABOUT") {
        crate::views::about::open(cx);
        // `check`: as Boa → Check for Updates….
        if about.trim() == "check" {
            state.updates.update(cx, |u, cx| u.check(true, cx));
        }
    }
    crate::views::query_history::dev::apply(cx);
    crate::views::csv_export::dev::apply(cx);
    crate::views::command_palette::dev::apply(cx);
}
