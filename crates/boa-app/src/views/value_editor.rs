//! ValueEditor.tsx: the larger editor for one cell of the data editor, for
//! long or multi-line text and JSON (with Format). Opened from the result
//! grid with ⇧↵, or by editing a multi-line / JSON / long value. ⌘↵ or
//! Apply applies, Esc / × cancels, "Set NULL" stores NULL (distinct from an
//! empty string: the text is NULL until the user types).

use boa_core::grid::values::format_json;
use boa_core::model::Value;
use gpui::prelude::*;
use gpui::{
    App, Context, Entity, EventEmitter, FocusHandle, Focusable, FontWeight, SharedString,
    Subscription, Window, div, px,
};
use gpui_component::Icon;
use gpui_component::input::{InputEvent, Textarea, TextareaState};

use crate::actions::{Dismiss, SaveValue};
use crate::theme::{self, ActivePalette as _, Palette};
use crate::views::chrome::{self, IconName};

/// Key context of the editor (`ValueEditor > Input`: ⌘↵ saves, Esc cancels).
pub const CONTEXT: &str = "ValueEditor";

/// The cell to edit (`ValueTarget`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValueTarget {
    pub column: String,
    pub ty: String,
    /// Row number as the grid shows it.
    pub row: usize,
    /// NULL for NULL (and a new row's `<default>`).
    pub value: Value,
    /// A json / jsonb column: offers Format.
    pub json: bool,
}

/// What the editor asks of the grid.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ValueEditorEvent {
    /// Apply this value (the original when untouched).
    Apply(Value),
    Cancel,
}

/// The value editor.
pub struct ValueEditor {
    focus_handle: FocusHandle,
    target: ValueTarget,
    input: Entity<TextareaState>,
    /// NULL until the user types (an emptied field means '').
    is_null: bool,
    touched: bool,
    /// Text set by us (Format, opening): its Change event isn't the user typing.
    setting: bool,
    _events: Subscription,
}

impl std::fmt::Debug for ValueEditor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ValueEditor")
            .field("target", &self.target)
            .field("is_null", &self.is_null)
            .field("touched", &self.touched)
            .finish_non_exhaustive()
    }
}

impl EventEmitter<ValueEditorEvent> for ValueEditor {}

impl ValueEditor {
    pub fn new(target: ValueTarget, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let text = target.value.clone().unwrap_or_default();
        let input = cx.new(|cx| {
            TextareaState::new(window, cx)
                .placeholder("<null>")
                .default_value(text)
        });
        let events = cx.subscribe(&input, |this: &mut Self, _, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) && !this.setting {
                this.is_null = false;
                this.touched = true;
                cx.notify();
            }
        });
        Self {
            focus_handle: cx.focus_handle(),
            is_null: target.value.is_none(),
            target,
            input,
            touched: false,
            setting: false,
            _events: events,
        }
    }

    pub fn target(&self) -> &ValueTarget {
        &self.target
    }

    /// The text field's content.
    pub fn text(&self, cx: &App) -> String {
        self.input.read(cx).value().to_string()
    }

    pub fn is_null(&self) -> bool {
        self.is_null
    }

    /// Focuses the text field.
    pub fn focus(&self, window: &mut Window, cx: &mut App) {
        let handle = self.input.read(cx).focus_handle(cx);
        window.focus(&handle, cx);
    }

    /// Replaces the text (as if typed).
    pub fn set_text(&mut self, text: String, window: &mut Window, cx: &mut Context<Self>) {
        self.setting = true;
        self.input.update(cx, |s, cx| s.set_value(text, window, cx));
        self.setting = false;
        self.is_null = false;
        self.touched = true;
        cx.notify();
    }

    /// Format (JSON): pretty-prints the text.
    pub fn format(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.is_null {
            return;
        }
        let pretty = format_json(&self.text(cx));
        self.set_text(pretty, window, cx);
    }

    /// Set NULL.
    pub fn set_null(&mut self, cx: &mut Context<Self>) {
        self.is_null = true;
        self.touched = true;
        cx.notify();
    }

    /// The value Apply would store.
    pub fn value(&self, cx: &App) -> Value {
        if !self.touched {
            self.target.value.clone()
        } else if self.is_null {
            None
        } else {
            Some(self.text(cx))
        }
    }

    /// ⌘↵ / Apply.
    pub fn apply(&mut self, cx: &mut Context<Self>) {
        let value = self.value(cx);
        cx.emit(ValueEditorEvent::Apply(value));
    }

    /// Esc / ×.
    pub fn cancel(&mut self, cx: &mut Context<Self>) {
        cx.emit(ValueEditorEvent::Cancel);
    }

    fn button(
        id: &'static str,
        label: impl IntoElement,
        enabled: bool,
        p: &Palette,
    ) -> gpui::Stateful<gpui::Div> {
        div()
            .id(id)
            .flex()
            .items_center()
            .h(px(24.))
            .px(px(8.))
            .rounded(theme::radius::SM)
            .text_size(theme::text::XS)
            .text_color(if enabled { p.quiet } else { p.ghost })
            .when(enabled, |d| {
                d.cursor_pointer()
                    .hover(|s| s.bg(p.hover).text_color(p.soft))
            })
            .child(label)
    }
}

