//! Quits that don't go through Boa's menu or ⌘Q: **Dock → Quit, logging
//! out, restarting or shutting down** (P0-2 in docs/parity-audit.md).
//!
//! All of them reach AppKit's `-[NSApplication terminate:]`, which asks the
//! app delegate's `applicationShouldTerminate:`. GPUI 0.3.14's delegate
//! (`GPUIApplicationDelegate`) doesn't implement it, so those quits went
//! straight to `applicationWillTerminate:` and the server rolled open
//! transactions back silently. The Tauri app held them through
//! `RunEvent::ExitRequested` (src `crates/boa-desktop/src/lib.rs:345-360`).
//!
//! [`intercept`] adds `applicationShouldTerminate:` to GPUI's delegate class
//! at run time (`class_addMethod`; it refuses if the class already has one).
//! The method asks a handler the app installs, which decides with
//! [`decide`] from a [`QuitGuard`] (plain `Cell`s, so the hook never borrows
//! GPUI's `App`):
//!
//! - the app is already quitting (asked and answered, ⌘Q with nothing open,
//!   …), or no transaction is open: [`TerminateReply::Now`], the quit goes on
//!   with every `on_app_quit` hook (session flush, export cleanup, tunnels);
//! - a transaction is open and the user wasn't asked in the last 3 s
//!   ([`QuitGate`], shared with ⌘Q): [`TerminateReply::Cancel`], and the app
//!   shows the usual "Quit with an open transaction?" prompt; confirming rolls
//!   back and quits through the normal path (now `quitting`, so let through);
//! - asked within the last 3 s: [`TerminateReply::Now`] (a stuck UI can't
//!   keep the app open), as in Tauri.
//!
//! Cancelling a logout or restart makes macOS say "Boa cancelled logout",
//! as the Tauri app (tao answers `NSTerminateCancel` for a held exit).
//!
//! The `unsafe` (adding the method, and the function-pointer cast it needs)
//! is confined to the macOS `hook` module below, with a scoped
//! `#[allow(unsafe_code)]` approved by the user for exactly this.

use std::cell::Cell;

use boa_core::state::guards::{QuitDecision, QuitGate};

/// The answer to `applicationShouldTerminate:` (`NSTerminateNow` /
/// `NSTerminateCancel`; `NSTerminateLater` isn't used: AppKit would then run
/// its loop in the modal-panel mode until a reply, which GPUI doesn't expect).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminateReply {
    /// Quit now.
    Now,
    /// Don't quit (the app asks the user instead).
    Cancel,
}

/// Whether a terminate request goes through: `quitting` (the app already
/// decided to quit), `open_tx` (some session has a transaction open), and
/// the ⌘Q gate (asked within 3 s: through). The gate is only consulted, and
/// restarted, when there is something to ask about.
pub fn decide(quitting: bool, open_tx: bool, gate: &mut QuitGate, now_ms: u64) -> TerminateReply {
    if quitting || !open_tx {
        return TerminateReply::Now;
    }
    match gate.request(now_ms) {
        QuitDecision::Quit => TerminateReply::Now,
        QuitDecision::Ask => TerminateReply::Cancel,
    }
}

/// What decides a quit, readable without GPUI's `App` (AppKit calls the hook
/// on the main thread, between GPUI's updates; the app keeps this current).
#[derive(Debug, Default)]
pub struct QuitGuard {
    gate: Cell<QuitGate>,
    quitting: Cell<bool>,
    open_tx: Cell<bool>,
}

impl QuitGuard {
    /// A quit request at `now_ms` through the 3 s gate (⌘Q, the menu).
    pub fn request(&self, now_ms: u64) -> QuitDecision {
        let mut gate = self.gate.get();
        let decision = gate.request(now_ms);
        self.gate.set(gate);
        decision
    }

    /// A terminate request from AppKit at `now_ms` ([`decide`]).
    pub fn should_terminate(&self, now_ms: u64) -> TerminateReply {
        let mut gate = self.gate.get();
        let reply = decide(self.quitting.get(), self.open_tx.get(), &mut gate, now_ms);
        self.gate.set(gate);
        reply
    }

    /// The app is on its way out: let every terminate through.
    pub fn set_quitting(&self) {
        self.quitting.set(true);
    }

