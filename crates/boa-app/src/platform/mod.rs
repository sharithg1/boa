//! Platform signals the reconnect loop listens to (`models::connections`):
//! the Mac woke from sleep, or the network changed (HANDOFF-PARITY.md §9
//! Phase 2: "retry on wake and network change").
//!
//! The Tauri app relied on the WebView for these (a 5 s timer that notices
//! it ran late after sleep, and `window.online`). The timer is kept
//! ([`boa_core::state::reconnect::woke_up`], in the model); this module adds
//! the native notifications, which arrive at once:
//!
//! - **did wake**: `NSWorkspaceDidWakeNotification` on NSWorkspace's
//!   notification centre (objc2 / objc2-app-kit / block2);
//! - **network change**: a SystemConfiguration dynamic-store watch of
//!   `State:/Network/Global/IPv4` and `.../IPv6` (the primary interface
//!   and its addresses), via the `system-configuration` crate.
//!
//! Both need a few `unsafe` calls (Objective-C messages, two extern
//! statics), so they live in [`macos`], the one module of the app with a
//! scoped `#[allow(unsafe_code)]` (approved by the user; see its docs for
//! each call's safety argument). Other platforms get a monitor that never
//! fires.
//!
//! [`terminate`] holds Dock → Quit / logout / restart while a transaction
//! is open (AppKit's `applicationShouldTerminate:`, added to GPUI's app
//! delegate; the only other `unsafe`, also approved by the user).
//!
//! Events are delivered on the main thread (the main operation queue and
//! the main run loop) into an unbounded channel; the model awaits it.

use futures::channel::mpsc;

#[cfg(target_os = "macos")]
mod macos;
pub mod terminate;

/// Something that should make the app check its connections now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlatformEvent {
    /// The machine woke from sleep.
    Woke,
    /// The primary network interface or its addresses changed.
    NetworkChanged,
}

/// Keeps the observers registered; dropping it removes them.
#[derive(Debug)]
pub struct Monitor {
    #[cfg(target_os = "macos")]
    _inner: Option<macos::Monitor>,
    #[cfg(not(target_os = "macos"))]
    _tx: mpsc::UnboundedSender<PlatformEvent>,
}

/// Starts watching for wake-ups and network changes. Call on the main
/// thread (the observers deliver there). Events arrive on the receiver
/// until the [`Monitor`] is dropped.
pub fn watch() -> (Monitor, mpsc::UnboundedReceiver<PlatformEvent>) {
    let (tx, rx) = mpsc::unbounded();
    #[cfg(target_os = "macos")]
    let monitor = Monitor {
        _inner: macos::Monitor::start(tx),
    };
    #[cfg(not(target_os = "macos"))]
    let monitor = Monitor { _tx: tx };
    (monitor, rx)
}
