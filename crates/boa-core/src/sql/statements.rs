//! Statements in a console script, DataGrip-style: the script splits on `;`
//! outside strings, quoted names, comments and dollar quotes; each
//! statement's range leaves out the blank lines and comments around it.
//! Parenthesised queries (`(SELECT …)`, `(WITH …)`, `(VALUES …)`) inside a
//! statement are recorded too, so ⌘↵ can offer "the whole statement or this
//! subquery". All offsets are bytes.

use super::chars::{char_at, char_before, is_space, skip_space};
use super::tokenize::dollar_tag;

/// A byte range of the script.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Span {
    pub from: usize,
    pub to: usize,
}

impl Span {
    /// The text of the span in `src`.
    pub fn text<'a>(&self, src: &'a str) -> &'a str {
        &src[self.from..self.to]
    }

    fn len(&self) -> usize {
        self.to - self.from
    }
}

/// A statement of the script and the parenthesised queries in it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Statement {
    pub from: usize,
    pub to: usize,
    /// `(SELECT …)` / `(WITH …)` / `(VALUES …)` / `(TABLE …)` interiors, innermost closing first.
    pub subqueries: Vec<Span>,
}

impl Statement {
    /// The statement's range.
    pub fn span(&self) -> Span {
        Span {
            from: self.from,
            to: self.to,
        }
    }
}

/// `/^(select|with|values|table)\b/i` at byte `k`.
fn query_start(sql: &str, k: usize) -> bool {
    ["select", "with", "values", "table"].iter().any(|w| {
        sql.get(k..k + w.len())
            .is_some_and(|s| s.eq_ignore_ascii_case(w))
            && !sql
                .as_bytes()
                .get(k + w.len())
                .is_some_and(|c| c.is_ascii_alphanumeric() || *c == b'_')
    })
}

/// Splits `sql` into statements (empty ones, like a lone `;`, are skipped).
pub fn split_statements(sql: &str) -> Vec<Statement> {
    struct Paren {
        at: usize,
        query: bool,
    }
    let b = sql.as_bytes();
    let n = b.len();
    let at = |k: usize| b.get(k).copied().unwrap_or(0);
    let mut out = Vec::new();
    // First significant byte of the current statement, end of its last significant token.
    let mut first: Option<usize> = None;
    let mut last = 0;
    let mut subs: Vec<Span> = Vec::new();
    // Open parens: where the interior starts and whether it is a query.
    let mut parens: Vec<Paren> = Vec::new();
    fn mark(first: &mut Option<usize>, last: &mut usize, a: usize, z: usize) {
        first.get_or_insert(a);
        *last = z;
    }

    let mut i = 0;
    while i < n {
        let c = b[i];
        let next = at(i + 1);
        if c == b'-' && next == b'-' {
            i = b[i..].iter().position(|&x| x == b'\n').map_or(n, |p| i + p);
        } else if c == b'/' && next == b'*' {
            i = sql[i + 2..].find("*/").map_or(n, |e| i + 2 + e + 2);
        } else if c == b'\'' || c == b'"' || c == b'`' {
            let mut j = i + 1;
            while j < n {
                if b[j] == c {
                    if at(j + 1) == c {
                        j += 2;
                    } else {
                        j += 1;
                        break;
                    }
                } else {
                    j += 1;
                }
            }
            mark(&mut first, &mut last, i, j);
            i = j;
        } else if let Some(tag_end) = (c == b'$').then(|| dollar_tag(b, i)).flatten() {
            let tag = &sql[i..tag_end];
            let j = sql[tag_end..]
                .find(tag)
                .map_or(n, |e| tag_end + e + tag.len());
            mark(&mut first, &mut last, i, j);
            i = j;
        } else if c == b';' {
            if let Some(from) = first.take() {
                out.push(Statement {
                    from,
                    to: last,
                    subqueries: std::mem::take(&mut subs),
                });
            }
            subs.clear();
            parens.clear();
            i += 1;
        } else {
            let ch = char_at(sql, i).unwrap_or('\0');
            let len = ch.len_utf8();
            if is_space(ch) {
                i += len;
                continue;
            }
            if c == b'(' {
                let k = skip_space(sql, i + 1);
                parens.push(Paren {
                    at: k,
                    query: query_start(sql, k),
                });
            } else if c == b')'
                && let Some(p) = parens.pop()
                && p.query
            {
                let mut e = i;
                while e > p.at
                    && let Some(d) = char_before(sql, e)
                    && is_space(d)
                {
                    e -= d.len_utf8();
                }
                subs.push(Span { from: p.at, to: e });
            }
            mark(&mut first, &mut last, i, i + len);
            i += len;
        }
    }
    if let Some(from) = first {
        out.push(Statement {
            from,
            to: last,
            subqueries: subs,
        });
    }
    out
}