impl Focusable for ValueEditor {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for ValueEditor {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = cx.palette().clone();
        let length = self.text(cx).encode_utf16().count();
        let size = if self.is_null {
            " · NULL".to_owned()
        } else {
            format!(" · {} chars", crate::grid::page::grouped(length))
        };
        let can_change = !self.is_null;
        let header = div()
            .flex_none()
            .flex()
            .items_center()
            .gap(px(12.))
            .py(px(12.))
            .pl(px(16.))
            .pr(px(12.))
            .border_b_1()
            .border_color(p.border)
            .child(
                div()
                    .min_w_0()
                    .child(
                        div()
                            .truncate()
                            .font_family(theme::MONO_FONT)
                            .text_size(theme::text::SM)
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(p.body)
                            .child(SharedString::from(self.target.column.clone())),
                    )
                    .child(
                        div()
                            .flex()
                            .text_size(theme::text::XS)
                            .text_color(p.quiet)
                            .child(format!("Row {} · ", self.target.row))
                            .child(
                                div()
                                    .text_color(p.sql_type)
                                    .child(SharedString::from(self.target.ty.clone())),
                            )
                            .child(size),
                    ),
            )
            .child(
                div()
                    .ml_auto()
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(px(4.))
                    .when(self.target.json, |d| {
                        d.child(Self::button("value-format", "Format", can_change, &p).when(
                            can_change,
                            |b| {
                                b.on_click(
                                    cx.listener(|this, _, window, cx| this.format(window, cx)),
                                )
                            },
                        ))
                    })
                    .child(
                        Self::button("value-set-null", "Set NULL", can_change, &p)
                            .when(can_change, |b| {
                                b.on_click(cx.listener(|this, _, _, cx| this.set_null(cx)))
                            }),
                    )
                    .child(
                        Self::button(
                            "value-close",
                            Icon::new(IconName::X).size(px(14.)),
                            true,
                            &p,
                        )
                        .on_click(cx.listener(|this, _, _, cx| this.cancel(cx))),
                    ),
            );
        let field = div()
            .flex_1()
            .min_h_0()
            .bg(p.editor)
            .px(px(16.))
            .py(px(12.))
            .font_family(theme::MONO_FONT)
            .text_size(theme::text::SM)
            .text_color(p.body)
            .child(
                Textarea::new(&self.input)
                    .appearance(false)
                    .h_full()
                    .text_size(theme::text::SM),
            );
        let footer = div()
            .flex_none()
            .flex()
            .items_center()
            .gap(px(8.))
            .px(px(16.))
            .py(px(10.))
            .border_t_1()
            .border_color(p.border)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .text_size(theme::text::XS)
                    .text_color(p.dim)
                    .child(chrome::kbd("⌘↵", cx))
                    .child("apply ·")
                    .child(chrome::kbd("Esc", cx))
                    .child("cancel"),
            )
            .child(
                div()
                    .id("value-apply")
                    .ml_auto()
                    .h(px(24.))
                    .px(px(12.))
                    .flex()
                    .items_center()
                    .rounded(theme::radius::SM)
                    .bg(p.body)
                    .text_color(p.background)
                    .text_size(theme::text::XS)
                    .font_weight(FontWeight::MEDIUM)
                    .cursor_pointer()
                    .hover(|s| s.opacity(0.9))
                    .on_click(cx.listener(|this, _, _, cx| this.apply(cx)))
                    .child("Apply"),
            );
        div()
            .id("value-editor")
            .key_context(CONTEXT)
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(|this, _: &SaveValue, _, cx| this.apply(cx)))
            .on_action(cx.listener(|this, _: &Dismiss, _, cx| this.cancel(cx)))
            .size_full()
            .flex()
            .flex_col()
            .font_family(theme::UI_FONT)
            .child(header)
            .child(field)
            .child(footer)
    }
}
