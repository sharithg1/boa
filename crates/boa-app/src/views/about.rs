//! AboutWindow.tsx: the About window (Boa → About Boa / Check for Updates…),
//! its own window (updater.rs `open_about`: 400×330, not resizable, not
//! minimizable, overlay title bar). Icon, name, version and architecture,
//! the build's commit date, "Check for Updates…" (or "Restart to Update"
//! once an update is downloaded), "Copy version info" (Return copies, as
//! the ↵ on the button says), and the update status line, all from
//! [`boa_core::state::updates`] on the [`Updates`] model.
//!
//! Also the status bar's "Restart to update" pill ([`update_pill`]).
//!
//! [`Updates`]: crate::models::updates::Updates

use std::time::Duration;

pub use boa_core::state::updates::{AppInfo, arch_label, version_info_text};
use boa_core::state::updates::{
    UpdateEvent, about_busy, about_status_line, about_version_line, release_date_label,
    update_pill_label, update_pill_title,
};
use gpui::prelude::*;
use gpui::{
    AnyElement, App, Bounds, ClipboardItem, Context, FocusHandle, Focusable, FontWeight, Global,
    MouseButton, SharedString, Subscription, Task, TitlebarOptions, Window, WindowBounds,
    WindowHandle, WindowOptions, div, img, px, size,
};
use gpui_component::spinner::Spinner;
use gpui_component::{Icon, Root, Sizable as _};

use super::chrome::{IconName, WindowDrag};
use crate::actions::CopyVersionInfo;
use crate::app_state::AppState;
use crate::models::updates::is_ready;
use crate::models::workspaces::now_ms;
use crate::theme::{self, ActivePalette as _};

/// The About window's size (updater.rs `inner_size(400.0, 330.0)`).
pub const ABOUT_SIZE: (f32, f32) = (400., 330.);

/// How long "Copied" shows.
const COPIED_FOR: Duration = Duration::from_millis(1500);
/// How long "Restarting…" shows after "Restart to Update" (the main window
/// may ask about open transactions, and the user may cancel).
const HANDED_OVER_FOR: Duration = Duration::from_millis(4000);

/// This build's version details (updater.rs `app_info`).
pub fn current_info() -> AppInfo {
    AppInfo {
        name: "boa-desktop".to_owned(),
        version: crate::app::VERSION.to_owned(),
        arch: std::env::consts::ARCH.to_owned(),
        commit: env!("BOA_BUILD_COMMIT").to_owned(),
        commit_date: env!("BOA_BUILD_DATE").to_owned(),
        os: os_version(),
    }
}

/// `macOS 15.4.1` (`sw_vers -productVersion`), else the OS name.
pub fn os_version() -> String {
    std::process::Command::new("sw_vers")
        .arg("-productVersion")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| format!("macOS {}", String::from_utf8_lossy(&o.stdout).trim()))
        .unwrap_or_else(|| std::env::consts::OS.to_owned())
}

/// The last update event, if any check ran.
fn update_status(cx: &App) -> Option<UpdateEvent> {
    AppState::try_global(cx).and_then(|s| s.updates.read(cx).status().cloned())
}

fn with_updates(
    cx: &mut App,
    f: impl FnOnce(&mut crate::models::updates::Updates, &mut Context<crate::models::updates::Updates>),
) {
    if let Some(state) = AppState::try_global(cx) {
        let updates = state.updates.clone();
        updates.update(cx, f);
    }
}

/// The About window, if one is open.
#[derive(Debug, Default)]
struct OpenAbout(Option<WindowHandle<Root>>);

impl Global for OpenAbout {}

/// The About window, if open.
pub fn window(cx: &App) -> Option<WindowHandle<Root>> {
    cx.try_global::<OpenAbout>().and_then(|a| a.0)
}

/// Opens the About window, or brings the open one to the front.
pub fn open(cx: &mut App) {
    if let Some(handle) = cx.default_global::<OpenAbout>().0
        && handle
            .update(cx, |_, window, _| crate::app::activate_window(window))
            .is_ok()
    {
        return;
    }
    let (w, h) = ABOUT_SIZE;
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
            None,
            size(px(w), px(h)),
            cx,
        ))),
        titlebar: Some(TitlebarOptions {
            title: Some("About Boa".into()),
            appears_transparent: true,
            traffic_light_position: None,
        }),
        app_owns_titlebar_drag: true,
        is_resizable: false,
        is_minimizable: false,
        focus: !crate::app::background(),
        show: !crate::app::background(),
        ..Default::default()
    };
    let opened = cx.open_window(options, |window, cx| {
        let view = cx.new(|cx| AboutView::new(current_info(), cx));
        window.focus(&view.focus_handle(cx), cx);
        cx.new(|cx| Root::new(view, window, cx))
    });
    match opened {
        Ok(handle) => cx.set_global(OpenAbout(Some(handle))),
        Err(err) => tracing::error!("failed to open About: {err:#}"),
    }
}

