//! Which SQL ⌘↵ runs: the selection, else the statement under the caret
//! (the Tauri app's `Workspace.tsx` `run` / `onStatement`).

use std::ops::Range;

use boa_core::sql::{Span, statements_at};

/// The statement at byte `pos` (then its subqueries there, outermost first),
/// or nothing when the caret is not on one: inside it, or on its first / last
/// line around it, but not on a blank line between statements.
pub fn statements_on(sql: &str, pos: usize) -> Vec<Span> {
    let items = statements_at(sql, pos);
    let Some(r) = items.first() else {
        return Vec::new();
    };
    let gap = if pos < r.from {
        &sql[pos..r.from]
    } else if pos > r.to {
        &sql[r.to..pos]
    } else {
        ""
    };
    if gap.contains('\n') {
        Vec::new()
    } else {
        items
    }
}

/// The whole statement under the caret at byte `pos` (what the editor
/// outlines), if any.
pub fn statement_on(sql: &str, pos: usize) -> Option<Range<usize>> {
    statements_on(sql, pos).first().map(|s| s.from..s.to)
}

/// The SQL ⌘↵ runs with `selection` (byte offsets; empty: just a caret):
/// the selected text when it isn't blank, else the statement under the caret
/// (without its `;`, like the Tauri app).
pub fn sql_to_run(sql: &str, selection: Range<usize>) -> Option<String> {
    let selected = sql.get(selection.clone()).unwrap_or_default();
    if !selected.trim().is_empty() {
        return Some(selected.to_owned());
    }
    let r = statement_on(sql, selection.start)?;
    Some(sql[r].to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCRIPT: &str = "select 1;\n\nselect *\nfrom t\nwhere x = 1;\n\n\nselect 3";

    fn caret(marked: &str) -> (String, usize) {
        let pos = marked.find('|').unwrap();
        (marked.replace('|', ""), pos)
    }

    #[test]
    fn runs_the_statement_under_the_caret() {
        let (sql, pos) = caret("select 1;\n\nselect *\nfr|om t\nwhere x = 1;\n\n\nselect 3");
        assert_eq!(sql, SCRIPT);
        assert_eq!(
            sql_to_run(&sql, pos..pos).as_deref(),
            Some("select *\nfrom t\nwhere x = 1")
        );
        // After the ';' on the statement's last line still counts.
        let end = SCRIPT.find("1;\n\n\n").unwrap() + 2;
        assert_eq!(
            sql_to_run(SCRIPT, end..end).as_deref(),
            Some("select *\nfrom t\nwhere x = 1")
        );
        assert_eq!(sql_to_run(SCRIPT, 0..0).as_deref(), Some("select 1"));
        let last = SCRIPT.len();
        assert_eq!(sql_to_run(SCRIPT, last..last).as_deref(), Some("select 3"));
    }

    #[test]
    fn nothing_on_a_blank_line_between_statements() {
        let blank = SCRIPT.find("\n\n\n").unwrap() + 2;
        assert_eq!(sql_to_run(SCRIPT, blank..blank), None);
        assert_eq!(sql_to_run("", 0..0), None);
        assert_eq!(sql_to_run("  -- only a comment\n", 3..3), None);
    }

    #[test]
    fn a_selection_overrides_the_statement() {
        let from = SCRIPT.find("from t").unwrap();
        assert_eq!(
            sql_to_run(SCRIPT, from..from + 6).as_deref(),
            Some("from t")
        );
        // A blank selection falls back to the statement at its start.
        assert_eq!(sql_to_run(SCRIPT, 9..11).as_deref(), Some("select 1"));
    }
}