    pub fn is_quitting(&self) -> bool {
        self.quitting.get()
    }

    /// Whether a transaction is open (mirrored from the Transactions model).
    pub fn set_open_tx(&self, open: bool) {
        self.open_tx.set(open);
    }

    pub fn open_tx(&self) -> bool {
        self.open_tx.get()
    }
}

/// Routes AppKit's `applicationShouldTerminate:` to `handler` (called on the
/// main thread, outside GPUI's updates; it must not borrow the `App`). Call
/// once on the main thread after GPUI finished launching (in the `run`
/// callback). False if it couldn't be installed (logged; terminates then go
/// through unasked, as before), and always on other platforms.
pub fn intercept(handler: impl Fn() -> TerminateReply + 'static) -> bool {
    #[cfg(target_os = "macos")]
    {
        hook::install(Box::new(handler))
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = handler;
        false
    }
}

#[cfg(target_os = "macos")]
#[allow(unsafe_code)]
mod hook {
    use std::cell::RefCell;
    use std::ffi::CString;

    use objc2::encode::Encode;
    use objc2::runtime::{AnyClass, AnyObject, Imp, Sel};
    use objc2::{MainThreadMarker, sel};
    use objc2_app_kit::{NSApplication, NSApplicationTerminateReply};

    use super::TerminateReply;

    type Handler = Box<dyn Fn() -> TerminateReply>;

    thread_local! {
        /// The app's handler; AppKit calls the method on the main thread.
        static HANDLER: RefCell<Option<Handler>> = const { RefCell::new(None) };
        /// The method was added (by us) to the delegate's class.
        static ADDED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    }

    /// `-(NSApplicationTerminateReply)applicationShouldTerminate:(NSApplication *)sender`
    /// on GPUI's delegate. Without a handler (or while it is being replaced,
    /// or during thread teardown): Now, as before the hook.
    extern "C-unwind" fn should_terminate(
        _this: &AnyObject,
        _cmd: Sel,
        _sender: *mut AnyObject,
    ) -> NSApplicationTerminateReply {
        let reply = HANDLER
            .try_with(|h| {
                h.try_borrow()
                    .ok()
                    .and_then(|h| h.as_ref().map(|handler| handler()))
            })
            .ok()
            .flatten()
            .unwrap_or(TerminateReply::Now);
        tracing::info!(?reply, "applicationShouldTerminate:");
        match reply {
            TerminateReply::Now => NSApplicationTerminateReply::TerminateNow,
            TerminateReply::Cancel => NSApplicationTerminateReply::TerminateCancel,
        }
    }

