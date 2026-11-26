//! Boa's dark theme: the colours of the Tauri app's `ui/src/assets/main.css`
//! (the `.dark` block), its type scale, and its fonts.
//!
//! [`Palette`] is a global; read it with `cx.palette()` ([`ActivePalette`]).
//! [`init`] also maps the palette onto gpui-component's `Theme` so its
//! widgets (inputs, tables, menus, dialogs, scrollbars...) match.
//!
//! **The rem is 13 px**, not Tailwind's 16: gpui-component's `Root` sets the
//! window's rem to `Theme::font_size` ([`text::SM`]). So GPUI's rem-based
//! helpers come out at 13/16 of the Tailwind class of the same name
//! (`gap_3()` is 9.75 px, `px_2()` 6.5 px, `text_sm()` 11.4 px, `h_8()` 26
//! px), and so do gpui-component's sizes (a medium `Input` or `Button` is
//! 26 px tall with 11.4 px text, an `xsmall` icon 9.75 px). When porting a
//! Tailwind class, write its value in px (`gap-3` → `.gap(px(12.))`) and
//! size text with [`text`].

use gpui::{App, Global, Hsla, SharedString, rgb, rgba};
use gpui_component::{Theme, ThemeMode};

/// UI text. The Tauri app's `--font-sans` starts with `-apple-system`, so its
/// chrome renders in the system font (SF Pro); Geist is only a fallback there.
pub const UI_FONT: &str = ".SystemUIFont";
/// Geist (bundled): the wordmark and anything that asks for Geist explicitly.
pub const GEIST: &str = "Geist";
/// Code, values and the SQL editor: Geist Mono (bundled).
pub const MONO_FONT: &str = "Geist Mono";

/// The type scale (`@theme` in main.css): size and line height in px.
pub mod text {
    use gpui::{Pixels, px};

    /// 11 / 16: meta (keycaps, counts, status bar).
    pub const XXS: Pixels = px(11.);
    pub const XXS_LINE: Pixels = px(16.);
    /// 12 / 18: secondary (sub-labels, descriptions, small buttons).
    pub const XS: Pixels = px(12.);
    pub const XS_LINE: Pixels = px(18.);
    /// 13 / 20: body (rows, tabs, menus, inputs, buttons, the editor).
    pub const SM: Pixels = px(13.);
    pub const SM_LINE: Pixels = px(20.);
    /// 14 / 20: group headers, dialog titles, the switcher input.
    pub const BASE: Pixels = px(14.);
    pub const BASE_LINE: Pixels = px(20.);
    /// 16 / 24: page headings.
    pub const LG: Pixels = px(16.);
    pub const LG_LINE: Pixels = px(24.);
    /// 20 / 28: settings page title.
    pub const XL: Pixels = px(20.);
    pub const XL_LINE: Pixels = px(28.);
}

/// Corner radii (`--radius: 0.5rem` and its steps).
pub mod radius {
    use gpui::{Pixels, px};

    pub const SM: Pixels = px(5.);
    pub const MD: Pixels = px(6.);
    pub const LG: Pixels = px(8.);
    pub const XL: Pixels = px(11.);
}

/// Every colour token of the dark theme, by its main.css name.
#[derive(Debug, Clone, PartialEq)]
pub struct Palette {
    // Surfaces
    /// `--background` #151616: the work area and the window.
    pub background: Hsla,
    /// `--chrome` #181819: toolbars and the area around panels.
    pub chrome: Hsla,
    /// `--sidebar` #181819.
    pub sidebar: Hsla,
    /// `--field` #212121: inputs, cards, selected pills.
    pub field: Hsla,
    /// `--card` #212121.
    pub card: Hsla,
    /// `--popover` #1f1f1f: menus and popovers.
    pub popover: Hsla,
    /// `--muted` / `--accent` / `--secondary` #252626.
    pub muted: Hsla,
    /// `--input` #313131: input borders.
    pub input: Hsla,
    /// `--hover` rgb(255 255 255 / 0.035).
    pub hover: Hsla,
    /// `--hover-strong` rgb(255 255 255 / 0.07).
    pub hover_strong: Hsla,
    /// `--scrim` rgb(0 0 0 / 0.5): behind dialogs.
    pub scrim: Hsla,

    // Borders
    /// `--border` #262727: hairlines.
    pub border: Hsla,
    /// `--border-subtle` #212121.
    pub border_subtle: Hsla,
    /// `--border-strong` #3a3b3b.
    pub border_strong: Hsla,
    /// `--ring` #599de7: focus ring.
    pub ring: Hsla,

