//! ConnectionDialog.tsx: add / edit a PostgreSQL data source (General
//! fields, `postgresql://` URL paste, SSL mode, environment tag + colour,
//! save password, read-only, the SSH tunnel with agent or key-file auth and
//! a file picker), Test connection, Save (and connect), Delete with a
//! second-click confirmation, and [`confirm_delete`] for the explorer.
//!
//! The form logic is boa-core's (`state::conn_url`: `parse_url`, `named`,
//! `form_valid`, `clean_ssh`); this view holds the fields, mirrors them
//! into a `ConnConfig` and calls [`Connections`]. It is drawn inside
//! gpui-component's `Dialog` (needs the window's `Root`).
//!
//! Deviation: the source has no file picker for the key file; a "Browse…"
//! button next to the field opens one (`cx.prompt_for_paths`), as the plan asks.

use std::cell::Cell;

use boa_core::connections::{
    ConnConfig, Environment, SavedConnection, SshAuth, SshTunnel, SslMode,
};
use boa_core::grid::conn_tag::{self, ConnectionColor};
use boa_core::state::conn_url::{blank_config, form_valid, name_placeholder, named, parse_url};
use gpui::prelude::*;
use gpui::{
    App, Context, Entity, FocusHandle, Focusable, PathPromptOptions, Pixels, SharedString,
    Subscription, Window, div, px,
};
use gpui_component::button::{Button, ButtonVariant, ButtonVariants as _};
use gpui_component::checkbox::Checkbox;
use gpui_component::dialog::Dialog;
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_component::{Disableable as _, Icon, Sizable as _, WindowExt as _};

use super::chrome::{IconName, icon};
use super::conn_tag::{color_hsla, environment};
use crate::app_state::AppState;
use crate::models::connections::Connections;
use crate::models::prefs::Prefs;
use crate::theme::{self, ActivePalette as _, Palette};

/// The dialog's width (`sm:max-w-[520px]`).
pub const DIALOG_WIDTH: f32 = 520.;

/// Vertical space around the form: the dialog's margins and padding.
const DIALOG_CHROME_HEIGHT: f32 = 96.;

/// SSL modes in menu order (`SSL_MODES`).
pub const SSL_MODES: [(SslMode, &str); 4] = [
    (SslMode::Disable, "Disable"),
    (SslMode::Prefer, "Prefer"),
    (SslMode::Require, "Require"),
    (SslMode::VerifyFull, "Verify full"),
];

/// SSH authentications in menu order (`SSH_AUTH`).
pub const SSH_AUTH: [(SshAuth, &str); 2] = [
    (SshAuth::Agent, "SSH agent"),
    (SshAuth::KeyFile, "Key file"),
];

/// Environments in menu order (`ENVIRONMENTS`).
pub const ENVIRONMENTS: [(Environment, &str); 4] = [
    (Environment::Development, "Development"),
    (Environment::Test, "Test"),
    (Environment::Staging, "Staging"),
    (Environment::Production, "Production"),
];

/// The description under the title.
pub const DESCRIPTION: &str =
    "Settings and the password are saved in a config file on this machine.";

/// The SSH section's note.
pub const SSH_NOTE: &str = "Runs the system ssh, so ~/.ssh/config, the agent and known_hosts apply (keys with a passphrase must be in the agent). Host and port above are as the SSH server sees them.";

/// What Test connection shows (`Test`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TestState {
    Idle,
    Running,
    Ok(String),
    Err(String),
}

/// The text fields.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Field {
    Url,
    Name,
    Host,
    Port,
    User,
    Password,
    Database,
    SshHost,
    SshPort,
    SshUser,
    SshKey,
}

impl Field {
    pub const ALL: [Field; 11] = [
        Field::Url,
        Field::Name,
        Field::Host,
        Field::Port,
        Field::User,
        Field::Password,
        Field::Database,
        Field::SshHost,
        Field::SshPort,
        Field::SshUser,
        Field::SshKey,
    ];
}

/// The dialog's content view.
pub struct ConnectionDialog {
    focus_handle: FocusHandle,
    connections: Entity<Connections>,
    prefs: Option<Entity<Prefs>>,
    editing: Option<SavedConnection>,
    cfg: ConnConfig,
    password: String,
    password_touched: bool,
    save_password: bool,
    test: TestState,
    busy: bool,
    confirm_delete: bool,
    inputs: Vec<(Field, Entity<InputState>)>,
    _subscriptions: Vec<Subscription>,
}

impl std::fmt::Debug for ConnectionDialog {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ConnectionDialog")
            .field("editing", &self.editing.as_ref().map(|c| &c.config.id))
            .field("cfg", &self.cfg)
            .field("test", &self.test)
            .finish_non_exhaustive()
    }
}

fn blank_ssh() -> SshTunnel {
    SshTunnel {
        enabled: false,
        host: String::new(),
        port: None,
        user: String::new(),
        auth: SshAuth::Agent,
        identity_file: None,
    }
}

