//! Saved connections: their settings and (for now) their passwords in
//! `<app config dir>/connections.json`. The file is written owner-only (0600)
//! on Unix, but passwords are stored in plain text.

use crate::error::CmdResult;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

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
    /// Sessions run with `default_transaction_read_only = on`.
    #[serde(default)]
    pub read_only: bool,
    /// Environment tag; `production` always confirms destructive statements.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub environment: Option<Environment>,
    /// Colour shown wherever the connection is used (a palette name such as
    /// `red`); unset: the environment's colour.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum Environment {
    Development,
    Test,
    Staging,
    Production,
}

/// A connection as the explorer lists it.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SavedConnection {
    #[serde(flatten)]
    pub config: ConnConfig,
    pub has_password: bool,
}

/// A connection as stored in the file.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Stored {
    #[serde(flatten)]
    config: ConnConfig,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    password: Option<String>,
}

fn file(dir: &Path) -> PathBuf {
    dir.join("connections.json")
}

fn load(dir: &Path) -> CmdResult<Vec<Stored>> {
    match std::fs::read(file(dir)) {
        Ok(bytes) => Ok(serde_json::from_slice(&bytes)?),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(e.into()),
    }
}

fn store(dir: &Path, list: &[Stored]) -> CmdResult<()> {
    std::fs::create_dir_all(dir)?;
    // Write then rename, so a crash never leaves a half-written file.
    let tmp = dir.join("connections.json.tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(list)?)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600))?;
    }
    std::fs::rename(tmp, file(dir))?;
    Ok(())
}

pub fn list(dir: &Path) -> CmdResult<Vec<SavedConnection>> {
    Ok(load(dir)?
        .into_iter()
        .map(|s| SavedConnection {
            has_password: s.password.is_some(),
            config: s.config,
        })
        .collect())
}

/// Adds or replaces (by id) a connection. `password`: `Some` stores it (when
/// `save_password`), `None` keeps the stored one; `save_password: false`
/// forgets it.
pub fn upsert(
    dir: &Path,
    config: ConnConfig,
    password: Option<String>,
    save_password: bool,
) -> CmdResult<()> {
    let mut list = load(dir)?;
    let idx = match list.iter().position(|s| s.config.id == config.id) {
        Some(i) => i,
        None => {
            list.push(Stored {
                config: config.clone(),
                password: None,
            });
            list.len() - 1
        }
    };
    let slot = &mut list[idx];
    slot.config = config;
    if !save_password {
        slot.password = None;
    } else if password.is_some() {
        slot.password = password;
    }
    store(dir, &list)
}

pub fn remove(dir: &Path, id: &str) -> CmdResult<()> {
    let mut list = load(dir)?;
    list.retain(|s| s.config.id != id);
    store(dir, &list)
}

/// A connection's settings and stored password.
pub fn get(dir: &Path, id: &str) -> CmdResult<Option<(ConnConfig, Option<String>)>> {
    Ok(load(dir)?
        .into_iter()
        .find(|s| s.config.id == id)
        .map(|s| (s.config, s.password)))
}

pub fn password(dir: &Path, id: &str) -> CmdResult<Option<String>> {
    Ok(get(dir, id)?.and_then(|(_, p)| p))
}

#[cfg(test)]
mod tests {
    use super::{Environment, SslMode, Stored, get, list, upsert};

    #[test]
    fn old_files_load_and_new_fields_round_trip() {
        // A file written before read-only / environment / colour existed.
        let old: Vec<Stored> = serde_json::from_str(
            r#"[{"id":"a","name":"A","host":"h","port":5432,"user":"u","database":"d","password":"p"}]"#,
        )
        .unwrap();
        let c = &old[0].config;
        assert!(!c.read_only);
        assert_eq!(c.environment, None);
        assert_eq!(c.color, None);
        assert_eq!(c.ssl_mode, SslMode::Prefer);

        let dir = std::env::temp_dir().join(format!("boa-conn-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut config = c.clone();
        config.read_only = true;
        config.environment = Some(Environment::Production);
        config.color = Some("red".into());
        upsert(&dir, config, Some("p".into()), true).unwrap();
        let (back, pw) = get(&dir, "a").unwrap().unwrap();
        assert!(back.read_only);
        assert_eq!(back.environment, Some(Environment::Production));
        assert_eq!(back.color.as_deref(), Some("red"));
        assert_eq!(pw.as_deref(), Some("p"));
        let json = String::from_utf8(std::fs::read(dir.join("connections.json")).unwrap()).unwrap();
        assert!(
            json.contains("\"readOnly\": true") && json.contains("\"environment\": \"production\"")
        );
        assert!(list(&dir).unwrap()[0].has_password);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