/// What ⌘↵ can run at byte `pos`: the statement there, then any subqueries
/// around the caret from the outermost in. A caret on a statement's own line
/// after its `;` still counts as that statement; past the last statement, the
/// last one runs. Empty when the script has no statement.
pub fn statements_at(sql: &str, pos: usize) -> Vec<Span> {
    let all = split_statements(sql);
    let has_newline =
        |a: usize, z: usize| a < z && sql.as_bytes()[a..z.min(sql.len())].contains(&b'\n');
    let found = all.iter().enumerate().find(|(k, s)| {
        let prev_end = if *k > 0 { Some(all[k - 1].to) } else { None };
        let next_start = all.get(k + 1).map_or(usize::MAX, |n| n.from);
        if pos >= s.from && pos <= s.to {
            return true;
        }
        // Between statements: belongs to this one if it is on its last line
        // (after the `;`), or anywhere after the previous one's line.
        if pos > s.to && pos < next_start {
            return !has_newline(s.to, pos);
        }
        if pos < s.from && prev_end.is_none_or(|p| pos > p) {
            return prev_end.is_none_or(|p| has_newline(p, pos));
        }
        false
    });
    let Some(stmt) = found.map(|(_, s)| s).or(all.last()) else {
        return Vec::new();
    };
    let mut subs: Vec<Span> = stmt
        .subqueries
        .iter()
        .copied()
        .filter(|r| pos + 1 >= r.from && pos <= r.to + 1)
        .collect();
    subs.sort_by_key(|r| std::cmp::Reverse(r.len()));
    let mut out = vec![stmt.span()];
    out.extend(subs);
    out
}

/// Monospace layout metrics for [`range_outline`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TextMetrics {
    pub char_width: f32,
    pub line_height: f32,
    pub pad_left: f32,
    pub pad_top: f32,
    pub tab_size: usize,
}

/// Outline of `range` as laid out in a monospace editor: one box per line,
/// joined into a single stepped polygon (points in order, not closed), in
/// content coordinates (not adjusted for scrolling). Empty when the range
/// covers no line.
pub fn range_outline(sql: &str, range: Span, m: &TextMetrics) -> Vec<(f32, f32)> {
    let col = |text: &str| {
        let mut c = 0;
        for ch in text.chars() {
            c = if ch == '\t' {
                (c / m.tab_size.max(1) + 1) * m.tab_size.max(1)
            } else {
                c + 1
            };
        }
        c
    };
    struct Row {
        x0: usize,
        x1: usize,
        y: usize,
    }
    let (from, to) = (range.from, range.to);
    let mut offset = 0;
    let mut rows = Vec::new();
    for (li, text) in sql.split('\n').enumerate() {
        let start = offset;
        let end = offset + text.len();
        offset = end + 1;
        if end < from || start > to {
            continue;
        }
        let a = from.max(start) - start;
        let z = to.min(end) - start;
        let x0 = if start >= from { 0 } else { col(&text[..a]) };
        let x1 = col(&text[..z]).max(x0 + 1);
        rows.push(Row { x0, x1, y: li });
    }
    let (Some(first), Some(last_row)) = (rows.first(), rows.last()) else {
        return Vec::new();
    };
    let pad = 3.0;
    let x = |c: usize, side: f32| m.pad_left + c as f32 * m.char_width + side * pad;
    let y = |row: usize| m.pad_top + row as f32 * m.line_height;
    let mut pts = vec![(x(first.x0, -1.0), y(first.y))];
    // Down the right side.
    for r in &rows {
        pts.push((x(r.x1, 1.0), y(r.y)));
        pts.push((x(r.x1, 1.0), y(r.y + 1)));
    }
    // Back up the left side.
    pts.push((x(last_row.x0, -1.0), y(last_row.y + 1)));
    for r in rows.iter().rev() {
        pts.push((x(r.x0, -1.0), y(r.y + 1)));
        pts.push((x(r.x0, -1.0), y(r.y)));
    }
    pts
}

/// [`range_outline`] as an SVG path (`M…L…Z`), empty when there is no outline.
pub fn range_path(sql: &str, range: Span, m: &TextMetrics) -> String {
    let pts = range_outline(sql, range, m);
    if pts.is_empty() {
        return String::new();
    }
    let body: Vec<String> = pts.iter().map(|(x, y)| format!("{x:.1},{y:.1}")).collect();
    format!("M{}Z", body.join("L"))
}

