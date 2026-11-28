//! ConnectionTag.tsx (environment badge + read-only lock, the editor's
//! colour stripe) and StatusDot.tsx's `connDot` / `DOT_LABEL`, as plain
//! render functions for the explorer, the title bar, the console toolbar and
//! the status bar. Colours come from `boa_core::grid::conn_tag` (connTag.ts).

use boa_core::connections::{ConnConfig, Environment};
use boa_core::grid::conn_tag::{self, ConnectionColor};
use gpui::prelude::*;
use gpui::{App, Div, Hsla, SharedString, div, px, rgb};

use super::chrome::{self, DotKind, IconName, icon};
use crate::models::connections::ConnState;
use crate::theme::{self, ActivePalette as _};

/// StatusDot.tsx `connDot`: connected → ok, connecting / reconnecting →
/// wait, failed → err, else disconnected (a ring).
pub fn conn_dot(state: &ConnState) -> DotKind {
    match state {
        ConnState::Connected { .. } => DotKind::Ok,
        ConnState::Connecting | ConnState::Reconnecting { .. } => DotKind::Wait,
        ConnState::Failed { .. } => DotKind::Err,
        ConnState::Disconnected => DotKind::Disc,
    }
}

/// StatusDot.tsx `DOT_LABEL` (the dot's tooltip).
pub fn dot_label(kind: DotKind) -> &'static str {
    match kind {
        DotKind::Run => "Busy",
        DotKind::Wait => "Connecting",
        DotKind::Err => "Failed",
        DotKind::Ok => "Connected",
        DotKind::Idle => "Idle",
        DotKind::Disc => "Disconnected",
    }
}

/// A connection's status dot (`size`: 7 px rows / tabs, 6 px headers),
/// with its label as tooltip. `id` must be unique in the window.
pub fn state_dot(id: SharedString, state: &ConnState, size: f32, cx: &App) -> gpui::Stateful<Div> {
    let kind = conn_dot(state);
    div()
        .id(gpui::ElementId::Name(id))
        .flex_none()
        .tooltip(chrome::tooltip(dot_label(kind)))
        .child(chrome::status_dot(kind, size, cx))
}

/// The tag environment of a saved connection.
pub fn environment(config: &ConnConfig) -> Option<conn_tag::Environment> {
    config.environment.map(|e| match e {
        Environment::Development => conn_tag::Environment::Development,
        Environment::Test => conn_tag::Environment::Test,
        Environment::Staging => conn_tag::Environment::Staging,
        Environment::Production => conn_tag::Environment::Production,
    })
}

/// A palette colour as a GPUI colour.
pub fn color_hsla(color: ConnectionColor) -> Hsla {
    rgb(color.rgb()).into()
}

/// The connection's colour (picked, else its environment's), if any
/// (connTag.ts `connectionColor`).
pub fn connection_color(config: &ConnConfig) -> Option<Hsla> {
    conn_tag::connection_color(environment(config), config.color.as_deref()).map(color_hsla)
}

/// ConnectionTag.tsx: the environment badge (`PROD`, white on the
/// connection colour) and a lock when read-only; None when it has neither.
pub fn connection_tag(config: &ConnConfig, cx: &App) -> Option<Div> {
    let env = environment(config);
    if env.is_none() && !config.read_only {
        return None;
    }
    let p = cx.palette();
    let badge = env.map(|env| {
        let bg = connection_color(config).unwrap_or_else(|| color_hsla(env.color()));
        div()
            .flex_none()
            .rounded(px(3.))
            .px(px(4.))
            .py(px(1.))
            .bg(bg)
            .font_family(theme::MONO_FONT)
            .text_size(px(9.5))
            .line_height(px(10.))
            // The source's `font-semibold`; GPUI's text renders heavier
            // than Chrome's, so one step lighter reads the same.
            .font_weight(gpui::FontWeight::MEDIUM)
            .text_color(gpui::white())
            .child(env.badge())
    });
    let lock = config.read_only.then(|| icon(IconName::Lock, 12., p.dim));
    Some(
        div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(4.))
            .children(badge)
            .children(lock),
    )
}

/// ConnectionTag.tsx `ConnectionStripe`: a 2 px bar in the connection colour
/// across the top of the editor; None without a colour.
pub fn connection_stripe(config: &ConnConfig) -> Option<Div> {
    let color = connection_color(config)?;
    Some(div().flex_none().h(px(2.)).w_full().bg(color))
}

#[cfg(test)]
mod tests {
    use super::*;
    use boa_core::connections::SslMode;

    fn cfg() -> ConnConfig {
        ConnConfig {
            kind: Default::default(),
            id: "c".into(),
            name: "c".into(),
            host: "h".into(),
            port: 5432,
            user: "u".into(),
            database: "d".into(),
            ssl_mode: SslMode::Prefer,
            read_only: false,
            environment: None,
            color: None,
            ssh: None,
        }
    }

    #[test]
    fn dots_and_colours_follow_the_state_and_tag() {
        assert_eq!(conn_dot(&ConnState::Disconnected), DotKind::Disc);
        assert_eq!(conn_dot(&ConnState::Connecting), DotKind::Wait);
        assert_eq!(
            conn_dot(&ConnState::Reconnecting { attempt: 2 }),
            DotKind::Wait
        );
        assert_eq!(
            conn_dot(&ConnState::Failed {
                message: String::new()
            }),
            DotKind::Err
        );
        assert_eq!(
            conn_dot(&ConnState::Connected {
                server_version: String::new(),
                databases: Vec::new()
            }),
            DotKind::Ok
        );
        assert_eq!(dot_label(DotKind::Disc), "Disconnected");
        let mut c = cfg();
        assert_eq!(connection_color(&c), None);
        assert!(connection_stripe(&c).is_none());
        c.environment = Some(Environment::Production);
        assert_eq!(connection_color(&c), Some(color_hsla(ConnectionColor::Red)));
        c.color = Some("purple".into());
        assert_eq!(
            connection_color(&c),
            Some(color_hsla(ConnectionColor::Purple))
        );
        c.color = Some("chartreuse".into());
        assert_eq!(connection_color(&c), Some(color_hsla(ConnectionColor::Red)));
    }
}
