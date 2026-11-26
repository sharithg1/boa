//! The macOS observers behind [`super::watch`]. **The only module of the app
//! allowed `unsafe`** (scoped `#[allow(unsafe_code)]`, approved by the user
//! for exactly this: sleep/wake and network-change notifications). Keep it
//! small; each `unsafe` block says why it is sound.

#![allow(unsafe_code)]

use std::ptr::NonNull;

use block2::RcBlock;
use core_foundation::array::CFArray;
use core_foundation::runloop::{CFRunLoop, CFRunLoopSource, kCFRunLoopCommonModes};
use core_foundation::string::CFString;
use futures::channel::mpsc::UnboundedSender;
use objc2::rc::Retained;
use objc2::runtime::{NSObjectProtocol, ProtocolObject};
use objc2_app_kit::{NSWorkspace, NSWorkspaceDidWakeNotification};
use objc2_foundation::{NSNotification, NSOperationQueue};
use system_configuration::dynamic_store::{
    SCDynamicStore, SCDynamicStoreBuilder, SCDynamicStoreCallBackContext,
};

use super::PlatformEvent;

/// Dynamic-store keys whose change means the network changed: the primary
/// IPv4 / IPv6 service (set when an interface comes up, removed when the
/// last one goes down, rewritten when the primary interface changes).
const NETWORK_KEYS: [&str; 2] = ["State:/Network/Global/IPv4", "State:/Network/Global/IPv6"];

/// The registered observers.
pub struct Monitor {
    wake: Option<Retained<ProtocolObject<dyn NSObjectProtocol>>>,
    network: Option<(SCDynamicStore, CFRunLoopSource)>,
}

impl std::fmt::Debug for Monitor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Monitor")
            .field("wake", &self.wake.is_some())
            .field("network", &self.network.is_some())
            .finish()
    }
}

impl Monitor {
    /// Registers both observers; None if neither could be (logged).
    pub fn start(tx: UnboundedSender<PlatformEvent>) -> Option<Self> {
        let monitor = Self {
            wake: watch_wake(tx.clone()),
            network: watch_network(tx),
        };
        if monitor.wake.is_none() && monitor.network.is_none() {
            tracing::warn!("no wake / network notifications: relying on the wake timer");
            return None;
        }
        Some(monitor)
    }
}

impl Drop for Monitor {
    fn drop(&mut self) {
        if let Some(observer) = self.wake.take() {
            let center = NSWorkspace::sharedWorkspace().notificationCenter();
            // SAFETY: `observer` is the token `addObserverForName…` returned
            // on this same centre, so it is "of the correct type".
            unsafe { center.removeObserver(observer.as_ref()) };
        }
        if let Some((_store, source)) = self.network.take() {
            // SAFETY: reading an immutable CoreFoundation constant.
            let mode = unsafe { kCFRunLoopCommonModes };
            CFRunLoop::get_main().remove_source(&source, mode);
        }
    }
}

/// NSWorkspace's did-wake notification, on the main queue.
fn watch_wake(
    tx: UnboundedSender<PlatformEvent>,
) -> Option<Retained<ProtocolObject<dyn NSObjectProtocol>>> {
    let center = NSWorkspace::sharedWorkspace().notificationCenter();
    let block = RcBlock::new(move |_: NonNull<NSNotification>| {
        let _ = tx.unbounded_send(PlatformEvent::Woke);
    });
    let queue = NSOperationQueue::mainQueue();
    // SAFETY: `NSWorkspaceDidWakeNotification` is an immutable NSString
    // constant AppKit exports; reading it is a plain load.
    let name = unsafe { NSWorkspaceDidWakeNotification };
    // SAFETY: `obj` is None (any sender); the queue is the main operation
    // queue, which has no extra threading requirements for the caller; the
    // block only sends on an `UnboundedSender`, which is `Send + Sync`, so
    // the block is sendable as the method requires.
    let observer = unsafe {
        center.addObserverForName_object_queue_usingBlock(Some(name), None, Some(&queue), &block)
    };
    Some(observer)
}

fn on_network_change(
    _store: SCDynamicStore,
    _changed: CFArray<CFString>,
    tx: &mut UnboundedSender<PlatformEvent>,
) {
    let _ = tx.unbounded_send(PlatformEvent::NetworkChanged);
}

/// A dynamic-store watch of [`NETWORK_KEYS`], on the main run loop.
fn watch_network(tx: UnboundedSender<PlatformEvent>) -> Option<(SCDynamicStore, CFRunLoopSource)> {
    let store = SCDynamicStoreBuilder::new("dev.boa.desktop.network")
        .callback_context(SCDynamicStoreCallBackContext {
            callout: on_network_change,
            info: tx,
        })
        .build();
    let keys: Vec<CFString> = NETWORK_KEYS.iter().map(|k| CFString::new(k)).collect();
    let keys = CFArray::from_CFTypes(&keys);
    let patterns: CFArray<CFString> = CFArray::from_CFTypes(&[]);
    if !store.set_notification_keys(&keys, &patterns) {
        tracing::warn!("could not watch network changes (SCDynamicStoreSetNotificationKeys)");
        return None;
    }
    let source = store.create_run_loop_source();
    // SAFETY: reading an immutable CoreFoundation constant.
    let mode = unsafe { kCFRunLoopCommonModes };
    CFRunLoop::get_main().add_source(&source, mode);
    Some((store, source))
}