/// One-line preview of a statement for the chooser: blanks collapsed, at
/// most `max` characters (an ellipsis ends a cut one).
pub fn preview(sql: &str, r: Span, max: usize) -> String {
    let mut flat = String::new();
    for word in r.text(sql).split(is_space).filter(|w| !w.is_empty()) {
        if !flat.is_empty() {
            flat.push(' ');
        }
        flat.push_str(word);
    }
    if flat.chars().count() > max {
        let mut cut: String = flat.chars().take(max.saturating_sub(1)).collect();
        cut.push('…');
        cut
    } else {
        flat
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCRIPT: &str = "-- Q1
SELECT 1;

-- Q2: subquery
SELECT a FROM t
WHERE id IN (SELECT id FROM u WHERE x = ';')
GROUP BY 1;
";

    fn text(r: Span) -> &'static str {
        r.text(SCRIPT)
    }

    fn idx(needle: &str) -> usize {
        SCRIPT.find(needle).unwrap()
    }

    #[test]
    fn splits_on_semicolon_outside_strings_and_leaves_comments_out() {
        let all = split_statements(SCRIPT);
        assert_eq!(
            all.iter().map(|s| text(s.span())).collect::<Vec<_>>(),
            [
                "SELECT 1",
                "SELECT a FROM t\nWHERE id IN (SELECT id FROM u WHERE x = ';')\nGROUP BY 1"
            ]
        );
        assert_eq!(
            all[1]
                .subqueries
                .iter()
                .map(|r| text(*r))
                .collect::<Vec<_>>(),
            ["SELECT id FROM u WHERE x = ';'"]
        );
    }

    #[test]
    fn ignores_semicolons_in_comments_and_dollar_quotes() {
        assert_eq!(
            split_statements("do $$ a; b $$; /* ; */ select 2 -- ;").len(),
            2
        );
    }

    #[test]
    fn picks_the_statement_at_the_caret_then_subqueries_around_it() {
        let in_sub = idx("FROM u");
        assert_eq!(
            statements_at(SCRIPT, in_sub)
                .into_iter()
                .map(text)
                .collect::<Vec<_>>(),
            [
                "SELECT a FROM t\nWHERE id IN (SELECT id FROM u WHERE x = ';')\nGROUP BY 1",
                "SELECT id FROM u WHERE x = ';'"
            ]
        );
        // Right before the opening paren still offers the subquery.
        assert_eq!(statements_at(SCRIPT, idx("(SELECT")).len(), 2);
        assert_eq!(statements_at(SCRIPT, idx("GROUP")).len(), 1);
    }

    #[test]
    fn keeps_the_caret_after_semicolon_on_that_statement_and_blank_lines_go_to_the_next() {
        let after_semi = idx("SELECT 1;") + "SELECT 1;".len();
        assert_eq!(
            statements_at(SCRIPT, after_semi)
                .into_iter()
                .map(text)
                .collect::<Vec<_>>(),
            ["SELECT 1"]
        );
        assert!(text(statements_at(SCRIPT, after_semi + 1)[0]).starts_with("SELECT a"));
        assert!(text(statements_at(SCRIPT, idx("-- Q2"))[0]).starts_with("SELECT a"));
        assert!(text(statements_at(SCRIPT, SCRIPT.len())[0]).starts_with("SELECT a"));
    }

    #[test]
    fn range_path_outlines_the_lines_of_a_range_as_one_stepped_shape() {
        let m = TextMetrics {
            char_width: 10.0,
            line_height: 20.0,
            pad_left: 0.0,
            pad_top: 0.0,
            tab_size: 4,
        };
        let d = range_path("ab\nabcd", Span { from: 0, to: 7 }, &m);
        assert!(
            d.starts_with("M-3.0,0.0L23.0,0.0L23.0,20.0L43.0,20.0L43.0,40.0"),
            "{d}"
        );
        assert!(d.ends_with('Z'));
    }

    // --- Rust-port specific ---------------------------------------------------

    #[test]
    fn uses_byte_offsets_on_non_ascii_text() {
        let sql = "select 'é;ü' as \"ñ\";\nselect (select 'π')  ;  ";
        let all = split_statements(sql);
        let texts: Vec<&str> = all.iter().map(|s| s.span().text(sql)).collect();
        assert_eq!(texts, ["select 'é;ü' as \"ñ\"", "select (select 'π')"]);
        assert_eq!(all[0].to, sql.find("\";").unwrap() + 1);
        assert_eq!(all[1].subqueries[0].text(sql), "select 'π'");
        // A caret after the second `select` (on a multibyte line) finds it.
        let pos = sql.find("'π'").unwrap() + "'π'".len();
        let at = statements_at(sql, pos);
        assert_eq!(at.len(), 2);
        assert_eq!(at[1].text(sql), "select 'π'");
        assert_eq!(statements_at("  -- only a comment", 3), []);
    }

    #[test]
    fn preview_collapses_blanks_and_cuts_by_characters() {
        let sql = "select\n  'ééé',\t x  from t";
        let all = Span {
            from: 0,
            to: sql.len(),
        };
        assert_eq!(preview(sql, all, 60), "select 'ééé', x from t");
        assert_eq!(preview(sql, all, 10), "select 'é…");
    }
}