    // Text, brightest to dimmest
    /// `--body` / `--foreground` #f0f0f0.
    pub body: Hsla,
    /// `--soft` #b8b8b8.
    pub soft: Hsla,
    /// `--quiet` / `--muted-foreground` #999999.
    pub quiet: Hsla,
    /// `--faint` #8a8a8a.
    pub faint: Hsla,
    /// `--dim` #6b6b6b.
    pub dim: Hsla,
    /// `--ghost` #444546.
    pub ghost: Hsla,
    /// `--link` #7bafe9.
    pub link: Hsla,
    /// `::selection` rgb(89 157 231 / 0.35).
    pub selection: Hsla,

    // Status
    /// `--destructive` / `--status-err` #f07178.
    pub destructive: Hsla,
    /// `--status-err-text` #f49a9f.
    pub status_err_text: Hsla,
    /// `--status-run` #599de7.
    pub status_run: Hsla,
    /// `--status-wait` #f1b567.
    pub status_wait: Hsla,
    /// `--status-ok` #6fcf97.
    pub status_ok: Hsla,

    // SQL editor surface
    /// `--editor` #1e1f22.
    pub editor: Hsla,
    /// `--editor-line` #26282e: the caret-line band.
    pub editor_line: Hsla,
    /// `--editor-gutter` #4b5059: line numbers.
    pub editor_gutter: Hsla,
    /// `--editor-gutter-active` #a1a3ab: the caret line's number.
    pub editor_gutter_active: Hsla,
    /// `--editor-divider` #313438.
    pub editor_divider: Hsla,

    // SQL highlighting (token kinds of lib/sql.ts)
    pub sql_keyword: Hsla,
    pub sql_type: Hsla,
    pub sql_function: Hsla,
    pub sql_string: Hsla,
    pub sql_number: Hsla,
    pub sql_comment: Hsla,
    pub sql_param: Hsla,
    pub sql_table: Hsla,
    pub sql_column: Hsla,
    pub sql_operator: Hsla,
}

impl Global for Palette {}

/// `#rrggbb` as a colour.
fn hex(value: u32) -> Hsla {
    rgb(value).into()
}

/// `#rrggbbaa` as a colour.
fn hexa(value: u32) -> Hsla {
    rgba(value).into()
}

impl Palette {
    /// The `.dark` theme of main.css (Boa's only theme).
    pub fn dark() -> Self {
        let status_wait = hex(0xf1b567);
        let status_err_text = hex(0xf49a9f);
        let link = hex(0x7bafe9);
        let status_ok = hex(0x6fcf97);
        let dim = hex(0x6b6b6b);
        let quiet = hex(0x999999);
        Self {
            background: hex(0x151616),
            chrome: hex(0x181819),
            sidebar: hex(0x181819),
            field: hex(0x212121),
            card: hex(0x212121),
            popover: hex(0x1f1f1f),
            muted: hex(0x252626),
            input: hex(0x313131),
            hover: hexa(0xffffff09),
            hover_strong: hexa(0xffffff12),
            scrim: hexa(0x00000080),

            border: hex(0x262727),
            border_subtle: hex(0x212121),
            border_strong: hex(0x3a3b3b),
            ring: hex(0x599de7),

            body: hex(0xf0f0f0),
            soft: hex(0xb8b8b8),
            quiet,
            faint: hex(0x8a8a8a),
            dim,
            ghost: hex(0x444546),
            link,
            selection: hexa(0x599de759),

            destructive: hex(0xf07178),
            status_err_text,
            status_run: hex(0x599de7),
            status_wait,
            status_ok,

            editor: hex(0x1e1f22),
            editor_line: hex(0x26282e),
            editor_gutter: hex(0x4b5059),
            editor_gutter_active: hex(0xa1a3ab),
            editor_divider: hex(0x313438),

            sql_keyword: status_wait,
            sql_type: status_err_text,
            sql_function: link,
            sql_string: hex(0xb5bd68),
            sql_number: status_ok,
            sql_comment: dim,
            sql_param: hex(0x7bafe9),
            sql_table: hex(0xa78bfa),
            sql_column: hex(0xc4b5fd),
            sql_operator: quiet,
        }
    }
}

/// Access to Boa's palette from any context that derefs to [`App`].
pub trait ActivePalette {
    fn palette(&self) -> &Palette;
}

impl ActivePalette for App {
    fn palette(&self) -> &Palette {
        self.global::<Palette>()
    }
}

/// Installs Boa's palette and applies it, with Boa's fonts and radii, to
/// gpui-component's theme. Call after `gpui_component::init`.
pub fn init(cx: &mut App) {
    let palette = Palette::dark();
    Theme::change(ThemeMode::Dark, None, cx);
    let p = palette.clone();
    Theme::update(cx, move |theme| apply(&p, theme));
    cx.set_global(palette);
}

