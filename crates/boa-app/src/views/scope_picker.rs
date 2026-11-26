//! ScopePicker.tsx: DataGrip's "N of M" badge on a connection (databases) or
//! a database (schemas). Clicking it opens a checklist of what the explorer
//! shows and introspects: a filter box, an "All" row (also covers ones
//! created later: the `'*'` scope), the default item tagged. Enter or a click
//! outside applies, Esc cancels.
//!
//! [`ScopeDraft`] is the picker's state and rules (pure, unit tested);
//! [`ScopePicker`] the popover view, which emits [`ScopePickerEvent`]. The
//! explorer owns the popover, places it under the badge and saves an applied
//! scope into the prefs (`setScope`); [`crate::models::catalogs`] then loads
//! what became visible.

use std::collections::BTreeSet;

use boa_core::state::prefs::Scope;
use gpui::prelude::*;
use gpui::{
    App, Context, DismissEvent, Entity, EventEmitter, FocusHandle, Focusable, MouseButton,
    SharedString, Subscription, Window, div, px,
};
use gpui_component::input::{Input, InputEvent, InputState};

use super::chrome::{IconName, icon};
use super::sidebar::tree::{ScopeBadge, ScopeKind};
use crate::actions::{ApplyScope, Dismiss};
use crate::theme::{self, ActivePalette as _};

/// The picker's state: the draft pick and the filter (ScopePicker.tsx's
/// `all` / `draft` / `filter`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScopeDraft {
    /// Every name, in display order.
    pub items: Vec<String>,
    /// What showed when it opened (resolved, so the default counts).
    pub selected: Vec<String>,
    /// The saved scope was `'*'` when it opened.
    pub saved_all: bool,
    /// "All" is ticked (`'*'`).
    pub all: bool,
    pub draft: BTreeSet<String>,
    pub filter: String,
}

impl ScopeDraft {
    /// Opening: the current pick, no filter.
    pub fn new(items: Vec<String>, selected: Vec<String>, is_all: bool) -> Self {
        Self {
            draft: selected.iter().cloned().collect(),
            items,
            selected,
            saved_all: is_all,
            all: is_all,
            filter: String::new(),
        }
    }

    /// The items matching the filter (case-insensitive substring).
    pub fn shown(&self) -> Vec<&str> {
        let q = self.filter.trim().to_lowercase();
        self.items
            .iter()
            .filter(|i| q.is_empty() || i.to_lowercase().contains(&q))
            .map(String::as_str)
            .collect()
    }

    pub fn is_on(&self, name: &str) -> bool {
        self.all || self.draft.contains(name)
    }

    /// How many are ticked.
    pub fn count(&self) -> usize {
        if self.all {
            self.items.len()
        } else {
            self.items
                .iter()
                .filter(|i| self.draft.contains(*i))
                .count()
        }
    }

    /// Ticks / unticks one; unticking one while "All" is on keeps the rest.
    pub fn toggle(&mut self, name: &str) {
        let mut base: BTreeSet<String> = if self.all {
            self.items.iter().cloned().collect()
        } else {
            self.draft.clone()
        };
        if !base.remove(name) {
            base.insert(name.to_owned());
        }
        self.all = false;
        self.draft = base;
    }

    /// The "All" row: on → everything incl. future ones; off → nothing.
    pub fn toggle_all(&mut self) {
        if self.all {
            self.all = false;
            self.draft.clear();
        } else {
            self.all = true;
        }
    }

    /// The "All" row's checkbox: ticked, mixed (some ticked) or empty.
    pub fn all_state(&self) -> CheckState {
        if self.all {
            CheckState::Checked
        } else if self.count() > 0 {
            CheckState::Mixed
        } else {
            CheckState::Unchecked
        }
    }

    /// Applying: the new scope, or None when nothing changed.
    pub fn result(&self) -> Option<Scope> {
        let changed = self.all != self.saved_all
            || (!self.all
                && (self.draft.len() != self.selected.len()
                    || self.selected.iter().any(|s| !self.draft.contains(s))));
        changed.then(|| {
            if self.all {
                Scope::All
            } else {
                Scope::Some(
                    self.items
                        .iter()
                        .filter(|i| self.draft.contains(*i))
                        .cloned()
                        .collect(),
                )
            }
        })
    }
}

/// A checkbox's look.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckState {
    Unchecked,
    Checked,
    Mixed,
}

/// What the picker asks its owner to do. Either way it is done and closes.
#[derive(Debug, Clone, PartialEq)]
pub enum ScopePickerEvent {
    /// Save this scope under [`ScopePicker::key`].
    Apply(Scope),
    /// Closed without a change (Esc, or nothing changed).
    Closed,
}

/// The popover.
pub struct ScopePicker {
    kind: ScopeKind,
    key: String,
    default_item: Option<String>,
    draft: ScopeDraft,
    input: Entity<InputState>,
    focus_handle: FocusHandle,
    done: bool,
    _subscriptions: Vec<Subscription>,
}

impl std::fmt::Debug for ScopePicker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ScopePicker")
            .field("key", &self.key)
            .field("draft", &self.draft)
            .finish_non_exhaustive()
    }
}

