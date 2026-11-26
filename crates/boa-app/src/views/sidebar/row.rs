//! How one explorer row looks (Sidebar.tsx `TreeRow`, `ConnectionIcon`,
//! `MetaIcon`, ConnectionTag.tsx): icons per kind, the label, the dim meta.

use std::time::Duration;

use boa_core::grid::table_meta::{KeyKind, MetaFolderKind};
use boa_core::model::TableKind;
use gpui::prelude::*;
use gpui::{Animation, AnimationExt as _, AnyElement, App, Div, Hsla, Rgba, div, px};

use super::tree::{ConnInfo, Meta, Node, NodeKind};
use crate::models::connections::ConnState;
use crate::theme::{self, ActivePalette as _};
use crate::views::chrome::{IconName, icon};

/// The `size-3.5` icons of the tree.
pub const ICON: f32 = 14.;

/// A connection colour (`0xRRGGBB`) as a GPUI colour.
pub fn rgb_color(rgb: u32) -> Hsla {
    Rgba {
        r: ((rgb >> 16) & 0xff) as f32 / 255.,
        g: ((rgb >> 8) & 0xff) as f32 / 255.,
        b: (rgb & 0xff) as f32 / 255.,
        a: 1.,
    }
    .into()
}

/// The tooltip text of a connection's icon (`ConnectionIcon` `label`).
pub fn conn_state_label(state: &ConnState) -> &'static str {
    match state {
        ConnState::Connected { .. } => "Connected",
        ConnState::Connecting => "Connecting…",
        ConnState::Reconnecting { .. } => "Connection lost: reconnecting…",
        ConnState::Failed { .. } => "Connection failed: expand to retry",
        ConnState::Disconnected => "Not connected",
    }
}

/// Server icon in the connection's colour, with its state as a badge on the
/// corner: green connected, pulsing amber connecting, red failed, none (and a
/// dimmed icon) when offline. `cutout` is the surface colour behind it.
pub fn connection_icon(conn: &ConnInfo, cutout: Hsla, cx: &App) -> AnyElement {
    let p = cx.palette();
    let color = conn.color.map_or(p.quiet, |c| rgb_color(c.rgb()));
    let offline = matches!(conn.state, ConnState::Disconnected);
    let server = icon(IconName::Server, ICON, color).when(offline, |i| i.opacity(0.55));
    let badge = |bg: Hsla| {
        div()
            .absolute()
            .right(px(-3.))
            .bottom(px(-3.))
            .size(px(7.))
            .rounded_full()
            .border(px(1.5))
            .border_color(cutout)
            .bg(bg)
    };
    let wrap = div()
        .relative()
        .flex_none()
        .size(px(ICON))
        .flex()
        .items_center()
        .justify_center()
        .child(server);
    match &conn.state {
        ConnState::Disconnected => wrap.into_any_element(),
        ConnState::Connected { .. } => wrap.child(badge(p.status_ok)).into_any_element(),
        ConnState::Failed { .. } => wrap.child(badge(p.destructive)).into_any_element(),
        ConnState::Connecting | ConnState::Reconnecting { .. } => wrap
            .child(
                badge(p.status_wait).with_animation(
                    gpui::ElementId::Name(format!("conn-pulse-{}", conn.id).into()),
                    Animation::new(Duration::from_millis(2000))
                        .repeat()
                        .with_easing(gpui::pulsating_between(0.5, 1.)),
                    |el, delta| el.opacity(delta),
                ),
            )
            .into_any_element(),
    }
}

/// ConnectionTag.tsx: the environment badge (`PROD`) in the connection
/// colour, plus a lock when read-only. None when it has neither.
pub fn connection_tag(conn: &ConnInfo, cx: &App) -> Option<Div> {
    if conn.environment.is_none() && !conn.read_only {
        return None;
    }
    let p = cx.palette();
    let mut tag = div().flex().flex_none().items_center().gap(px(4.));
    if let Some(env) = conn.environment {
        let bg = conn.color.map_or(p.dim, |c| rgb_color(c.rgb()));
        tag = tag.child(
            div()
                .flex_none()
                .rounded(px(3.))
                .px(px(4.))
                .py(px(1.))
                .bg(bg)
                .font_family(theme::MONO_FONT)
                .text_size(px(9.5))
                .line_height(px(10.))
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .text_color(gpui::white())
                .child(env.badge()),
        );
    }
    if conn.read_only {
        tag = tag.child(icon(IconName::Lock, 12., p.dim));
    }
    Some(tag)
}