fn new_id() -> String {
    // crypto.randomUUID() in the TS: a v4-shaped id from the clock and a counter.
    use std::sync::atomic::{AtomicU64, Ordering};
    static N: AtomicU64 = AtomicU64::new(0);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    let n = N.fetch_add(1, Ordering::Relaxed);
    let mut h = nanos ^ (u128::from(std::process::id()) << 64) ^ (u128::from(n) << 96);
    // splitmix-ish scramble so ids don't look sequential.
    h = h.wrapping_mul(0x9e37_79b9_7f4a_7c15_f39c_c060_5ced_c835);
    let hex = format!("{h:032x}");
    format!(
        "{}-{}-4{}-{}{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[13..16],
        ["8", "9", "a", "b"][usize::from(hex.as_bytes()[16] % 4)],
        &hex[17..20],
        &hex[20..32]
    )
}

/// Digits only (the numeric inputs: `value.replace(/\D/g, '')`).
fn digits(s: &str) -> String {
    s.chars().filter(char::is_ascii_digit).collect()
}

/// A port as typed: 0 when empty or out of range (invalid).
fn port_of(s: &str) -> u16 {
    let d = digits(s);
    let d = d.trim_start_matches('0');
    if d.len() > 5 {
        return 0;
    }
    d.parse::<u32>()
        .ok()
        .and_then(|n| u16::try_from(n).ok())
        .unwrap_or(0)
}

