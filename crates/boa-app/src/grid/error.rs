//! A failed statement as the results pane shows it: `[SQLSTATE] message`, the
//! Detail / Hint lines (store.ts `errorText`) and, when the server gave a
//! position, the statement's line with a caret under it (as psql does).

use boa_core::error::CmdError;

/// A failed run, ready to draw.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunError {
    /// `[42P01] relation "x" does not exist`, or the message alone for app errors.
    pub message: String,
    /// `Detail: …` and `Hint: …` lines.
    pub detail: Option<String>,
    /// `LINE 2: select * from nosuch` and the caret line under it.
    pub excerpt: Option<(String, String)>,
}

impl RunError {
    /// An error that isn't the server's (not connected, …).
    pub fn plain(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            detail: None,
            excerpt: None,
        }
    }

    pub fn from_cmd(err: &CmdError, sql: &str) -> Self {
        let message = match err.code.as_deref() {
            Some(code) if is_sqlstate(code) => format!("[{code}] {}", err.message),
            _ => err.message.clone(),
        };
        let detail = [
            err.detail.as_ref().map(|d| format!("Detail: {d}")),
            err.hint.as_ref().map(|h| format!("Hint: {h}")),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join("\n");
        Self {
            message,
            detail: (!detail.is_empty()).then_some(detail),
            excerpt: err.position.and_then(|p| excerpt(sql, p as usize)),
        }
    }
}

/// Five digits / capitals: a SQLSTATE, not an app code like `not_connected`.
fn is_sqlstate(code: &str) -> bool {
    code.len() == 5
        && code
            .bytes()
            .all(|b| b.is_ascii_digit() || b.is_ascii_uppercase())
}

/// The line holding 1-based character `position` of `sql`, psql-style.
fn excerpt(sql: &str, position: usize) -> Option<(String, String)> {
    let index = position.checked_sub(1)?;
    let mut line_no = 1;
    let mut line_start = 0; // in chars
    let mut found = false;
    for (i, ch) in sql.chars().enumerate() {
        if i == index {
            found = true;
            break;
        }
        if ch == '\n' {
            line_no += 1;
            line_start = i + 1;
        }
    }
    if !found && index != sql.chars().count() {
        return None;
    }
    let line: String = sql
        .chars()
        .skip(line_start)
        .take_while(|&c| c != '\n')
        .map(|c| if c == '\t' { ' ' } else { c })
        .collect();
    let label = format!("LINE {line_no}: ");
    let caret = format!(
        "{}^",
        " ".repeat(label.chars().count() + index - line_start)
    );
    Some((format!("{label}{}", line.trim_end_matches('\r')), caret))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn server_errors_carry_code_detail_hint_and_position() {
        let err = CmdError {
            message: "relation \"nosuch\" does not exist".into(),
            code: Some("42P01".into()),
            position: Some(15),
            detail: Some("d".into()),
            hint: Some("h".into()),
            ..CmdError::default()
        };
        let e = RunError::from_cmd(&err, "select * from nosuch");
        assert_eq!(e.message, "[42P01] relation \"nosuch\" does not exist");
        assert_eq!(e.detail.as_deref(), Some("Detail: d\nHint: h"));
        let (line, caret) = e.excerpt.unwrap();
        assert_eq!(line, "LINE 1: select * from nosuch");
        assert_eq!(caret, format!("{}^", " ".repeat(8 + 14)));
    }

    #[test]
    fn app_codes_are_not_shown() {
        let err = CmdError::code("not_connected", "Not connected");
        let e = RunError::from_cmd(&err, "select 1");
        assert_eq!(e.message, "Not connected");
        assert_eq!(e.detail, None);
        assert_eq!(e.excerpt, None);
    }

    #[test]
    fn excerpt_finds_the_line() {
        let sql = "select 1;\nselect é from\nwhere";
        // 'é' is char 18 (1-based): line 2, column 8.
        let (line, caret) = excerpt(sql, 18).unwrap();
        assert_eq!(line, "LINE 2: select é from");
        assert_eq!(caret.chars().count(), "LINE 2: ".len() + 7 + 1);
        assert!(excerpt(sql, 0).is_none());
        assert!(excerpt(sql, 500).is_none());
    }
}
