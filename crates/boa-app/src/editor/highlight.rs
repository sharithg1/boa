//! Syntax highlighting for the SQL editor from the ported tokenizer
//! (`boa_core::sql::tokenize`), plugged into gpui-component's editor through
//! its `InputHighlighter` seam. Colours follow the Tauri app's
//! `SqlHighlight.tsx` (`CLASS`) and main.css (`--sql-*`), via [`Palette`].
//!
//! The whole buffer is re-tokenized on every edit: one linear pass with no
//! allocation per token, fine for console-sized scripts.

use std::cell::RefCell;
use std::ops::Range;
use std::rc::Rc;

use boa_core::sql::{Catalog, Token, TokenKind, tokenize_into};
use gpui::{Context, FontStyle, HighlightStyle, SharedString, Window};
use gpui_component::input::{
    EditorState, FoldRange, HighlightStyleResolver, InputEdit, InputHighlighter,
    InputHighlighterFactory, Rope,
};

use crate::theme::Palette;

/// The names the database knows (schemas, tables, columns), shared between
/// the editor and its highlighter. `None` until the catalog is loaded.
pub type SharedCatalog = Rc<RefCell<Option<Catalog>>>;

/// The style of a token kind (`SqlHighlight.tsx`'s `CLASS`): schema and
/// table names share a colour, comments are italic, plain text keeps the
/// editor's foreground.
pub fn token_style(kind: TokenKind, p: &Palette) -> HighlightStyle {
    let color = match kind {
        TokenKind::Keyword => p.sql_keyword,
        TokenKind::Type => p.sql_type,
        TokenKind::Function => p.sql_function,
        TokenKind::String => p.sql_string,
        TokenKind::Number => p.sql_number,
        TokenKind::Comment => p.sql_comment,
        TokenKind::Param => p.sql_param,
        TokenKind::Schema | TokenKind::Table => p.sql_table,
        TokenKind::Column => p.sql_column,
        TokenKind::Operator => p.sql_operator,
        TokenKind::Plain => return HighlightStyle::default(),
    };
    HighlightStyle {
        color: Some(color),
        font_style: (kind == TokenKind::Comment).then_some(FontStyle::Italic),
        ..HighlightStyle::default()
    }
}

/// Style runs for the byte range `range` of a buffer whose tokens are
/// `tokens` (contiguous, from 0, as [`tokenize_into`] makes them): ordered,
/// non-empty, non-overlapping, covering `range` exactly, as the editor asks.
/// Neighbouring tokens of the same kind are merged into one run.
pub fn style_runs(
    tokens: &[Token],
    range: &Range<usize>,
    palette: &Palette,
) -> Vec<(Range<usize>, HighlightStyle)> {
    let mut runs: Vec<(Range<usize>, TokenKind)> = Vec::new();
    if range.start >= range.end {
        return Vec::new();
    }
    // First token that ends after the start of the range.
    let first = tokens.partition_point(|t| t.end <= range.start);
    let mut covered = range.start;
    for t in &tokens[first..] {
        if t.start >= range.end {
            break;
        }
        let start = t.start.max(range.start);
        let end = t.end.min(range.end);
        if start > covered {
            // A gap (stale tokens): plain text.
            push_run(&mut runs, covered..start, TokenKind::Plain);
        }
        if end > start {
            push_run(&mut runs, start..end, t.kind);
            covered = end;
        }
    }
    if covered < range.end {
        push_run(&mut runs, covered..range.end, TokenKind::Plain);
    }
    runs.into_iter()
        .map(|(r, kind)| (r, token_style(kind, palette)))
        .collect()
}

fn push_run(runs: &mut Vec<(Range<usize>, TokenKind)>, range: Range<usize>, kind: TokenKind) {
    if let Some((last, last_kind)) = runs.last_mut()
        && *last_kind == kind
        && last.end == range.start
    {
        last.end = range.end;
        return;
    }
    runs.push((range, kind));
}

/// The editor's highlighter: the tokens of the current text.
#[derive(Debug)]
pub struct SqlHighlighter {
    tokens: Vec<Token>,
    catalog: SharedCatalog,
    palette: Palette,
}

impl SqlHighlighter {
    pub fn new(catalog: SharedCatalog, palette: Palette) -> Self {
        Self {
            tokens: Vec::new(),
            catalog,
            palette,
        }
    }

    /// Re-tokenizes `text`.
    pub fn retokenize(&mut self, text: &str) {
        let catalog = self.catalog.borrow();
        tokenize_into(text, catalog.as_ref(), &mut self.tokens);
    }

    pub fn tokens(&self) -> &[Token] {
        &self.tokens
    }
}

impl InputHighlighter for SqlHighlighter {
    fn language(&self) -> SharedString {
        SharedString::new_static("sql")
    }

    fn update(
        &mut self,
        _edit: Option<InputEdit>,
        text: &Rope,
        _folding: bool,
        _window: &mut Window,
        _cx: &mut Context<EditorState>,
    ) {
        self.retokenize(&text.to_string());
    }

