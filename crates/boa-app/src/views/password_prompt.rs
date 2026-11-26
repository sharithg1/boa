//! ConnectionDialog.tsx `PasswordPrompt`: asks for the password of a
//! connection when the server wants one that isn't stored, or refused the
//! one it got (the model's [`PasswordPromptState`]). "Save password" stores
//! it in connections.json before connecting.
//!
//! [`subscribe`] (called once by the main window's root view) opens it on
//! [`ConnectionsEvent::PasswordPrompt`]; it closes itself once the model's
//! prompt is gone (connected) and tells the model when it is dismissed.

use gpui::prelude::*;
use gpui::{
    App, Context, Entity, FocusHandle, Focusable, Global, Subscription, WeakEntity, Window, div, px,
};
use gpui_component::button::{Button, ButtonVariants as _};
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::spinner::Spinner;
use gpui_component::{Disableable as _, Sizable as _, WindowExt as _};

use super::checkbox;
use super::chrome::{IconName, icon};
use super::connection_dialog::{self, md_button};
use crate::app_state::AppState;
use crate::models::connections::{Connections, ConnectionsEvent, PasswordPromptState};
use crate::theme::{self, ActivePalette as _};

/// The prompt's width (`sm:max-w-[400px]`).
pub const PROMPT_WIDTH: f32 = 400.;

/// The prompt's content view.
pub struct PasswordPrompt {
    focus_handle: FocusHandle,
    connections: Entity<Connections>,
    connection_id: String,
    input: Entity<InputState>,
    save: bool,
    busy: bool,
    closed: bool,
    _subscriptions: Vec<Subscription>,
}

impl std::fmt::Debug for PasswordPrompt {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PasswordPrompt")
            .field("connection_id", &self.connection_id)
            .field("save", &self.save)
            .field("busy", &self.busy)
            .finish_non_exhaustive()
    }
}

/// The prompt showing, if any (one at a time).
#[derive(Default)]
struct ActivePrompt(Option<WeakEntity<PasswordPrompt>>);

impl Global for ActivePrompt {}

impl PasswordPrompt {
    pub fn new(
        connection_id: String,
        connections: Entity<Connections>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Password")
                .masked(true)
        });
        let subscriptions = vec![
            cx.subscribe_in(&input, window, |this, _, e: &InputEvent, window, cx| {
                if matches!(e, InputEvent::PressEnter { .. }) {
                    this.submit(window, cx);
                }
            }),
            cx.observe_in(&connections, window, |this, connections, window, cx| {
                let still = connections
                    .read(cx)
                    .password_prompt()
                    .is_some_and(|p| p.connection_id == this.connection_id);
                if !still && !this.closed {
                    this.closed = true;
                    window.close_dialog(cx);
                }
                cx.notify();
            }),
        ];
        Self {
            focus_handle: cx.focus_handle(),
            connections,
            connection_id,
            input,
            save: false,
            busy: false,
            closed: false,
            _subscriptions: subscriptions,
        }
    }

    /// The connection it asks for.
    pub fn connection_id(&self) -> &str {
        &self.connection_id
    }

    /// The password field.
    pub fn input(&self) -> &Entity<InputState> {
        &self.input
    }

    pub fn is_busy(&self) -> bool {
        self.busy
    }

    /// The "Save password" checkbox.
    pub fn set_save(&mut self, save: bool, cx: &mut Context<Self>) {
        self.save = save;
        cx.notify();
    }

    /// A new round (another refusal, or another connection): empty field, not busy.
    fn reset(&mut self, connection_id: String, window: &mut Window, cx: &mut Context<Self>) {
        self.connection_id = connection_id;
        self.busy = false;
        self.input.update(cx, |i, cx| i.set_value("", window, cx));
        cx.notify();
    }

    fn state<'a>(&self, cx: &'a App) -> Option<&'a PasswordPromptState> {
        self.connections
            .read(cx)
            .password_prompt()
            .filter(|p| p.connection_id == self.connection_id)
    }

    /// Connect (with the typed password; saved first when asked to).
    pub fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        self.busy = true;
        cx.notify();
        let password = self.input.read(cx).value().to_string();
        let (id, save) = (self.connection_id.clone(), self.save);
        let task = self
            .connections
            .update(cx, |c, cx| c.submit_password(&id, password, save, cx));
        cx.spawn_in(window, async move |this, cx| {
            let _ = task.await;
            this.update(cx, |this, cx| {
                this.busy = false;
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Cancel: closes the prompt.
    pub fn cancel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.connections
            .update(cx, |c, cx| c.dismiss_password_prompt(cx));
        if !self.closed {
            self.closed = true;
            window.close_dialog(cx);
        }
    }
}

impl Focusable for PasswordPrompt {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for PasswordPrompt {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = cx.palette().clone();
        let conn = self
            .connections
            .read(cx)
            .get(&self.connection_id)
            .map(|c| c.config.clone());
        let (title, target) = match &conn {
            Some(c) => (
                format!("Connect to {}", c.name),
                format!("{}@{}:{}/{}", c.user, c.host, c.port, c.database),
            ),
            None => ("Connect".to_owned(), String::new()),
        };
        let error = self.state(cx).and_then(|s| s.error.clone());
        // DialogHeader (flex-col gap-2): the title with dialog.tsx's close
        // button (a 16 px X at 70 %), then the target in mono text-xs.
        let close = div()
            .id("prompt-close")
            .flex_none()
            .mr(px(-2.))
            .rounded(px(2.))
            .cursor_pointer()
            .opacity(0.7)
            .hover(|s| s.opacity(1.))
            .tooltip(super::chrome::tooltip("Close"))
            .on_click(cx.listener(|this, _, window, cx| this.cancel(window, cx)))
            .child(icon(IconName::X, 16., p.body));
        div()
            .track_focus(&self.focus_handle)
            .flex()
            .flex_col()
            .gap(px(12.))
            .text_color(p.body)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(8.))
                    .child(
                        div()
                            .flex()
                            .items_start()
                            .justify_between()
                            .gap(px(8.))
                            .child(
                                div()
                                    .text_size(theme::text::BASE)
                                    .line_height(theme::text::BASE_LINE)
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .child(title),
                            )
                            .child(close),
                    )
                    .child(
                        div()
                            .font_family(theme::MONO_FONT)
                            .text_size(theme::text::XS)
                            .line_height(theme::text::XS_LINE)
                            .text_color(p.quiet)
                            .child(target),
                    ),
            )
            // input.tsx: h-9, px-3, text-sm, `dark:bg-input/30`.
            .child(
                Input::new(&self.input)
                    .w_full()
                    .large()
                    .bg(p.input.opacity(0.3)),
            )
            .child(
                div().flex().child(checkbox::sized(
                    checkbox::checkbox("prompt-save-password", self.save, "Save password", &p)
                        .on_click(cx.listener(|this, on: &bool, _, cx| this.set_save(*on, cx))),
                )),
            )
            .children(error.map(|error| {
                div()
                    .flex()
                    .items_start()
                    .gap(px(8.))
                    .text_size(theme::text::SM)
                    .line_height(theme::text::SM_LINE)
                    .text_color(p.status_err_text)
                    .child(div().mt(px(2.)).child(icon(
                        IconName::CircleAlert,
                        16.,
                        p.status_err_text,
                    )))
                    .child(div().min_w_0().flex_1().child(error))
            }))
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap(px(8.))
                    .child(
                        md_button(Button::new("prompt-cancel").ghost(), "Cancel")
                            .on_click(cx.listener(|this, _, window, cx| this.cancel(window, cx))),
                    )
                    .child(
                        md_button(
                            Button::new("prompt-connect")
                                .primary()
                                .when(self.busy, |b| b.child(Spinner::new().with_size(px(14.)))),
                            "Connect",
                        )
                        .disabled(self.busy)
                        .on_click(cx.listener(|this, _, window, cx| this.submit(window, cx))),
                    ),
            )
    }
}

