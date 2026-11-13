//! The error every command returns to the frontend: a message plus, for
//! server errors, the Postgres details (SQLSTATE, position, detail, hint).

use serde::Serialize;
use std::error::Error as _;
use std::fmt;

#[derive(Debug, Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct CmdError {
    pub message: String,
    /// SQLSTATE (`42P01`), or an app code such as `password_required`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    /// 1-based character offset into the statement.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub position: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
}

impl CmdError {
    pub fn msg(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            ..Self::default()
        }
    }

    pub fn code(code: &str, message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            code: Some(code.to_owned()),
            ..Self::default()
        }
    }
}

impl fmt::Display for CmdError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl From<tokio_postgres::Error> for CmdError {
    fn from(e: tokio_postgres::Error) -> Self {
        if let Some(db) = e.as_db_error() {
            return Self {
                message: db.message().to_owned(),
                code: Some(db.code().code().to_owned()),
                position: match db.position() {
                    Some(tokio_postgres::error::ErrorPosition::Original(p)) => Some(*p),
                    _ => None,
                },
                detail: db.detail().map(str::to_owned),
                hint: db.hint().map(str::to_owned),
            };
        }
        // "error connecting to server: Connection refused (os error 61)"
        let mut message = e.to_string();
        let mut source = e.source();
        while let Some(s) = source {
            let s_text = s.to_string();
            if !message.contains(&s_text) {
                message.push_str(": ");
                message.push_str(&s_text);
            }
            source = s.source();
        }
        if message.contains("password missing") {
            return Self::code("password_required", "The server asked for a password");
        }
        Self::msg(message)
    }
}

impl From<native_tls::Error> for CmdError {
    fn from(e: native_tls::Error) -> Self {
        Self::msg(format!("TLS: {e}"))
    }
}

impl From<keyring::Error> for CmdError {
    fn from(e: keyring::Error) -> Self {
        Self::msg(format!("Keychain: {e}"))
    }
}

impl From<std::io::Error> for CmdError {
    fn from(e: std::io::Error) -> Self {
        Self::msg(e.to_string())
    }
}

impl From<serde_json::Error> for CmdError {
    fn from(e: serde_json::Error) -> Self {
        Self::msg(e.to_string())
    }
}

pub type CmdResult<T> = Result<T, CmdError>;