impl EventEmitter<ScopePickerEvent> for ScopePicker {}
impl EventEmitter<DismissEvent> for ScopePicker {}

impl Focusable for ScopePicker {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl ScopePicker {
    /// A picker for `badge` (focus its filter box with [`ScopePicker::focus_filter`]).
    pub fn new(badge: &ScopeBadge, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let noun = badge.kind.noun();
        let input = cx.new(|cx| InputState::new(window, cx).placeholder(format!("Filter {noun}")));
        let subscriptions = vec![cx.subscribe(&input, |this, input, event, cx| match event {
            InputEvent::Change => {
                this.draft.filter = input.read(cx).value().to_string();
                cx.notify();
            }
            InputEvent::PressEnter { .. } => this.apply(cx),
            _ => {}
        })];
        Self {
            kind: badge.kind,
            key: badge.key.clone(),
            default_item: badge.default_item.clone(),
            draft: ScopeDraft::new(badge.items.clone(), badge.selected.clone(), badge.is_all),
            input,
            focus_handle: cx.focus_handle(),
            done: false,
            _subscriptions: subscriptions,
        }
    }

    /// The prefs `scopes` key it edits.
    pub fn key(&self) -> &str {
        &self.key
    }

    pub fn draft(&self) -> &ScopeDraft {
        &self.draft
    }

    /// Focuses the filter box (`autoFocus`).
    pub fn focus_filter(&self, window: &mut Window, cx: &mut App) {
        self.input.update(cx, |s, cx| s.focus(window, cx));
    }

    /// Sets the filter text (as typing would).
    pub fn set_filter(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.input
            .update(cx, |s, cx| s.set_value(text.to_owned(), window, cx));
        self.draft.filter = text.to_owned();
        cx.notify();
    }

    pub fn toggle(&mut self, name: &str, cx: &mut Context<Self>) {
        self.draft.toggle(name);
        cx.notify();
    }

    pub fn toggle_all(&mut self, cx: &mut Context<Self>) {
        self.draft.toggle_all();
        cx.notify();
    }

    /// Enter / a click outside: emits the new scope (if it changed).
    pub fn apply(&mut self, cx: &mut Context<Self>) {
        if std::mem::replace(&mut self.done, true) {
            return;
        }
        match self.draft.result() {
            Some(scope) => cx.emit(ScopePickerEvent::Apply(scope)),
            None => cx.emit(ScopePickerEvent::Closed),
        }
        cx.emit(DismissEvent);
    }

    /// Esc: closes without applying.
    pub fn cancel(&mut self, cx: &mut Context<Self>) {
        if std::mem::replace(&mut self.done, true) {
            return;
        }
        cx.emit(ScopePickerEvent::Closed);
        cx.emit(DismissEvent);
    }

    fn checkbox(state: CheckState, cx: &App) -> gpui::Div {
        let p = cx.palette();
        let on = state != CheckState::Unchecked;
        let b = div()
            .flex_none()
            .size(px(16.))
            .rounded(px(4.))
            .flex()
            .items_center()
            .justify_center()
            .border_1();
        let b = if on {
            b.bg(p.status_run).border_color(p.status_run)
        } else {
            b.border_color(p.input).bg(p.field)
        };
        match state {
            CheckState::Checked => b.child(icon(IconName::Check, 12., gpui::white())),
            CheckState::Mixed => b.child(icon(IconName::Minus, 12., gpui::white())),
            CheckState::Unchecked => b,
        }
    }

