//! Saved connections: their settings in `<app config dir>/connections.json`,
//! their passwords in the OS keychain (never in the file).

use crate::error::CmdResult;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

const KEYCHAIN_SERVICE: &str = "dev.dbclient.desktop";

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "kebab-case")]
pub enum SslMode {
    Disable,
    /// TLS when the server offers it; certificates are not verified (libpq `prefer`).
    #[default]
    Prefer,
    /// TLS required; certificates are not verified (libpq `require`).
    Require,
    /// TLS required, certificate chain and host name verified.
    VerifyFull,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnConfig {
    pub id: String,
    pub name: String,
    /// Host name, IP, or a Unix socket directory (starting with `/`).
    pub host: String,
    pub port: u16,
    pub user: String,
    /// Database to connect to first (and to list the others from).
    pub database: String,
    #[serde(default)]
    pub ssl_mode: SslMode,
}

/// A connection as the explorer lists it.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SavedConnection {
    #[serde(flatten)]
    pub config: ConnConfig,
    pub has_password: bool,
}

fn file(dir: &Path) -> PathBuf {
    dir.join("connections.json")
}

pub fn load(dir: &Path) -> CmdResult<Vec<ConnConfig>> {
    match std::fs::read(file(dir)) {
        Ok(bytes) => Ok(serde_json::from_slice(&bytes)?),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(e.into()),
    }
}

fn store(dir: &Path, list: &[ConnConfig]) -> CmdResult<()> {
    std::fs::create_dir_all(dir)?;
    // Write then rename, so a crash never leaves a half-written file.
    let tmp = dir.join("connections.json.tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(list)?)?;
    std::fs::rename(tmp, file(dir))?;
    Ok(())
}

/// Adds or replaces (by id) a connection.
pub fn upsert(dir: &Path, config: ConnConfig) -> CmdResult<()> {
    let mut list = load(dir)?;
    match list.iter_mut().find(|c| c.id == config.id) {
        Some(slot) => *slot = config,
        None => list.push(config),
    }
    store(dir, &list)
}

pub fn remove(dir: &Path, id: &str) -> CmdResult<()> {
    let mut list = load(dir)?;
    list.retain(|c| c.id != id);
    store(dir, &list)?;
    delete_password(id)
}

pub fn get(dir: &Path, id: &str) -> CmdResult<Option<ConnConfig>> {
    Ok(load(dir)?.into_iter().find(|c| c.id == id))
}

fn entry(id: &str) -> CmdResult<keyring::Entry> {
    Ok(keyring::Entry::new(KEYCHAIN_SERVICE, id)?)
}

pub fn password(id: &str) -> CmdResult<Option<String>> {
    match entry(id)?.get_password() {
        Ok(p) => Ok(Some(p)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(e.into()),
    }
}

pub fn set_password(id: &str, password: &str) -> CmdResult<()> {
    Ok(entry(id)?.set_password(password)?)
}

pub fn delete_password(id: &str) -> CmdResult<()> {
    match entry(id)?.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(e.into()),
    }
}