/// The icon left of a row's label.
pub fn node_icon(node: &Node, cutout: Hsla, cx: &App) -> Option<AnyElement> {
    let p = cx.palette();
    let key_color = |key: Option<KeyKind>| match key {
        Some(KeyKind::Primary) => p.status_wait,
        Some(KeyKind::Foreign) => p.link,
        _ => p.dim,
    };
    let el = match &node.kind {
        NodeKind::Connection(conn) => return Some(connection_icon(conn, cutout, cx)),
        NodeKind::Status | NodeKind::NoDatabases => return None,
        NodeKind::Database { active, .. } => icon(
            IconName::Cylinder,
            ICON,
            if *active { p.link } else { p.quiet },
        ),
        NodeKind::Schema { .. } => icon(IconName::Boxes, ICON, p.quiet),
        NodeKind::Group { .. } | NodeKind::Folder(_) => icon(IconName::Folder, ICON, p.quiet),
        NodeKind::Table { kind, .. } => match kind {
            TableKind::View => icon(IconName::Eye, ICON, p.dim),
            TableKind::Table => icon(IconName::Table2, ICON, p.dim),
        },
        NodeKind::Column { key, .. } => match key {
            Some(k) => icon(IconName::KeyRound, ICON, key_color(Some(*k))),
            None => icon(IconName::Columns3, ICON, p.dim),
        },
        NodeKind::MetaItem { folder, key, .. } => match folder {
            MetaFolderKind::Checks => icon(IconName::SquareCheck, ICON, p.dim),
            MetaFolderKind::Indexes => icon(
                IconName::ListOrdered,
                ICON,
                if *key == Some(KeyKind::Primary) {
                    p.status_wait
                } else {
                    p.dim
                },
            ),
            MetaFolderKind::Keys | MetaFolderKind::ForeignKeys => {
                icon(IconName::KeyRound, ICON, key_color(*key))
            }
        },
    };
    Some(el.into_any_element())
}

/// The row's label (connections: the name and their tag).
pub fn node_label(node: &Node, cx: &App) -> Div {
    let p = cx.palette();
    let label = div().flex_none().whitespace_nowrap();
    match &node.kind {
        NodeKind::Connection(conn) => div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(8.))
            .child(label.child(node.label.clone()))
            .children(connection_tag(conn, cx)),
        NodeKind::Status | NodeKind::NoDatabases => {
            label.text_color(p.dim).child(node.label.clone())
        }
        _ => label
            .when(node.mono, |d| d.font_family(theme::MONO_FONT))
            .when(node.italic, |d| d.italic())
            .child(node.label.clone()),
    }
}

/// Plain dim meta text (`text-xs text-dim`).
pub fn meta_text(text: impl Into<gpui::SharedString>, cx: &App) -> Div {
    div()
        .flex_none()
        .whitespace_nowrap()
        .text_size(theme::text::XS)
        .text_color(cx.palette().dim)
        .child(text.into())
}

/// The meta after the label, except scope badges (the view draws those, as
/// they are buttons).
pub fn node_meta(meta: &Meta, cx: &App) -> Option<Div> {
    let p = cx.palette();
    Some(match meta {
        Meta::None | Meta::Scope(_) | Meta::Database { .. } => return None,
        Meta::Text(t) => meta_text(t.clone(), cx),
        Meta::Failed => meta_text("failed", cx).text_color(p.status_err_text),
        Meta::Reconnecting => meta_text("reconnecting…", cx).text_color(p.status_wait),
        Meta::Column { ty, not_null } => meta_text("", cx)
            .font_family(theme::MONO_FONT)
            .flex()
            .child(div().text_color(p.sql_type).child(ty.clone()))
            .when(*not_null, |d| d.child("\u{a0}not null")),
        Meta::Detail(d) => meta_text(d.clone(), cx).font_family(theme::MONO_FONT),
    })
}