    fn row(
        &self,
        id: impl Into<SharedString>,
        state: CheckState,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let p = cx.palette();
        div()
            .id(gpui::ElementId::Name(id.into()))
            .flex()
            .flex_none()
            .items_center()
            .gap(px(8.))
            .h(px(28.))
            .px(px(12.))
            .text_size(theme::text::SM)
            .text_color(p.soft)
            .cursor_default()
            .hover(|s| s.bg(p.hover))
            .child(Self::checkbox(state, cx))
    }
}

impl Render for ScopePicker {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = cx.palette().clone();
        let noun = self.kind.noun();
        let total = self.draft.items.len();
        let mut list = div()
            .id("scope-picker-list")
            .flex()
            .flex_col()
            .max_h(px(360.))
            .overflow_y_scroll()
            .py(px(4.));
        if self.draft.filter.is_empty() {
            list = list.child(
                self.row("scope-all", self.draft.all_state(), cx)
                    .border_b_1()
                    .border_color(p.border_subtle)
                    .on_click(cx.listener(|this, _, _, cx| this.toggle_all(cx)))
                    .child(format!("All {noun}"))
                    .child(
                        div()
                            .text_size(theme::text::XS)
                            .text_color(p.dim)
                            .child("includes new ones"),
                    ),
            );
        }
        let shown: Vec<String> = self.draft.shown().into_iter().map(str::to_owned).collect();
        if shown.is_empty() {
            list = list.child(
                div()
                    .px(px(12.))
                    .py(px(12.))
                    .text_size(theme::text::SM)
                    .text_color(p.dim)
                    .child(format!("No {noun} match")),
            );
        }
        for name in shown {
            let state = if self.draft.is_on(&name) {
                CheckState::Checked
            } else {
                CheckState::Unchecked
            };
            let is_default = self.default_item.as_deref() == Some(name.as_str());
            let toggled = name.clone();
            list = list.child(
                self.row(format!("scope-item-{name}"), state, cx)
                    .on_click(cx.listener(move |this, _, _, cx| this.toggle(&toggled, cx)))
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .font_family(theme::MONO_FONT)
                            .child(name),
                    )
                    .when(is_default, |d| {
                        d.child(
                            div()
                                .flex_none()
                                .text_size(theme::text::XS)
                                .text_color(p.dim)
                                .child("(default)"),
                        )
                    }),
            );
        }
        div()
            .id("scope-picker")
            .key_context(crate::keymap::SCOPE_PICKER)
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(|this, _: &ApplyScope, _, cx| this.apply(cx)))
            .on_action(cx.listener(|this, _: &Dismiss, _, cx| this.cancel(cx)))
            .on_mouse_down_out(cx.listener(|this, _, _, cx| this.apply(cx)))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
            .occlude()
            .w(px(340.))
            .flex()
            .flex_col()
            .rounded(theme::radius::LG)
            .border_1()
            .border_color(p.border)
            .bg(p.popover)
            .shadow_lg()
            .text_color(p.soft)
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(px(8.))
                    .h(px(36.))
                    .px(px(12.))
                    .border_b_1()
                    .border_color(p.border)
                    .child(icon(IconName::Search, 14., p.dim))
                    .child(
                        div().flex_1().min_w_0().child(
                            Input::new(&self.input)
                                .appearance(false)
                                .px_0()
                                .py_0()
                                .h(px(28.))
                                .text_size(theme::text::SM),
                        ),
                    )
                    .child(
                        div()
                            .flex_none()
                            .text_size(theme::text::XS)
                            .text_color(p.dim)
                            .child(format!("{} of {total}", self.draft.count())),
                    ),
            )
            .child(list)
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .h(px(32.))
                    .px(px(12.))
                    .border_t_1()
                    .border_color(p.border)
                    .text_size(theme::text::XS)
                    .text_color(p.dim)
                    .child("Enter or click outside to apply · Esc cancels"),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| (*s).to_owned()).collect()
    }

    fn draft(selected: &[&str], all: bool) -> ScopeDraft {
        ScopeDraft::new(
            names(&["postgres", "app", "analytics"]),
            names(selected),
            all,
        )
    }

    #[test]
    fn opens_on_the_current_pick() {
        let d = draft(&["app"], false);
        assert!(d.is_on("app") && !d.is_on("postgres"));
        assert_eq!(d.count(), 1);
        assert_eq!(d.all_state(), CheckState::Mixed);
        assert_eq!(d.result(), None, "unchanged: nothing to apply");
        let d = draft(&["postgres", "app", "analytics"], true);
        assert_eq!(d.all_state(), CheckState::Checked);
        assert_eq!(d.count(), 3);
        assert_eq!(d.result(), None);
    }

    #[test]
    fn ticking_applies_the_pick_in_display_order() {
        let mut d = draft(&["app"], false);
        d.toggle("postgres");
        assert_eq!(d.result(), Some(Scope::Some(names(&["postgres", "app"]))));
        d.toggle("postgres");
        assert_eq!(d.result(), None, "back to the saved pick");
        d.toggle("app");
        assert_eq!(d.result(), Some(Scope::Some(vec![])));
        assert_eq!(d.all_state(), CheckState::Unchecked);
    }

    #[test]
    fn all_means_star_and_unticking_one_keeps_the_rest() {
        let mut d = draft(&["app"], false);
        d.toggle_all();
        assert!(d.all);
        assert_eq!(d.result(), Some(Scope::All));
        // Unticking one while All is on: the others stay ticked, no longer '*'.
        d.toggle("analytics");
        assert!(!d.all);
        assert_eq!(d.result(), Some(Scope::Some(names(&["postgres", "app"]))));
        // All off from All: nothing.
        let mut d = draft(&["postgres", "app", "analytics"], true);
        d.toggle_all();
        assert_eq!(d.count(), 0);
        assert_eq!(d.result(), Some(Scope::Some(vec![])));
        // Saved '*' and every item ticked by hand is still a change (no longer '*').
        let mut d = draft(&["postgres", "app", "analytics"], true);
        d.toggle("app");
        d.toggle("app");
        assert_eq!(
            d.result(),
            Some(Scope::Some(names(&["postgres", "app", "analytics"])))
        );
    }

    #[test]
    fn filter_narrows_the_list_case_insensitively() {
        let mut d = draft(&["app"], false);
        d.filter = "  AN ".into();
        assert_eq!(d.shown(), ["analytics"]);
        d.filter = "p".into();
        assert_eq!(d.shown(), ["postgres", "app"]);
        d.filter = "zzz".into();
        assert!(d.shown().is_empty());
        // The count covers every item, not just the shown ones.
        assert_eq!(d.count(), 1);
    }
}
