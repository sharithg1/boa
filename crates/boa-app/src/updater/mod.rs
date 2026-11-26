//! In-app updates, the I/O half (Phase 8): fetch `latest.json`, download the
//! update, verify its minisign signature, swap the running `.app` and
//! relaunch. The pure half (manifest parsing, version ordering, picking this
//! Mac's download, the check → download → ready state machine and the texts)
//! is [`boa_core::state::updates`]; the GPUI model that drives both and shows
//! "Restart to update" is `models::updates`.
//!
//! Everything here is **blocking** (network and file work): call it from
//! `cx.background_spawn` or a thread, never on the main thread.
//!
//! The flow is the one `tauri-plugin-updater` 2.x (the Tauri app's updater)
//! implements, so the same release feed serves both apps:
//!
//! 1. [`check`]: GET the endpoint ([`endpoint`]: `BOA_UPDATE_ENDPOINT` or
//!    [`UPDATE_ENDPOINT`]) and pick the update for this Mac
//!    ([`boa_core::state::updates::select_update`]).
//! 2. [`download`]: GET the `.app.tar.gz` and verify it ([`verify_signature`])
//!    against [`embedded_pubkey`] (the Tauri app's `plugins.updater.pubkey`).
//!    Only a verified download becomes a [`VerifiedUpdate`].
//! 3. [`install`]: unpack it next to the running bundle ([`current_bundle`]) and
//!    swap the two directories (`renamex_np(RENAME_SWAP)`, or two renames where
//!    the file system can't swap).
//! 4. [`schedule_relaunch`] then quit: the new bundle starts once this process
//!    has exited (`open -n`), so session.json is flushed before it is read.
//!
//! Deviations from the plugin: plain `http` is accepted only for loopback
//! hosts (local rehearsals); the signed version (`version:` in the trusted
//! comment, written by `tauri signer sign --app-version` and `tauri build`)
//! is required ([`REQUIRE_SIGNED_VERSION`]); and there is no "administrator
//! privileges" fallback: a bundle the user can't write is reported
//! ([`Error::PermissionDenied`]) instead.

mod fetch;
mod install;
mod verify;

#[cfg(feature = "dev-hooks")]
pub mod dev;

#[cfg(test)]
pub(crate) mod tests;

use std::fmt;
use std::path::PathBuf;

pub use boa_core::state::updates::{AvailableUpdate, UPDATE_ENDPOINT};
pub use fetch::{
    ENDPOINT_ENV, check, current_arch, download, endpoint, fetch_manifest, resolve_endpoint,
};
pub use install::{bundle_of_executable, current_bundle, install, schedule_relaunch, unpack};
pub use verify::{
    PRODUCTION_PUBKEY, REQUIRE_SIGNED_VERSION, VerifiedUpdate, embedded_pubkey,
    pubkey_is_production, signed_version, verify_signature,
};

/// The running app's version (the workspace version).
pub fn current_version() -> &'static str {
    crate::app::VERSION
}

/// Why a check, download or install failed. `Display` is the message the
/// About window / toast shows.
#[derive(Debug)]
pub enum Error {
    /// The request failed (DNS, TLS, connection, timeout…).
    Http(String),
    /// The server answered with an error status.
    Status {
        url: String,
        status: u16,
    },
    /// An `http://` URL to a host other than this Mac.
    InsecureUrl(String),
    /// `latest.json` is unreadable, or has no download for this Mac.
    Manifest(boa_core::state::updates::UpdateError),
    /// The public key or signature can't be decoded, or the signature does
    /// not match the download.
    Signature(String),
    /// The signature is for another version than the one announced.
    SignedVersion {
        signed: String,
        announced: String,
    },
    /// The signature carries no version and one is required.
    MissingSignedVersion,
    /// The running executable is not inside an `.app` bundle (e.g. `cargo run`).
    NotInBundle(PathBuf),
    /// The download is not an `.app` archive the updater can install.
    Archive(String),
    /// The bundle's folder can't be written by this user.
    PermissionDenied(PathBuf),
    Io(std::io::Error),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Http(e) => write!(f, "{e}"),
            Error::Status { url, status } => write!(f, "{url} answered HTTP {status}"),
            Error::InsecureUrl(url) => write!(f, "refusing to update over plain HTTP: {url}"),
            Error::Manifest(e) => write!(f, "{e}"),
            Error::Signature(e) => write!(f, "the update's signature is not valid: {e}"),
            Error::SignedVersion { signed, announced } => write!(
                f,
                "the update is signed for version {signed}, but {announced} was announced"
            ),
            Error::MissingSignedVersion => {
                f.write_str("the update's signature does not name its version")
            }
            Error::NotInBundle(exe) => write!(
                f,
                "Boa is not running from an app bundle ({}), so it can't update itself",
                exe.display()
            ),
            Error::Archive(e) => write!(f, "the update archive is not valid: {e}"),
            Error::PermissionDenied(app) => write!(
                f,
                "Boa can't replace {} (permission denied). Download the new version and \
                 install it by hand",
                app.display()
            ),
            Error::Io(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error::Io(e)
    }
}

impl From<boa_core::state::updates::UpdateError> for Error {
    fn from(e: boa_core::state::updates::UpdateError) -> Self {
        Error::Manifest(e)
    }
}
