//! The network half: `latest.json` and the download (blocking `reqwest` with
//! rustls + the macOS trust store, the TLS stack `tauri-plugin-updater` used).

use std::io::Read as _;
use std::time::Duration;

use boa_core::state::updates::{ReleaseManifest, select_update};
use reqwest::Url;
use reqwest::blocking::Client;
use reqwest::header::ACCEPT;

use super::{AvailableUpdate, Error, UPDATE_ENDPOINT, VerifiedUpdate};

/// Set to another `latest.json` URL to check there instead (local rehearsals,
/// staging feeds). The download is still verified against the embedded key.
pub const ENDPOINT_ENV: &str = "BOA_UPDATE_ENDPOINT";

const MANIFEST_TIMEOUT: Duration = Duration::from_secs(60);
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(15 * 60);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);
/// Refuse anything bigger than this (a release bundle is ~10–30 MB).
const MAX_DOWNLOAD: u64 = 512 * 1024 * 1024;

/// The feed to check: `$BOA_UPDATE_ENDPOINT`, or the GitHub release feed.
pub fn endpoint() -> String {
    std::env::var(ENDPOINT_ENV)
        .ok()
        .map(|v| v.trim().to_owned())
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| UPDATE_ENDPOINT.to_owned())
}

/// The architecture name the feed uses (`aarch64` / `x86_64`).
pub fn current_arch() -> &'static str {
    std::env::consts::ARCH
}

/// Fills the plugin's URL variables: `{{current_version}}`, `{{target}}`
/// (`darwin`), `{{arch}}` and `{{bundle_type}}` (`app`), raw or URL-encoded.
pub fn resolve_endpoint(template: &str, current_version: &str, arch: &str) -> String {
    let mut url = template.to_owned();
    for (name, value) in [
        ("current_version", current_version),
        ("target", "darwin"),
        ("arch", arch),
        ("bundle_type", "app"),
    ] {
        url = url
            .replace(&format!("{{{{{name}}}}}"), value)
            .replace(&format!("%7B%7B{name}%7D%7D"), value);
    }
    url
}

/// `https`, or `http` to this Mac (rehearsals with a local server).
pub(super) fn checked_url(url: &str) -> Result<Url, Error> {
    let parsed = Url::parse(url).map_err(|e| Error::Http(format!("bad URL {url}: {e}")))?;
    let loopback = is_loopback(&parsed);
    match parsed.scheme() {
        "https" => Ok(parsed),
        "http" if loopback => Ok(parsed),
        _ => Err(Error::InsecureUrl(url.to_owned())),
    }
}

fn is_loopback(url: &Url) -> bool {
    let host = url.host_str().unwrap_or_default();
    let host = host.trim_start_matches('[').trim_end_matches(']');
    host.eq_ignore_ascii_case("localhost")
        || host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback())
}

fn client(url: &Url, timeout: Duration) -> Result<Client, Error> {
    // rustls needs a process-wide crypto provider (reqwest's
    // rustls-no-provider); ring, as the plugin installs it.
    if rustls::crypto::CryptoProvider::get_default().is_none() {
        let _ = rustls::crypto::ring::default_provider().install_default();
    }
    let mut builder = Client::builder()
        .user_agent(concat!("Boa/", env!("CARGO_PKG_VERSION"), " (updater)"))
        .connect_timeout(CONNECT_TIMEOUT)
        .timeout(timeout);
    if is_loopback(url) {
        // The system proxy settings are for the outside world.
        builder = builder.no_proxy();
    }
    builder.build().map_err(|e| Error::Http(e.to_string()))
}

fn http_error(e: reqwest::Error) -> Error {
    // reqwest's Display hides the cause ("error sending request for url");
    // the chain says what happened.
    let mut msg = e.to_string();
    let mut source = std::error::Error::source(&e);
    while let Some(s) = source {
        msg.push_str(": ");
        msg.push_str(&s.to_string());
        source = s.source();
    }
    Error::Http(msg)
}

/// GETs and parses `latest.json`. `Ok(None)`: the server answered
/// `204 No Content` (the plugin's "no update").
pub fn fetch_manifest(url: &str) -> Result<Option<ReleaseManifest>, Error> {
    let parsed = checked_url(url)?;
    let resp = client(&parsed, MANIFEST_TIMEOUT)?
        .get(parsed)
        .header(ACCEPT, "application/json")
        .send()
        .map_err(http_error)?;
    let status = resp.status();
    if status == reqwest::StatusCode::NO_CONTENT {
        return Ok(None);
    }
    if !status.is_success() {
        return Err(Error::Status {
            url: url.to_owned(),
            status: status.as_u16(),
        });
    }
    let text = resp.text().map_err(http_error)?;
    Ok(Some(ReleaseManifest::from_json(&text)?))
}

/// Checks `endpoint` (a URL template, see [`resolve_endpoint`]) for a version
/// newer than `current_version` with a download for `arch`.
pub fn check(
    endpoint: &str,
    current_version: &str,
    arch: &str,
) -> Result<Option<AvailableUpdate>, Error> {
    let url = resolve_endpoint(endpoint, current_version, arch);
    let Some(manifest) = fetch_manifest(&url)? else {
        return Ok(None);
    };
    let update = select_update(&manifest, current_version, arch)?;
    if let Some(u) = &update {
        // Fail at check time, not after the download.
        checked_url(&u.url)?;
    }
    Ok(update)
}

/// Downloads `update` and verifies it against `pubkey` (normally
/// [`super::embedded_pubkey`]). `progress(received, total)` is called after
/// every chunk (`total`: the Content-Length, when sent).
pub fn download(
    update: &AvailableUpdate,
    pubkey: &str,
    mut progress: impl FnMut(u64, Option<u64>),
) -> Result<VerifiedUpdate, Error> {
    let url = checked_url(&update.url)?;
    let mut resp = client(&url, DOWNLOAD_TIMEOUT)?
        .get(url)
        .header(ACCEPT, "application/octet-stream")
        .send()
        .map_err(http_error)?;
    let status = resp.status();
    if !status.is_success() {
        return Err(Error::Status {
            url: update.url.clone(),
            status: status.as_u16(),
        });
    }
    let total = resp.content_length();
    if total.is_some_and(|t| t > MAX_DOWNLOAD) {
        return Err(Error::Archive(format!(
            "{} bytes is too big",
            total.unwrap_or(0)
        )));
    }
    let mut bytes = Vec::with_capacity(total.unwrap_or(0).min(MAX_DOWNLOAD) as usize);
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = resp
            .read(&mut buf)
            .map_err(|e| Error::Http(e.to_string()))?;
        if n == 0 {
            break;
        }
        bytes.extend_from_slice(&buf[..n]);
        if bytes.len() as u64 > MAX_DOWNLOAD {
            return Err(Error::Archive("the download is too big".into()));
        }
        progress(bytes.len() as u64, total);
    }
    VerifiedUpdate::verify(bytes, update, pubkey)
}