/// Maps the palette onto gpui-component's theme colours, fonts and radii.
fn apply(p: &Palette, theme: &mut Theme) {
    theme.font_family = SharedString::new_static(UI_FONT);
    theme.font_size = text::SM;
    theme.mono_font_family = SharedString::new_static(MONO_FONT);
    theme.mono_font_size = text::SM;
    theme.radius = radius::MD;
    theme.radius_lg = radius::LG;
    theme.shadow = true;

    let c = &mut theme.colors;
    c.background = p.background;
    c.foreground = p.body;
    c.border = p.border;
    c.input = p.input;
    c.ring = p.ring;
    c.caret = p.body;
    c.selection = p.selection;
    c.link = p.link;
    c.link_hover = p.link;
    c.link_active = p.link;
    c.overlay = p.scrim;
    c.window_border = p.border;
    c.drag_border = p.ring;

    c.muted = p.muted;
    c.muted_foreground = p.quiet;
    c.accent = p.muted;
    c.accent_foreground = p.body;
    c.secondary = p.muted;
    c.secondary_foreground = p.body;
    c.secondary_hover = p.hover_strong;
    c.secondary_active = p.hover_strong;
    c.primary = p.body;
    c.primary_foreground = p.background;
    c.popover = p.popover;
    c.popover_foreground = p.body;
    c.group_box = p.card;
    c.group_box_foreground = p.body;

    c.danger = p.destructive;
    c.danger_foreground = p.body;
    c.success = p.status_ok;
    c.warning = p.status_wait;
    c.info = p.status_run;

    c.sidebar = p.sidebar;
    c.sidebar_foreground = p.soft;
    c.sidebar_border = p.border;
    c.sidebar_accent = p.muted;
    c.sidebar_accent_foreground = p.body;
    c.sidebar_primary = p.body;
    c.sidebar_primary_foreground = p.background;

    c.title_bar = p.background;
    c.title_bar_border = p.border;
    c.status_bar = p.chrome;
    c.status_bar_border = p.border;
    c.tab_bar = p.background;
    c.tab = p.background;
    c.tab_active = p.field;
    c.tab_foreground = p.quiet;
    c.tab_active_foreground = p.body;

    c.list = p.background;
    c.list_even = p.background;
    c.list_head = p.chrome;
    c.list_hover = p.hover;
    c.list_active = p.hover_strong;
    c.list_active_border = p.ring;

    c.table = p.background;
    c.table_even = p.background;
    c.table_head = p.chrome;
    c.table_head_foreground = p.quiet;
    c.table_hover = p.hover;
    c.table_active = p.selection;
    c.table_active_border = p.ring;
    c.table_row_border = p.border_subtle;

    c.scrollbar = gpui::transparent_black();
    c.scrollbar_thumb = hexa(0xffffff1a);
    c.scrollbar_thumb_hover = hexa(0xffffff33);

    // The SQL editor (gpui-component's Editor reads these from the
    // highlight theme on every render): main.css `--editor*`.
    let mut highlight = (*theme.highlight_theme).clone();
    highlight.style.editor_background = Some(p.editor);
    highlight.style.editor_foreground = Some(p.body);
    highlight.style.editor_active_line = Some(p.editor_line);
    highlight.style.editor_line_number = Some(p.editor_gutter);
    highlight.style.editor_active_line_number = Some(p.editor_gutter_active);
    theme.highlight_theme = std::sync::Arc::new(highlight);
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::px;

    #[test]
    fn dark_palette_matches_main_css() {
        let p = Palette::dark();
        assert_eq!(p.background, hex(0x151616));
        assert_eq!(p.chrome, hex(0x181819));
        assert_eq!(p.field, hex(0x212121));
        assert_eq!(p.border, hex(0x262727));
        assert_eq!(p.body, hex(0xf0f0f0));
        assert_eq!(p.dim, hex(0x6b6b6b));
        assert_eq!(p.destructive, hex(0xf07178));
        // rgb(255 255 255 / 0.035) and 0.07
        assert!((p.hover.a - 0.035).abs() < 0.005, "{}", p.hover.a);
        assert!(
            (p.hover_strong.a - 0.07).abs() < 0.005,
            "{}",
            p.hover_strong.a
        );
        assert!((p.selection.a - 0.35).abs() < 0.005, "{}", p.selection.a);
    }

    #[test]
    fn type_scale() {
        let sizes = [text::XS, text::SM, text::BASE, text::LG, text::XL];
        assert_eq!(sizes, [px(12.), px(13.), px(14.), px(16.), px(20.)]);
    }
}