impl ConnectionDialog {
    /// The form for a new data source (`edit: None`) or for editing `edit`.
    pub fn new(
        edit: Option<String>,
        connections: Entity<Connections>,
        prefs: Option<Entity<Prefs>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let editing = edit.and_then(|id| connections.read(cx).get(&id).cloned());
        let cfg = editing
            .as_ref()
            .map_or_else(|| blank_config(new_id()), |c| c.config.clone());
        let save_password = editing.as_ref().is_none_or(|c| c.has_password);
        let ssh = cfg.ssh.clone().unwrap_or_else(blank_ssh);
        let value = |field: Field| -> (String, String) {
            match field {
                Field::Url => (
                    String::new(),
                    "postgresql://user:password@host:5432/database".into(),
                ),
                Field::Name => (cfg.name.clone(), name_placeholder(&cfg)),
                Field::Host => (cfg.host.clone(), String::new()),
                Field::Port => (
                    if cfg.port == 0 {
                        String::new()
                    } else {
                        cfg.port.to_string()
                    },
                    String::new(),
                ),
                Field::User => (cfg.user.clone(), String::new()),
                Field::Password => (
                    String::new(),
                    Self::password_placeholder_for(editing.as_ref(), false).into(),
                ),
                Field::Database => (cfg.database.clone(), String::new()),
                Field::SshHost => (
                    ssh.host.clone(),
                    "bastion.example.com or a ~/.ssh/config alias".into(),
                ),
                Field::SshPort => (
                    ssh.port.map(|p| p.to_string()).unwrap_or_default(),
                    "22".into(),
                ),
                Field::SshUser => (ssh.user.clone(), "from ~/.ssh/config".into()),
                Field::SshKey => (
                    ssh.identity_file.clone().unwrap_or_default(),
                    "~/.ssh/id_ed25519".into(),
                ),
            }
        };
        let mut inputs = Vec::new();
        let mut subscriptions = Vec::new();
        for field in Field::ALL {
            let (text, placeholder) = value(field);
            let input = cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder(placeholder)
                    .default_value(text)
                    .masked(field == Field::Password)
            });
            subscriptions.push(cx.subscribe_in(
                &input,
                window,
                move |this, _, event: &InputEvent, window, cx| match event {
                    InputEvent::Change => this.on_change(field, window, cx),
                    InputEvent::PressEnter { .. } => this.submit(window, cx),
                    _ => {}
                },
            ));
            inputs.push((field, input));
        }
        subscriptions.push(cx.observe(&connections, |_, _, cx| cx.notify()));
        Self {
            focus_handle: cx.focus_handle(),
            connections,
            prefs,
            editing,
            cfg,
            password: String::new(),
            password_touched: false,
            save_password,
            test: TestState::Idle,
            busy: false,
            confirm_delete: false,
            inputs,
            _subscriptions: subscriptions,
        }
    }

    /// A field's input state.
    pub fn input(&self, field: Field) -> &Entity<InputState> {
        &self
            .inputs
            .iter()
            .find(|(f, _)| *f == field)
            .expect("every field has an input")
            .1
    }

    fn text(&self, field: Field, cx: &App) -> String {
        self.input(field).read(cx).value().to_string()
    }

    fn set_text(&self, field: Field, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        let input = self.input(field).clone();
        if input.read(cx).value().as_ref() != text {
            let text = text.to_owned();
            input.update(cx, |i, cx| i.set_value(text, window, cx));
        }
    }

    /// The config as the fields say (before `named`).
    pub fn config(&self) -> &ConnConfig {
        &self.cfg
    }

    /// Editing an existing connection.
    pub fn editing(&self) -> Option<&SavedConnection> {
        self.editing.as_ref()
    }

    pub fn test_state(&self) -> &TestState {
        &self.test
    }

    pub fn save_password(&self) -> bool {
        self.save_password
    }

    pub fn confirming_delete(&self) -> bool {
        self.confirm_delete
    }

    fn password_placeholder_for(editing: Option<&SavedConnection>, touched: bool) -> &'static str {
        if editing.is_some_and(|c| c.has_password) && !touched {
            "<stored>"
        } else {
            ""
        }
    }

    /// The Password field's placeholder: `<stored>` until a new one is typed.
    pub fn password_placeholder(&self) -> &'static str {
        Self::password_placeholder_for(self.editing.as_ref(), self.password_touched)
    }

    /// The Name field's placeholder (`database@host`).
    pub fn name_placeholder(&self) -> String {
        name_placeholder(&self.cfg)
    }

    /// Test / Save are enabled (`valid`).
    pub fn valid(&self) -> bool {
        form_valid(&self.cfg)
    }

    /// The password to send: what was typed, or None for the stored one (`typedPassword`).
    pub fn typed_password(&self) -> Option<String> {
        let stored = self.editing.as_ref().is_some_and(|c| c.has_password);
        (self.password_touched || !stored).then(|| self.password.clone())
    }

    fn ssh(&self) -> SshTunnel {
        self.cfg.ssh.clone().unwrap_or_else(blank_ssh)
    }

    fn set_ssh(&mut self, f: impl FnOnce(&mut SshTunnel)) {
        let mut ssh = self.ssh();
        f(&mut ssh);
        self.cfg.ssh = Some(ssh);
        self.test = TestState::Idle;
    }

    /// `set(patch)`: a field changed, so a test result no longer applies.
    fn touched(&mut self, cx: &mut Context<Self>) {
        self.test = TestState::Idle;
        cx.notify();
    }

    fn refresh_name_placeholder(&self, window: &mut Window, cx: &mut Context<Self>) {
        let placeholder = name_placeholder(&self.cfg);
        self.input(Field::Name)
            .clone()
            .update(cx, |i, cx| i.set_placeholder(placeholder, window, cx));
    }

    fn on_change(&mut self, field: Field, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.text(field, cx);
        match field {
            Field::Url => {
                let Some(parsed) = parse_url(&text) else {
                    return;
                };
                let password = parsed.apply(&mut self.cfg);
                for (f, value) in [
                    (Field::Host, self.cfg.host.clone()),
                    (Field::Port, self.cfg.port.to_string()),
                    (Field::User, self.cfg.user.clone()),
                    (Field::Database, self.cfg.database.clone()),
                ] {
                    self.set_text(f, &value, window, cx);
                }
                if let Some(password) = password {
                    self.set_text(Field::Password, &password, window, cx);
                    self.password = password;
                    self.password_touched = true;
                }
                self.refresh_name_placeholder(window, cx);
            }
            Field::Name => self.cfg.name = text,
            Field::Host => {
                self.cfg.host = text;
                self.refresh_name_placeholder(window, cx);
            }
            Field::Port => {
                self.cfg.port = port_of(&text);
                let shown =
                    if self.cfg.port == 0 && digits(&text).trim_start_matches('0').is_empty() {
                        String::new()
                    } else {
                        digits(&text).trim_start_matches('0').to_owned()
                    };
                if shown != text {
                    self.set_text(Field::Port, &shown, window, cx);
                }
            }
            Field::User => self.cfg.user = text,
            Field::Password => {
                self.password = text;
                if !self.password_touched {
                    self.password_touched = true;
                    self.input(Field::Password)
                        .clone()
                        .update(cx, |i, cx| i.set_placeholder("", window, cx));
                }
            }
            Field::Database => {
                self.cfg.database = text;
                self.refresh_name_placeholder(window, cx);
            }
            Field::SshHost => self.set_ssh(|s| s.host = text),
            Field::SshPort => {
                let port = port_of(&text);
                self.set_ssh(|s| s.port = (port != 0).then_some(port));
                let shown = digits(&text).trim_start_matches('0').to_owned();
                if shown != text {
                    self.set_text(Field::SshPort, &shown, window, cx);
                }
            }
            Field::SshUser => self.set_ssh(|s| s.user = text),
            Field::SshKey => self.set_ssh(|s| s.identity_file = Some(text)),
        }
        self.touched(cx);
    }

    /// Types `text` into a field (as if pasted): sets it and applies the change.
    pub fn set_field_text(
        &mut self,
        field: Field,
        text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.set_text(field, text, window, cx);
        self.on_change(field, window, cx);
    }

    /// Sets the SSL mode (the SSL select).
    pub fn set_ssl_mode(&mut self, mode: SslMode, cx: &mut Context<Self>) {
        self.cfg.ssl_mode = mode;
        self.touched(cx);
    }

    /// Sets the environment (None: "None").
    pub fn set_environment(&mut self, env: Option<Environment>, cx: &mut Context<Self>) {
        self.cfg.environment = env;
        self.touched(cx);
    }

    /// Sets the colour (None: auto, the environment's).
    pub fn set_color(&mut self, color: Option<ConnectionColor>, cx: &mut Context<Self>) {
        self.cfg.color = color.map(|c| c.name().to_owned());
        self.touched(cx);
    }

    pub fn set_read_only(&mut self, on: bool, cx: &mut Context<Self>) {
        self.cfg.read_only = on;
        self.touched(cx);
    }

    pub fn set_save_password(&mut self, on: bool, cx: &mut Context<Self>) {
        self.save_password = on;
        cx.notify();
    }

    /// The SSH tunnel checkbox.
    pub fn set_ssh_enabled(&mut self, on: bool, cx: &mut Context<Self>) {
        self.set_ssh(|s| s.enabled = on);
        cx.notify();
    }

    pub fn set_ssh_auth(&mut self, auth: SshAuth, cx: &mut Context<Self>) {
        self.set_ssh(|s| s.auth = auth);
        cx.notify();
    }

    /// The key file's "Browse…": a file picker; the chosen path goes in the field.
    pub fn browse_key_file(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Choose".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = paths.await else {
                return;
            };
            let Some(path) = paths.into_iter().next() else {
                return;
            };
            this.update_in(cx, |this, window, cx| this.set_key_file(&path, window, cx))
                .ok();
        })
        .detach();
    }

    /// Puts a key path in the field (`~/` for the home directory: ssh expands it).
    pub fn set_key_file(
        &mut self,
        path: &std::path::Path,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let mut text = path.display().to_string();
        if let Some(home) = std::env::var_os("HOME").map(std::path::PathBuf::from)
            && let Ok(rest) = path.strip_prefix(&home)
        {
            text = format!("~/{}", rest.display());
        }
        self.set_text(Field::SshKey, &text, window, cx);
        self.set_ssh(|s| s.identity_file = Some(text));
        cx.notify();
    }

    /// Test connection (`runTest`).
    pub fn run_test(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.valid() || self.test == TestState::Running {
            return;
        }
        self.test = TestState::Running;
        cx.notify();
        let config = named(&self.cfg);
        let password = self.typed_password();
        let task = self
            .connections
            .update(cx, |c, cx| c.test(config, password, cx));
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                this.test = match result {
                    Ok(version) => TestState::Ok(version),
                    Err(e) if e.code.as_deref() == Some("password_required") => {
                        TestState::Err("The server asked for a password".into())
                    }
                    Err(e) => TestState::Err(e.message),
                };
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Save / Save and connect (`submit`).
    pub fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.valid() || self.busy {
            return;
        }
        self.busy = true;
        cx.notify();
        let config = named(&self.cfg);
        let password = self.typed_password();
        let save = self.save_password;
        let created = self.editing.is_none();
        let id = config.id.clone();
        let task = self
            .connections
            .update(cx, |c, cx| c.save(config, password.clone(), save, cx));
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            this.update_in(cx, |this, window, cx| match result {
                Ok(()) => {
                    window.close_dialog(cx);
                    if created {
                        // Expand the new connection in the explorer and connect it.
                        if let Some(prefs) = this.prefs.clone() {
                            prefs.update(cx, |p, cx| p.update(cx, |p| p.set_expanded(&id, true)));
                        }
                        let password = if save { None } else { password };
                        this.connections.update(cx, |c, cx| {
                            c.connect_with(&id, password, cx).detach();
                        });
                    }
                }
                Err(e) => {
                    this.test = TestState::Err(e.message);
                    this.busy = false;
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    /// Delete: the first click asks ("Click to delete"), the second deletes.
    pub fn remove(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(id) = self.editing.as_ref().map(|c| c.config.id.clone()) else {
            return;
        };
        if !self.confirm_delete {
            self.confirm_delete = true;
            cx.notify();
            return;
        }
        let task = self.connections.update(cx, |c, cx| c.delete(&id, cx));
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            this.update_in(cx, |this, window, cx| match result {
                Ok(()) => window.close_dialog(cx),
                Err(e) => {
                    this.test = TestState::Err(e.message);
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    fn title(&self) -> String {
        match &self.editing {
            Some(c) => format!("Edit {}", c.config.name),
            None => "New PostgreSQL data source".into(),
        }
    }

    // ── Rendering ─────────────────────────────────────────────────────────

    fn field(label: &'static str, child: impl IntoElement, cx: &App) -> gpui::Div {
        let p = cx.palette();
        div()
            .flex()
            .flex_col()
            .gap(px(6.))
            .min_w_0()
            .child(
                div()
                    .text_size(theme::text::XS)
                    .line_height(theme::text::XS_LINE)
                    .text_color(p.quiet)
                    .child(label),
            )
            .child(child)
    }

    /// input.tsx: `h-9 px-3 text-sm`, `border-input`, `dark:bg-input/30`.
    fn text_input(&self, field: Field, mono: bool, p: &Palette) -> Input {
        // `large`: h-11 (2.75 rem, 35.75 px here), 12 px padding, 13 px text.
        let input = Input::new(self.input(field))
            .w_full()
            .large()
            .bg(p.input.opacity(0.3));
        if mono {
            input
                .font_family(theme::MONO_FONT)
                .text_size(theme::text::XS)
        } else {
            input
        }
    }

    fn select<T: Copy + PartialEq + 'static>(
        id: &'static str,
        current: T,
        choices: Vec<(T, &'static str)>,
        on_pick: impl Fn(&mut Self, T, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let label = choices
            .iter()
            .find(|(v, _)| *v == current)
            .map_or("", |(_, l)| *l);
        let this = cx.entity().downgrade();
        let on_pick = std::rc::Rc::new(on_pick);
        let p = cx.palette();
        // select.tsx's SelectTrigger: `h-9 px-3 py-2 text-sm`, `border-input`,
        // `dark:bg-input/30`, a 16 px chevron in text-muted-foreground at 50 %.
        Button::new(id)
            .outline()
            .w_full()
            .h(FIELD_HEIGHT)
            .px(px(12.))
            .rounded(theme::radius::MD)
            .border_color(p.input)
            .bg(p.input.opacity(0.3))
            .text_color(p.body)
            .child(
                div()
                    .flex()
                    .flex_1()
                    .items_center()
                    .justify_between()
                    .text_size(theme::text::SM)
                    .font_weight(gpui::FontWeight::NORMAL)
                    .child(label)
                    .child(icon(IconName::ChevronDown, 16., p.quiet.opacity(0.5))),
            )
            .dropdown_menu(move |mut menu, _, _| {
                for &(value, text) in &choices {
                    let this = this.clone();
                    let on_pick = on_pick.clone();
                    menu = menu.item(PopupMenuItem::new(text).checked(value == current).on_click(
                        move |_, _, cx| {
                            this.update(cx, |this, cx| on_pick(this, value, cx)).ok();
                        },
                    ));
                }
                menu
            })
            .into_any_element()
    }

    fn render_colors(&self, cx: &mut Context<Self>) -> gpui::Div {
        let p = cx.palette().clone();
        let picked = self
            .cfg
            .color
            .as_deref()
            .and_then(ConnectionColor::from_name);
        let fallback = environment(&self.cfg).map(conn_tag::Environment::color);
        // `size-5` swatches `gap-1.5` apart; the picked one has `ring-2
        // ring-foreground/60` around a 2 px `ring-offset-background` gap,
        // drawn outside the swatch without moving the others.
        let ring = |selected: bool| {
            div()
                .flex_none()
                .relative()
                .size(px(20.))
                .when(selected, |d| {
                    d.child(
                        div()
                            .absolute()
                            .top(px(-4.))
                            .left(px(-4.))
                            .size(px(28.))
                            .rounded_full()
                            .border_2()
                            .border_color(p.body.opacity(0.6))
                            .bg(p.background),
                    )
                })
        };
        let auto = ring(picked.is_none()).child(
            div()
                .id("color-auto")
                .absolute()
                .inset_0()
                .rounded_full()
                .flex()
                .items_center()
                .justify_center()
                .cursor_pointer()
                .border_1()
                .border_dashed()
                .border_color(fallback.map_or(p.dim, color_hsla))
                .text_size(px(9.))
                .text_color(p.dim)
                .tooltip(super::chrome::tooltip(match fallback {
                    Some(c) => format!("Auto ({})", c.name()),
                    None => "None".into(),
                }))
                .on_click(cx.listener(|this, _, _, cx| this.set_color(None, cx)))
                .child("A"),
        );
        let swatches = ConnectionColor::ALL.into_iter().map(|color| {
            ring(picked == Some(color)).child(
                div()
                    .id(SharedString::from(format!("color-{}", color.name())))
                    .absolute()
                    .inset_0()
                    .rounded_full()
                    .cursor_pointer()
                    .bg(color_hsla(color))
                    .tooltip(super::chrome::tooltip(color.name()))
                    .on_click(cx.listener(move |this, _, _, cx| this.set_color(Some(color), cx))),
            )
        });
        Self::field(
            "Colour",
            div()
                .flex()
                .items_center()
                .gap(px(6.))
                .h(FIELD_HEIGHT)
                .child(auto)
                .children(swatches),
            cx,
        )
    }

    fn render_test_result(&self, cx: &App) -> Option<gpui::Div> {
        let p = cx.palette();
        match &self.test {
            TestState::Ok(version) => Some(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .text_size(theme::text::SM)
                    .text_color(p.status_ok)
                    .child(icon(IconName::CircleCheck, 16., p.status_ok))
                    .child(format!("Connected · {version}")),
            ),
            TestState::Err(message) => Some(
                div()
                    .flex()
                    .items_start()
                    .gap(px(8.))
                    .text_size(theme::text::SM)
                    .text_color(p.status_err_text)
                    .child(div().mt(px(2.)).child(icon(
                        IconName::CircleAlert,
                        16.,
                        p.status_err_text,
                    )))
                    .child(div().min_w_0().flex_1().child(message.clone())),
            ),
            _ => None,
        }
    }
}

impl Focusable for ConnectionDialog {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for ConnectionDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = cx.palette().clone();
        let valid = self.valid();
        // `max-h-[calc(100vh-2rem)] overflow-y-auto`: the dialog keeps 16 px
        // margins plus its own padding; the form scrolls inside.
        let max_h = (window.viewport_size().height - px(DIALOG_CHROME_HEIGHT)).max(px(200.));
        let ssh = self.ssh();
        let row2 = |a: gpui::Div, b: gpui::Div| {
            div()
                .flex()
                .gap(px(12.))
                .child(a.flex_1().flex_basis(px(0.)))
                .child(b.flex_1().flex_basis(px(0.)))
        };
        let row_port = |a: gpui::Div, b: gpui::Div| {
            div()
                .flex()
                .gap(px(12.))
                .child(a.flex_1())
                .child(b.w(px(96.)).flex_none())
        };

        // DialogHeader (flex-col gap-2): the title (text-base semibold) with
        // dialog.tsx's close button (a 16 px X at 70 %, top-4 right-4), then
        // the description (text-sm text-muted-foreground).
        let close = div()
            .id("connection-dialog-close")
            .flex_none()
            .mr(px(-2.))
            .rounded(px(2.))
            .cursor_pointer()
            .opacity(0.7)
            .hover(|s| s.opacity(1.))
            .tooltip(super::chrome::tooltip("Close"))
            .on_click(|_, window, cx| window.close_dialog(cx))
            .child(icon(IconName::X, 16., p.body));
        let header = div()
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
                            .text_color(p.body)
                            .child(self.title()),
                    )
                    .child(close),
            )
            .child(
                div()
                    .text_size(theme::text::SM)
                    .line_height(theme::text::SM_LINE)
                    .text_color(p.quiet)
                    .child(DESCRIPTION),
            );

        let url = self
            .editing
            .is_none()
            .then(|| Self::field("URL", self.text_input(Field::Url, true, &p), cx));

        let ssl = Self::select(
            "ssl-mode",
            self.cfg.ssl_mode,
            SSL_MODES.to_vec(),
            |this, mode, cx| this.set_ssl_mode(mode, cx),
            cx,
        );
        let mut env_choices: Vec<(Option<Environment>, &'static str)> = vec![(None, "None")];
        env_choices.extend(ENVIRONMENTS.iter().map(|&(e, l)| (Some(e), l)));
        let env = Self::select(
            "environment",
            self.cfg.environment,
            env_choices,
            |this, env, cx| this.set_environment(env, cx),
            cx,
        );

        let checks = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap_x(px(20.))
            .gap_y(px(8.))
            .child(super::checkbox::sized(
                check_box("save-password", "Save password", &p)
                    .checked(self.save_password)
                    .on_click(cx.listener(|this, on: &bool, _, cx| this.set_save_password(*on, cx))),
            ))
            .child(
                div()
                    .id("read-only-wrap")
                    .tooltip(super::chrome::tooltip(
                        "Sessions run with default_transaction_read_only = on: the server refuses writes",
                    ))
                    .child(super::checkbox::sized(
                        check_box("read-only", "Read-only", &p)
                            .checked(self.cfg.read_only)
                            .on_click(cx.listener(|this, on: &bool, _, cx| this.set_read_only(*on, cx))),
                    )),
            );

        let production = conn_tag::is_production(environment(&self.cfg)).then(|| {
            div()
                .mt(px(-4.))
                .text_size(theme::text::XS)
                .text_color(p.quiet)
                .child("Destructive statements on production always ask for confirmation.")
        });

        let ssh_section = div()
            .flex()
            .flex_col()
            .gap(px(12.))
            .border_t_1()
            .border_color(p.border)
            .pt(px(12.))
            .child(
                div()
                    .id("ssh-wrap")
                    .w_full()
                    .tooltip(super::chrome::tooltip(
                        "Connect through an SSH server, with the system ssh",
                    ))
                    .child(super::checkbox::sized(
                        check_box("ssh-enabled", "SSH tunnel", &p)
                            .checked(ssh.enabled)
                            .on_click(
                                cx.listener(|this, on: &bool, _, cx| this.set_ssh_enabled(*on, cx)),
                            ),
                    )),
            )
            .when(ssh.enabled, |el| {
                let auth = Self::select(
                    "ssh-auth",
                    ssh.auth,
                    SSH_AUTH.to_vec(),
                    |this, auth, cx| this.set_ssh_auth(auth, cx),
                    cx,
                );
                el.child(row_port(
                    Self::field("SSH host", self.text_input(Field::SshHost, false, &p), cx),
                    Self::field("SSH port", self.text_input(Field::SshPort, false, &p), cx),
                ))
                .child(row2(
                    Self::field("SSH user", self.text_input(Field::SshUser, false, &p), cx),
                    Self::field("Authentication", auth, cx),
                ))
                .when(ssh.auth == SshAuth::KeyFile, |el| {
                    el.child(Self::field(
                        "Private key file",
                        div()
                            .flex()
                            .gap(px(8.))
                            .child(div().flex_1().min_w_0().child(self.text_input(
                                Field::SshKey,
                                true,
                                &p,
                            )))
                            .child(
                                Button::new("browse-key")
                                    .outline()
                                    .label("Browse…")
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.browse_key_file(window, cx)
                                    })),
                            ),
                        cx,
                    ))
                })
                .child(
                    div()
                        .mt(px(-4.))
                        .text_size(theme::text::XS)
                        .text_color(p.quiet)
                        .child(SSH_NOTE),
                )
            });

        let footer = div()
            .flex()
            .items_center()
            .justify_between()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(
                        md_button(Button::new("test-connection").outline(), "Test connection")
                            .border_color(p.border)
                            .bg(gpui::transparent_black())
                            .loading(self.test == TestState::Running)
                            .disabled(!valid || self.test == TestState::Running)
                            .on_click(cx.listener(|this, _, window, cx| this.run_test(window, cx))),
                    )
                    .when(self.editing.is_some(), |el| {
                        let confirm = self.confirm_delete;
                        let button = md_button(
                            Button::new("delete-connection"),
                            if confirm { "Click to delete" } else { "Delete" },
                        )
                        .icon(Icon::new(IconName::Trash))
                        .on_click(cx.listener(|this, _, window, cx| this.remove(window, cx)));
                        el.child(if confirm {
                            button.with_variant(ButtonVariant::Danger)
                        } else {
                            button.ghost().text_color(p.quiet)
                        })
                    }),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(
                        md_button(Button::new("cancel").ghost(), "Cancel")
                            .text_color(p.body)
                            .on_click(|_, window, cx| window.close_dialog(cx)),
                    )
                    .child(
                        md_button(
                            Button::new("save").primary(),
                            if self.editing.is_some() {
                                "Save"
                            } else {
                                "Save and connect"
                            },
                        )
                        .disabled(!valid || self.busy)
                        .on_click(cx.listener(|this, _, window, cx| this.submit(window, cx))),
                    ),
            );

        div()
            .id("connection-dialog")
            .track_focus(&self.focus_handle)
            .max_h(max_h)
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .gap(px(12.))
            .text_color(p.body)
            .child(header)
            .children(url)
            .child(Self::field(
                "Name",
                self.text_input(Field::Name, false, &p),
                cx,
            ))
            .child(row_port(
                Self::field("Host", self.text_input(Field::Host, false, &p), cx),
                Self::field("Port", self.text_input(Field::Port, false, &p), cx),
            ))
            .child(row2(
                Self::field("User", self.text_input(Field::User, false, &p), cx),
                Self::field("Password", self.text_input(Field::Password, false, &p), cx),
            ))
            .child(row2(
                Self::field("Database", self.text_input(Field::Database, false, &p), cx),
                Self::field("SSL", ssl, cx),
            ))
            .child(row2(
                Self::field("Environment", env, cx),
                self.render_colors(cx),
            ))
            .child(checks)
            .children(production)
            .child(ssh_section)
            .children(self.render_test_result(cx))
            .child(footer)
    }
}

/// Inputs and select triggers: `h-9`.
const FIELD_HEIGHT: Pixels = px(36.);

/// dialog.tsx's surface (`DIALOG_SURFACE_CLASSES`): centred in the window,
/// `bg-popover`, `rounded-xl`, `px-[18px] py-4` (gpui-component's sits a
/// tenth of the window down, on the window background, 16 px all round).
pub fn dialog_surface(dialog: Dialog, cx: &App) -> Dialog {
    let p = cx.palette();
    dialog
        .centered()
        .bg(p.popover)
        .rounded(theme::radius::XL)
        .px(px(18.))
        .py(px(16.))
}

/// button.tsx `size="md"`: 30 px, `rounded-[5px] px-3.5`, text-sm
/// font-medium (the dialog footers).
pub fn md_button(button: Button, label: &'static str) -> Button {
    button
        .with_size(px(30.))
        .h(px(30.))
        .px(px(14.))
        .rounded(px(5.))
        .child(
            div()
                .text_size(theme::text::SM)
                .line_height(theme::text::SM_LINE)
                .font_weight(gpui::FontWeight::MEDIUM)
                .child(label),
        )
}

/// A labelled checkbox as the dialog's `Label` + `Checkbox`
/// ([`super::checkbox::checkbox`]: a 16 px box, `gap-2`, text-sm
/// `text-soft`); wrap it with [`super::checkbox::sized`].
fn check_box(id: &'static str, label: &'static str, p: &Palette) -> Checkbox {
    super::checkbox::checkbox(id, false, label, p)
}

/// Opens the connection dialog: `None` adds a data source (⌘O), `Some(id)`
/// edits that one. Called by the explorer (p3/explorer) and the ⌘O action.
pub fn open(edit: Option<String>, window: &mut Window, cx: &mut App) {
    open_view(edit, window, cx);
}

/// [`open`], returning the dialog's view (tests, dev hooks).
pub fn open_view(
    edit: Option<String>,
    window: &mut Window,
    cx: &mut App,
) -> Option<Entity<ConnectionDialog>> {
    let state = AppState::try_global(cx)?.clone();
    let view = cx.new(|cx| {
        ConnectionDialog::new(
            edit,
            state.connections.clone(),
            Some(state.prefs.clone()),
            window,
            cx,
        )
    });
    let name = view.read(cx).input(Field::Name).clone();
    let first = Cell::new(true);
    let content = view.clone();
    window.open_dialog(cx, move |dialog, window, cx| {
        if first.replace(false) {
            let handle = name.read(cx).focus_handle(cx);
            window.focus(&handle, cx);
        }
        dialog_surface(dialog, cx)
            .w(px(DIALOG_WIDTH))
            .close_button(false)
            .child(content.clone())
    });
    Some(view)
}

/// Asks to delete a saved connection (with confirmation), then deletes it
/// (its password and query history too).
pub fn confirm_delete(id: String, window: &mut Window, cx: &mut App) {
    let Some(state) = AppState::try_global(cx).cloned() else {
        return;
    };
    let Some(name) = state
        .connections
        .read(cx)
        .get(&id)
        .map(|c| c.config.name.clone())
    else {
        return;
    };
    window.open_alert_dialog(cx, move |alert, _, _| {
        let connections = state.connections.clone();
        let toasts = state.toasts.clone();
        let id = id.clone();
        alert
            .title(format!("Delete {name}?"))
            .description("Its settings, saved password and query history are removed.")
            .show_cancel(true)
            .ok_text("Delete")
            .ok_variant(ButtonVariant::Danger)
            .on_ok(move |_, _, cx| {
                let task = connections.update(cx, |c, cx| c.delete(&id, cx));
                let toasts = toasts.clone();
                cx.spawn(async move |cx| {
                    if let Err(e) = task.await {
                        toasts.update(cx, |t, cx| {
                            t.report_error("Could not delete the connection", e.message, cx);
                        });
                    }
                })
                .detach();
                true
            })
    });
}

/// Dev hook (feature `dev-hooks`): `BOA_DEV_DIALOG=new|url|ssh|edit|prompt`
/// opens the dialog (or the password prompt) in a scripted state for
/// screenshots. `edit` / `prompt` use the first saved connection (with
/// `BOA_DEV_CONNECTION=1`: "Local dev").
#[cfg(feature = "dev-hooks")]
pub mod dev {
    use gpui::{App, Window};

    use super::{Field, open_view};
    use crate::app_state::AppState;

    pub const ENV: &str = "BOA_DEV_DIALOG";

    pub fn apply(window: &mut Window, cx: &mut App) {
        let Some(what) = std::env::var(ENV).ok().filter(|v| !v.is_empty()) else {
            return;
        };
        let Some(state) = AppState::try_global(cx).cloned() else {
            return;
        };
        match what.as_str() {
            "edit" | "prompt" => {
                // Once the saved list is in.
                let load = state.connections.update(cx, |c, cx| c.load(cx));
                let handle = window.window_handle();
                cx.spawn(async move |cx| {
                    load.await;
                    handle
                        .update(cx, |_, window, cx| {
                            let Some(id) = state
                                .connections
                                .read(cx)
                                .saved
                                .last()
                                .map(|c| c.config.id.clone())
                            else {
                                return;
                            };
                            if what == "edit" {
                                open_view(Some(id), window, cx);
                            } else {
                                state.connections.update(cx, |c, cx| {
                                    c.ask_password(
                                        &id,
                                        Some(format!(
                                            "password authentication failed for user \"{}\"",
                                            "boa"
                                        )),
                                        cx,
                                    )
                                });
                            }
                        })
                        .ok();
                })
                .detach();
            }
            other => {
                let Some(view) = open_view(None, window, cx) else {
                    return;
                };
                view.update(cx, |v, cx| match other {
                    "url" => v.set_field_text(
                        Field::Url,
                        "postgresql://analyst:s3cret@db.internal.example.com:6432/warehouse?sslmode=require",
                        window,
                        cx,
                    ),
                    "ssh" => {
                        v.set_field_text(Field::Name, "Production (via bastion)", window, cx);
                        v.set_field_text(Field::Host, "10.0.12.7", window, cx);
                        v.set_ssh_enabled(true, cx);
                        v.set_ssh_auth(boa_core::connections::SshAuth::KeyFile, cx);
                        v.set_field_text(Field::SshHost, "bastion.example.com", window, cx);
                        v.set_field_text(Field::SshUser, "deploy", window, cx);
                        v.set_field_text(Field::SshKey, "~/.ssh/id_ed25519", window, cx);
                        v.set_environment(Some(boa_core::connections::Environment::Production), cx);
                    }
                    _ => {}
                });
            }
        }
    }
}

#[cfg(test)]
mod tests;