    fn update_batch(
        &mut self,
        edits: &[(InputEdit, Rope)],
        _folding: bool,
        _window: &mut Window,
        _cx: &mut Context<EditorState>,
    ) {
        // Only the final text matters: tokenize once.
        if let Some((_, text)) = edits.last() {
            self.retokenize(&text.to_string());
        }
    }

    fn styles(
        &self,
        range: &Range<usize>,
        _resolver: &dyn HighlightStyleResolver,
    ) -> Vec<(Range<usize>, HighlightStyle)> {
        style_runs(&self.tokens, range, &self.palette)
    }

    fn fold_ranges(&self, _text: &Rope) -> Vec<FoldRange> {
        Vec::new()
    }
}

/// The factory the editor state calls to create its highlighter (whatever
/// language name it asks for: the editor is SQL only).
pub fn factory(catalog: SharedCatalog, palette: Palette) -> InputHighlighterFactory {
    Rc::new(move |_language: &str| {
        Some(
            Box::new(SqlHighlighter::new(catalog.clone(), palette.clone()))
                as Box<dyn InputHighlighter>,
        )
    })
}

#[cfg(test)]
mod tests {
    use boa_core::sql::{Column, Schema, Table, tokenize};

    use super::*;

    fn runs(sql: &str, catalog: Option<&Catalog>) -> Vec<(String, Option<gpui::Hsla>, bool)> {
        let p = Palette::dark();
        let tokens = tokenize(sql, catalog);
        style_runs(&tokens, &(0..sql.len()), &p)
            .into_iter()
            .map(|(r, s)| {
                (
                    sql[r].to_owned(),
                    s.color,
                    s.font_style == Some(FontStyle::Italic),
                )
            })
            .collect()
    }

    #[test]
    fn highlights_a_sample_like_sql_highlight_tsx() {
        let p = Palette::dark();
        let schemas = vec![Schema {
            name: "public".into(),
            tables: vec![Table {
                name: "orders".into(),
                columns: vec![Column {
                    name: "total".into(),
                    ty: "numeric".into(),
                    ..Column::default()
                }],
                ..Table::default()
            }],
        }];
        let catalog = Catalog::from_schemas(&schemas);
        let sql = "-- top\nselect count(*), total::int from public.orders where id = $1 and s = 'x' limit 10;";
        let got = runs(sql, Some(&catalog));
        let colour_of = |text: &str| {
            got.iter()
                .find(|(t, _, _)| t == text)
                .unwrap_or_else(|| panic!("no run {text:?} in {got:?}"))
                .1
        };
        assert_eq!(colour_of("-- top"), Some(p.sql_comment));
        assert!(got.iter().any(|(t, _, italic)| t == "-- top" && *italic));
        assert_eq!(colour_of("select"), Some(p.sql_keyword));
        assert_eq!(colour_of("count"), Some(p.sql_function));
        assert_eq!(colour_of("int"), Some(p.sql_type));
        assert_eq!(colour_of("public"), Some(p.sql_table));
        assert_eq!(colour_of("orders"), Some(p.sql_table));
        assert_eq!(colour_of("total"), Some(p.sql_column));
        assert_eq!(colour_of("$1"), Some(p.sql_param));
        assert_eq!(colour_of("'x'"), Some(p.sql_string));
        assert_eq!(colour_of("10"), Some(p.sql_number));
        // The runs cover the text exactly, in order.
        let joined: String = got.iter().map(|(t, _, _)| t.as_str()).collect();
        assert_eq!(joined, sql);
    }

    #[test]
    fn without_a_catalog_names_stay_plain() {
        let p = Palette::dark();
        let got = runs("select x from t", None);
        assert_eq!(got.last().map(|r| (r.0.as_str(), r.1)), Some((" t", None)));
        assert!(
            got.iter()
                .all(|(_, c, _)| *c != Some(p.sql_table) && *c != Some(p.sql_column))
        );
    }

    #[test]
    fn runs_cover_a_sub_range_exactly() {
        let p = Palette::dark();
        let sql = "select 1;\nselect 'é' from t;";
        let tokens = tokenize(sql, None);
        for start in 0..sql.len() {
            for end in start..=sql.len() {
                if !sql.is_char_boundary(start) || !sql.is_char_boundary(end) {
                    continue;
                }
                let r = style_runs(&tokens, &(start..end), &p);
                let mut at = start;
                for (range, _) in &r {
                    assert_eq!(range.start, at, "{start}..{end}: {r:?}");
                    assert!(range.end > range.start);
                    at = range.end;
                }
                assert_eq!(at, end.max(start), "{start}..{end}: {r:?}");
            }
        }
    }

    #[test]
    fn stale_tokens_still_cover_the_range() {
        let p = Palette::dark();
        let tokens = tokenize("select", None);
        let r = style_runs(&tokens, &(0..10), &p);
        assert_eq!(r.first().map(|x| x.0.clone()), Some(0..6));
        assert_eq!(r.last().map(|x| x.0.clone()), Some(6..10));
    }
}