    pub(super) fn install(handler: Handler) -> bool {
        let Some(mtm) = MainThreadMarker::new() else {
            tracing::warn!("quit guard: not on the main thread; Dock quits won't ask");
            return false;
        };
        let app = NSApplication::sharedApplication(mtm);
        let Some(delegate) = app.delegate() else {
            tracing::warn!("quit guard: NSApp has no delegate; Dock quits won't ask");
            return false;
        };
        HANDLER.with_borrow_mut(|h| *h = Some(handler));
        if ADDED.get() {
            return true;
        }
        let object: &AnyObject = (*delegate).as_ref();
        let class: &AnyClass = object.class();
        let name = sel!(applicationShouldTerminate:);
        if class.instance_method(name).is_some() {
            // A newer GPUI implements it: leave it alone (gpui is pinned with
            // `=`; re-check this module when bumping it).
            tracing::warn!(
                class = %class.name().to_string_lossy(),
                "quit guard: the app delegate already answers applicationShouldTerminate:; not hooked"
            );
            HANDLER.with_borrow_mut(|h| *h = None);
            return false;
        }
        // "Q@:@" on 64-bit: the return type, self, _cmd, sender.
        let Ok(types) = CString::new(format!(
            "{}{}{}{}",
            NSApplicationTerminateReply::ENCODING,
            <*mut AnyObject as Encode>::ENCODING,
            Sel::ENCODING,
            <*mut AnyObject as Encode>::ENCODING,
        )) else {
            return false;
        };
        let method: extern "C-unwind" fn(
            &AnyObject,
            Sel,
            *mut AnyObject,
        ) -> NSApplicationTerminateReply = should_terminate;
        // SAFETY: `Imp` is the untyped function-pointer type the runtime
        // stores; it is never called through this type. The runtime calls it
        // as an Objective-C method with the signature `types` describes
        // (self, _cmd, one object argument, an NSUInteger result), which is
        // exactly `should_terminate`'s ABI (`&AnyObject` and `*mut AnyObject`
        // are object pointers; `NSApplicationTerminateReply` is a
        // `#[repr(transparent)]` NSUInteger). Function pointers have the same
        // size.
        let imp = unsafe {
            std::mem::transmute::<
                extern "C-unwind" fn(
                    &AnyObject,
                    Sel,
                    *mut AnyObject,
                ) -> NSApplicationTerminateReply,
                Imp,
            >(method)
        };
        // SAFETY: `class` is a live, registered class (the class of NSApp's
        // delegate); adding a method to a registered class is allowed by the
        // runtime (it takes its own lock) and we checked it has no
        // implementation of this selector, so nothing is replaced. `types` is
        // a NUL-terminated encoding matching `imp` (above) and outlives the
        // call (the runtime copies it). We're on the main thread, where AppKit
        // would send the message.
        let added = unsafe {
            objc2::ffi::class_addMethod(
                std::ptr::from_ref(class).cast_mut(),
                name,
                imp,
                types.as_ptr(),
            )
        };
        if !added.as_bool() {
            tracing::warn!("quit guard: class_addMethod failed; Dock quits won't ask");
            HANDLER.with_borrow_mut(|h| *h = None);
            return false;
        }
        ADDED.set(true);
        tracing::debug!("quit guard: applicationShouldTerminate: installed");
        true
    }
}

#[cfg(test)]
mod tests {
    use boa_core::state::guards::QUIT_AGAIN_MS;

    use super::*;

    const T: u64 = 1_700_000_000_000;

    #[test]
    fn nothing_open_quits_now_without_touching_the_gate() {
        let mut gate = QuitGate::default();
        assert_eq!(decide(false, false, &mut gate, T), TerminateReply::Now);
        assert_eq!(gate, QuitGate::default());
        // So a ⌘Q right after still asks.
        assert_eq!(gate.request(T + 1), QuitDecision::Ask);
    }

    #[test]
    fn an_open_transaction_cancels_and_asks_then_a_second_quit_within_3_s_goes_through() {
        let mut gate = QuitGate::default();
        assert_eq!(decide(false, true, &mut gate, T), TerminateReply::Cancel);
        assert_eq!(
            decide(false, true, &mut gate, T + QUIT_AGAIN_MS),
            TerminateReply::Now
        );
        // Long after: asks again.
        assert_eq!(
            decide(false, true, &mut gate, T + 3 * QUIT_AGAIN_MS),
            TerminateReply::Cancel
        );
    }

    #[test]
    fn once_quitting_every_terminate_goes_through() {
        let mut gate = QuitGate::default();
        assert_eq!(decide(true, true, &mut gate, T), TerminateReply::Now);
        assert_eq!(decide(true, true, &mut gate, T + 1), TerminateReply::Now);
        assert_eq!(gate, QuitGate::default(), "not counted as asking");
    }

    #[test]
    fn the_guard_shares_the_gate_between_cmd_q_and_the_dock() {
        let guard = QuitGuard::default();
        guard.set_open_tx(true);
        // ⌘Q asked; the Dock's quit 1 s later goes through.
        assert_eq!(guard.request(T), QuitDecision::Ask);
        assert_eq!(guard.should_terminate(T + 1_000), TerminateReply::Now);
        // The Dock asked; ⌘Q 1 s later quits anyway.
        assert_eq!(guard.should_terminate(T + 10_000), TerminateReply::Cancel);
        assert_eq!(guard.request(T + 11_000), QuitDecision::Quit);
        // Confirmed (quitting): the quit's own terminate isn't held.
        guard.set_quitting();
        assert!(guard.is_quitting());
        assert_eq!(guard.should_terminate(T + 11_001), TerminateReply::Now);
        // Closed transactions: through.
        let guard = QuitGuard::default();
        assert_eq!(guard.should_terminate(T), TerminateReply::Now);
    }
}