/// Opens the prompt for `connection_id` (or refreshes the one showing).
pub fn open(
    connection_id: String,
    window: &mut Window,
    cx: &mut App,
) -> Option<Entity<PasswordPrompt>> {
    let connections = AppState::try_global(cx)?.connections.clone();
    if let Some(existing) = cx
        .try_global::<ActivePrompt>()
        .and_then(|a| a.0.as_ref())
        .and_then(WeakEntity::upgrade)
        .filter(|e| !e.read(cx).closed)
    {
        existing.update(cx, |p, cx| p.reset(connection_id, window, cx));
        return Some(existing);
    }
    let view = cx.new(|cx| PasswordPrompt::new(connection_id, connections.clone(), window, cx));
    cx.set_global(ActivePrompt(Some(view.downgrade())));
    let input = view.read(cx).input.clone();
    let first = std::cell::Cell::new(true);
    let content = view.clone();
    window.open_dialog(cx, move |dialog, window, cx| {
        if first.replace(false) {
            let handle = input.read(cx).focus_handle(cx);
            window.focus(&handle, cx);
        }
        let on_close = content.downgrade();
        let connections = connections.clone();
        connection_dialog::dialog_surface(dialog, cx)
            .w(px(PROMPT_WIDTH))
            .close_button(false)
            .on_close(move |_, _, cx| {
                // Esc, the backdrop or the ×: the prompt is dismissed.
                if let Some(view) = on_close.upgrade() {
                    view.update(cx, |p, _| p.closed = true);
                }
                connections.update(cx, |c, cx| c.dismiss_password_prompt(cx));
            })
            .child(content.clone())
    });
    Some(view)
}

/// Opens the prompt whenever the connections model asks for it. The
/// caller (the main window's root view) keeps the subscription.
pub fn subscribe<V: 'static>(window: &mut Window, cx: &mut Context<V>) -> Option<Subscription> {
    let connections = AppState::try_global(cx)?.connections.clone();
    Some(cx.subscribe_in(
        &connections,
        window,
        |_, _, event: &ConnectionsEvent, window, cx| {
            if let ConnectionsEvent::PasswordPrompt { id } = event {
                open(id.clone(), window, cx);
            }
        },
    ))
}