/// The About window's view.
pub struct AboutView {
    focus_handle: FocusHandle,
    info: AppInfo,
    copied: bool,
    copied_timer: Option<Task<()>>,
    /// "Restart to Update" was pressed in the last 4 s.
    handed_over: bool,
    handed_over_timer: Option<Task<()>>,
    drag: WindowDrag,
    _subscriptions: Vec<Subscription>,
}

impl std::fmt::Debug for AboutView {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AboutView")
            .field("info", &self.info)
            .field("copied", &self.copied)
            .finish_non_exhaustive()
    }
}

impl AboutView {
    pub fn new(info: AppInfo, cx: &mut Context<Self>) -> Self {
        let mut subscriptions = Vec::new();
        if let Some(state) = AppState::try_global(cx) {
            let updates = state.updates.clone();
            subscriptions.push(cx.observe(&updates, |_, _, cx| cx.notify()));
        }
        Self {
            focus_handle: cx.focus_handle(),
            info,
            copied: false,
            copied_timer: None,
            handed_over: false,
            handed_over_timer: None,
            drag: WindowDrag::new(),
            _subscriptions: subscriptions,
        }
    }

    pub fn info(&self) -> &AppInfo {
        &self.info
    }

    pub fn copied(&self) -> bool {
        self.copied
    }

    /// The status line under the buttons, if any.
    pub fn status_line(&self, cx: &App) -> Option<String> {
        about_status_line(update_status(cx).as_ref(), self.handed_over)
    }

    /// "Copy version info": writes [`version_info_text`] to the clipboard.
    pub fn copy(&mut self, cx: &mut Context<Self>) {
        cx.write_to_clipboard(ClipboardItem::new_string(version_info_text(&self.info)));
        self.copied = true;
        self.copied_timer = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(COPIED_FOR).await;
            this.update(cx, |this, cx| {
                this.copied = false;
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    /// "Restart to Update": hands over to the main window, which asks about
    /// open transactions first ([`Updates::restart_to_update`]).
    ///
    /// [`Updates::restart_to_update`]: crate::models::updates::Updates::restart_to_update
    pub fn install(&mut self, cx: &mut Context<Self>) {
        self.handed_over = true;
        self.handed_over_timer = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(HANDED_OVER_FOR).await;
            this.update(cx, |this, cx| {
                this.handed_over = false;
                cx.notify();
            })
            .ok();
        }));
        with_updates(cx, |u, cx| u.restart_to_update(false, cx));
        cx.notify();
    }

    fn button(
        id: &'static str,
        primary: bool,
        disabled: bool,
        label: impl Into<SharedString>,
        cx: &App,
    ) -> gpui::Stateful<gpui::Div> {
        let p = cx.palette();
        let (hover_primary, hover_outline) = (p.body.opacity(0.9), p.hover_strong);
        div()
            .id(id)
            .flex()
            .items_center()
            .gap(px(6.))
            .h(px(32.))
            .px_3()
            .rounded(theme::radius::MD)
            .text_size(theme::text::SM)
            .font_weight(FontWeight::MEDIUM)
            .when(!disabled, |d| d.cursor_pointer())
            .when(disabled, |d| d.opacity(0.5))
            .when(primary, |d| {
                d.bg(p.body)
                    .text_color(p.background)
                    .when(!disabled, |d| d.hover(move |s| s.bg(hover_primary)))
            })
            .when(!primary, |d| {
                d.border_1()
                    .border_color(p.input)
                    .bg(p.background.opacity(0.3))
                    .text_color(p.body)
                    .when(!disabled, |d| d.hover(move |s| s.bg(hover_outline)))
            })
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(label.into())
    }
}

