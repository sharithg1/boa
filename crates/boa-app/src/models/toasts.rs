//! [`Toasts`]: the in-app toasts (store.ts `pushToast` / `dismissToast`,
//! rendered by views::toasts like Toasts.tsx).
//!
//! - A toast shows for its TTL ([`DEFAULT_TTL`], 6 s; `None`: until dismissed).
//! - One pushed with the `key` of a toast still showing replaces it in place,
//!   keeping its id (and position), and restarts its TTL: one status toast per
//!   connection, not one per retry round.
//! - At most [`MAX_TOASTS`] show; pushing a sixth drops the oldest.
//! - Kinds: info (no dot, plain title), success (green dot), error (red dot).
//!   An optional action button runs a callback and dismisses the toast.

use std::collections::HashMap;
use std::fmt;
use std::rc::Rc;
use std::time::Duration;

use gpui::{App, Context, SharedString, Task, Window};

/// How long a toast shows by default (pushToast's `ttlMs = 6000`).
pub const DEFAULT_TTL: Duration = Duration::from_millis(6000);

/// At most this many toasts show (`.slice(-5)`).
pub const MAX_TOASTS: usize = 5;

/// Identifies a toast; a keyed replacement keeps the id.
pub type ToastId = u64;

/// store.ts `Toast['kind']`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ToastKind {
    Info,
    Success,
    Error,
}

/// What a toast's button runs.
pub type ToastCallback = Rc<dyn Fn(&mut Window, &mut App)>;

/// A button on a toast (`action: { label, run }`).
#[derive(Clone)]
pub struct ToastAction {
    pub label: SharedString,
    pub run: ToastCallback,
}

impl fmt::Debug for ToastAction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ToastAction")
            .field("label", &self.label)
            .finish_non_exhaustive()
    }
}

/// A toast showing.
#[derive(Debug, Clone)]
pub struct Toast {
    pub id: ToastId,
    /// A toast pushed with the key of one still showing replaces it.
    pub key: Option<SharedString>,
    pub kind: ToastKind,
    pub title: SharedString,
    pub body: Option<SharedString>,
    pub action: Option<ToastAction>,
}

/// What to show (`Omit<Toast, 'id'>`), built with [`NewToast::info`] etc.
#[derive(Debug, Clone)]
pub struct NewToast {
    pub key: Option<SharedString>,
    pub kind: ToastKind,
    pub title: SharedString,
    pub body: Option<SharedString>,
    pub action: Option<ToastAction>,
}

impl NewToast {
    pub fn new(kind: ToastKind, title: impl Into<SharedString>) -> Self {
        Self {
            key: None,
            kind,
            title: title.into(),
            body: None,
            action: None,
        }
    }

    pub fn info(title: impl Into<SharedString>) -> Self {
        Self::new(ToastKind::Info, title)
    }

    pub fn success(title: impl Into<SharedString>) -> Self {
        Self::new(ToastKind::Success, title)
    }

    pub fn error(title: impl Into<SharedString>) -> Self {
        Self::new(ToastKind::Error, title)
    }

    /// Replaces a toast with the same key that is still showing.
    pub fn key(mut self, key: impl Into<SharedString>) -> Self {
        self.key = Some(key.into());
        self
    }

    pub fn body(mut self, body: impl Into<SharedString>) -> Self {
        self.body = Some(body.into());
        self
    }

    /// A button that runs `run` and dismisses the toast.
    pub fn action(
        mut self,
        label: impl Into<SharedString>,
        run: impl Fn(&mut Window, &mut App) + 'static,
    ) -> Self {
        self.action = Some(ToastAction {
            label: label.into(),
            run: Rc::new(run),
        });
        self
    }
}

/// The toasts entity.
#[derive(Default)]
pub struct Toasts {
    toasts: Vec<Toast>,
    next_id: ToastId,
    timers: HashMap<ToastId, Task<()>>,
}

impl fmt::Debug for Toasts {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Toasts")
            .field("toasts", &self.toasts)
            .finish_non_exhaustive()
    }
}

impl Toasts {
    pub fn new() -> Self {
        Self::default()
    }

    /// The toasts showing, oldest first.
    pub fn toasts(&self) -> &[Toast] {
        &self.toasts
    }

    /// Shows `toast` for [`DEFAULT_TTL`]. Returns its id.
    pub fn push(&mut self, toast: NewToast, cx: &mut Context<Self>) -> ToastId {
        self.push_for(toast, Some(DEFAULT_TTL), cx)
    }

    /// Shows `toast` for `ttl` (None: until dismissed). A toast with the same
    /// key still showing is replaced in place and keeps its id.
    pub fn push_for(
        &mut self,
        toast: NewToast,
        ttl: Option<Duration>,
        cx: &mut Context<Self>,
    ) -> ToastId {
        let same = toast
            .key
            .as_ref()
            .and_then(|key| self.toasts.iter().position(|t| t.key.as_ref() == Some(key)));
        let id = match same {
            Some(i) => self.toasts[i].id,
            None => {
                self.next_id += 1;
                self.next_id
            }
        };
        self.timers.remove(&id);
        let shown = Toast {
            id,
            key: toast.key,
            kind: toast.kind,
            title: toast.title,
            body: toast.body,
            action: toast.action,
        };
        match same {
            Some(i) => self.toasts[i] = shown,
            None => {
                self.toasts.push(shown);
                while self.toasts.len() > MAX_TOASTS {
                    let dropped = self.toasts.remove(0);
                    self.timers.remove(&dropped.id);
                }
            }
        }
        if let Some(ttl) = ttl.filter(|ttl| !ttl.is_zero()) {
            let timer = cx.spawn(async move |this, cx| {
                cx.background_executor().timer(ttl).await;
                this.update(cx, |this, cx| this.dismiss(id, cx)).ok();
            });
            self.timers.insert(id, timer);
        }
        cx.notify();
        id
    }

    /// Removes a toast (its × or action button, or its TTL).
    pub fn dismiss(&mut self, id: ToastId, cx: &mut Context<Self>) {
        self.timers.remove(&id);
        let before = self.toasts.len();
        self.toasts.retain(|t| t.id != id);
        if self.toasts.len() != before {
            cx.notify();
        }
    }

    /// Removes the toast with `key`, if one shows (e.g. a connection's failure
    /// toast once it is back).
    pub fn dismiss_key(&mut self, key: &str, cx: &mut Context<Self>) {
        if let Some(id) = self.find_key(key).map(|t| t.id) {
            self.dismiss(id, cx);
        }
    }

    /// The toast with `key`, if one shows.
    pub fn find_key(&self, key: &str) -> Option<&Toast> {
        self.toasts.iter().find(|t| t.key.as_deref() == Some(key))
    }

    /// store.ts `reportError(title, err)`: an error toast with the message.
    pub fn report_error(
        &mut self,
        title: impl Into<SharedString>,
        message: impl Into<SharedString>,
        cx: &mut Context<Self>,
    ) -> ToastId {
        self.push(NewToast::error(title).body(message), cx)
    }
}
