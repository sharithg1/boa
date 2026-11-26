//! Minisign signature checks, as `tauri-plugin-updater` does them: the
//! public key and the signature are base64 of the minisign key / signature
//! *files* (`tauri signer generate` / `tauri signer sign` write them so).

use base64::Engine as _;
use minisign_verify::{PublicKey, Signature};

use super::{AvailableUpdate, Error};

/// The Tauri app's updater public key (`plugins.updater.pubkey` in
/// `crates/boa-desktop/tauri.conf.json`, minisign key id A427C18232B67439).
/// Installed apps accept only updates signed with its private key.
pub const PRODUCTION_PUBKEY: &str = "dW50cnVzdGVkIGNvbW1lbnQ6IG1pbmlzaWduIHB1YmxpYyBrZXk6IEE0MjdDMTgyMzJCNjc0MzkKUldRNWRMWXlnc0VucEUrSVdiM2ovMlUyVzdPVFZSWmxVSGQ0d0dkcjVwcVMvNWlYaU5LeWFtVGkK";

/// The signature must name the version it was made for (`version:` in the
/// trusted comment) and that must be the announced one. Every bundle
/// `scripts/bundle.sh` and `tauri build` sign carries it; the Tauri app did
/// not require it (`requireSignedVersion` off).
pub const REQUIRE_SIGNED_VERSION: bool = true;

/// The key this build trusts: [`PRODUCTION_PUBKEY`], unless the build was made
/// with `BOA_UPDATER_PUBKEY=<base64 public key>` in the environment (local
/// update rehearsals with a throwaway key; `scripts/bundle.sh` says so loudly
/// and the release workflow refuses it).
pub fn embedded_pubkey() -> &'static str {
    match option_env!("BOA_UPDATER_PUBKEY") {
        Some(key) if !key.trim().is_empty() => key,
        _ => PRODUCTION_PUBKEY,
    }
}

/// This build trusts the production key.
pub fn pubkey_is_production() -> bool {
    embedded_pubkey() == PRODUCTION_PUBKEY
}

fn decode_text(what: &str, b64: &str) -> Result<String, Error> {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(b64.trim())
        .map_err(|e| Error::Signature(format!("{what} is not base64: {e}")))?;
    String::from_utf8(bytes).map_err(|_| Error::Signature(format!("{what} is not text")))
}

/// The `version:` field of a trusted comment (`timestamp:…\tfile:…\tversion:…`).
pub fn signed_version(trusted_comment: &str) -> Option<&str> {
    trusted_comment
        .split('\t')
        .find_map(|field| field.strip_prefix("version:"))
}

fn same_version(a: &str, b: &str) -> bool {
    use boa_core::state::updates::Version;
    match (Version::parse(a), Version::parse(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}

/// Checks `data` against `signature` (base64 of a minisign signature file)
/// and `pubkey` (base64 of a minisign public key file), then the signed
/// version against `announced_version` (the trusted comment is only read once
/// the global signature covering it has been checked).
pub fn verify_signature(
    data: &[u8],
    signature: &str,
    pubkey: &str,
    announced_version: &str,
    require_signed_version: bool,
) -> Result<(), Error> {
    let key = PublicKey::decode(&decode_text("the public key", pubkey)?)
        .map_err(|e| Error::Signature(format!("bad public key: {e}")))?;
    let sig = Signature::decode(&decode_text("the signature", signature)?)
        .map_err(|e| Error::Signature(format!("bad signature: {e}")))?;
    // Legacy (non-prehashed) signatures allowed, as the plugin does.
    key.verify(data, &sig, true)
        .map_err(|e| Error::Signature(e.to_string()))?;
    match signed_version(sig.trusted_comment()) {
        Some(signed) if !same_version(signed, announced_version) => Err(Error::SignedVersion {
            signed: signed.to_owned(),
            announced: announced_version.to_owned(),
        }),
        Some(_) => Ok(()),
        None if require_signed_version => Err(Error::MissingSignedVersion),
        None => Ok(()),
    }
}

/// A downloaded update whose signature has been checked: the only thing
/// [`super::install`] accepts.
pub struct VerifiedUpdate {
    version: String,
    bytes: Vec<u8>,
}

impl std::fmt::Debug for VerifiedUpdate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VerifiedUpdate")
            .field("version", &self.version)
            .field("bytes", &self.bytes.len())
            .finish()
    }
}

impl VerifiedUpdate {
    /// Verifies `bytes` as the download of `update` against `pubkey`.
    pub fn verify(bytes: Vec<u8>, update: &AvailableUpdate, pubkey: &str) -> Result<Self, Error> {
        verify_signature(
            &bytes,
            &update.signature,
            pubkey,
            &update.version,
            REQUIRE_SIGNED_VERSION,
        )?;
        Ok(Self {
            version: update.version.clone(),
            bytes,
        })
    }

    /// The version this update installs.
    pub fn version(&self) -> &str {
        &self.version
    }

    /// The `.app.tar.gz`.
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    #[cfg(test)]
    pub(super) fn unverified_for_tests(version: &str, bytes: Vec<u8>) -> Self {
        Self {
            version: version.to_owned(),
            bytes,
        }
    }
}