impl Focusable for AboutView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for AboutView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = cx.palette().clone();
        let status = update_status(cx);
        let busy = about_busy(status.as_ref());
        let ready = is_ready(status.as_ref());
        let line = about_status_line(status.as_ref(), self.handed_over);
        let date = release_date_label(&self.info.commit_date, now_ms() as i64);
        let spinner = |color| Spinner::new().with_size(px(14.)).color(color);
        let update_button = if ready {
            let handed_over = self.handed_over;
            Self::button("about-install", true, handed_over, "Restart to Update", cx)
                .when(handed_over, |d| d.child(spinner(p.background)))
                .when(!handed_over, |d| {
                    d.on_click(cx.listener(|this, _, _, cx| this.install(cx)))
                })
        } else {
            Self::button("about-check", false, busy, "Check for Updates…", cx)
                .when(busy, |d| d.child(spinner(p.body)))
                .when(!busy, |d| {
                    d.on_click(|_, _, cx| with_updates(cx, |u, cx| u.check(true, cx)))
                })
        };
        let copy = Self::button(
            "about-copy",
            !ready,
            false,
            if self.copied {
                "Copied"
            } else {
                "Copy version info"
            },
            cx,
        )
        .on_click(cx.listener(|this, _, _, cx| this.copy(cx)))
        .child(
            Icon::new(if self.copied {
                IconName::Check
            } else {
                IconName::CornerDownLeft
            })
            .size(px(14.)),
        );
        div()
            .id("about")
            .key_context(crate::keymap::ABOUT)
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(|this, _: &CopyVersionInfo, _, cx| this.copy(cx)))
            // Return copies, once per press (`e.key === 'Enter' && !e.repeat`):
            // a key-down listener, as a key binding fires on every repeat.
            .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, _, cx| {
                if event.keystroke.key == "enter" && !event.is_held {
                    cx.stop_propagation();
                    this.copy(cx);
                }
            }))
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .items_center()
            .bg(p.card)
            .px_6()
            .pt_9()
            .pb_5()
            .font_family(theme::UI_FONT)
            .text_color(p.body)
            .child(
                self.drag.apply(
                    div()
                        .id("about-drag")
                        .absolute()
                        .top_0()
                        .left_0()
                        .right_0()
                        .h(px(32.)),
                ),
            )
            .child(img("boa-icon.png").size(px(64.)))
            .child(
                div()
                    .mt_3()
                    .text_size(theme::text::XL)
                    .line_height(theme::text::XL_LINE)
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(crate::app::APP_NAME),
            )
            .child(
                div()
                    .mt_1()
                    .text_size(theme::text::BASE)
                    .text_color(p.soft)
                    .child(about_version_line(&self.info)),
            )
            .child(
                div()
                    .mt_1()
                    .h(theme::text::XS_LINE)
                    .text_size(theme::text::XS)
                    .text_color(p.dim)
                    .child(date),
            )
            .child(div().mt_5().flex().gap_2().child(update_button).child(copy))
            .child(
                div()
                    .mt_3()
                    .min_h(px(36.))
                    .max_w_full()
                    .text_center()
                    .text_size(theme::text::XS)
                    .line_height(px(18.))
                    .text_color(p.quiet)
                    .children(line),
            )
    }
}

/// StatusBar.tsx `UpdatePill`: "Restart to update to 0.7.0" once a newer
/// Boa is downloaded; None otherwise. A click runs
/// [`Updates::restart_to_update`]; while that asks / installs the pill says
/// "Restarting…" and is disabled.
///
/// [`Updates::restart_to_update`]: crate::models::updates::Updates::restart_to_update
pub fn update_pill(cx: &App) -> Option<AnyElement> {
    let status = update_status(cx)?;
    if !is_ready(Some(&status)) {
        return None;
    }
    let version = status.version.clone().unwrap_or_default();
    let installing = AppState::try_global(cx).is_some_and(|s| s.updates.read(cx).pill_restarting());
    let p = cx.palette();
    let hover = p.body.opacity(0.9);
    Some(
        div()
            .id("update-pill")
            .debug_selector(|| "update-pill".into())
            .ml_1()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(6.))
            .h(px(24.))
            .px_2()
            .rounded(theme::radius::MD)
            .bg(p.body)
            .when(installing, |d| d.opacity(0.6))
            .when(!installing, |d| {
                d.cursor_pointer()
                    .hover(move |s| s.bg(hover))
                    .on_click(|_, _, cx| with_updates(cx, |u, cx| u.restart_to_update(true, cx)))
            })
            .text_color(p.background)
            .text_size(theme::text::XS)
            .font_weight(FontWeight::MEDIUM)
            .tooltip(super::chrome::tooltip(update_pill_title(&version)))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(
                Icon::default()
                    .path("icons/arrow-down-to-line.svg")
                    .size(px(14.)),
            )
            .child(update_pill_label(&version, installing))
            .into_any_element(),
    )
}
