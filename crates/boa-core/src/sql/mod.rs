//! SQL language logic for the console editor: a tokenizer for highlighting
//! ([`tokenize`]), statement splitting and the statement under the caret
//! ([`statements`]), schema-aware completion ([`complete`]) and the write
//! guard rails ([`guard`]). No parser, no I/O, no UI: plain functions over
//! `&str`, ported from the Tauri app's `ui/src/lib/{sql,statements,complete,guard}.ts`.
//!
//! # Offsets
//!
//! Every position in this API (token ranges, statement ranges, the caret
//! passed to [`complete::complete`], completion replacement ranges, the caret
//! inside a completion's insert text) is a **byte offset into the UTF-8
//! string**, always on a `char` boundary. The TypeScript original used UTF-16
//! code-unit offsets; the two agree on ASCII text only, so callers coming from
//! a UTF-16 world (none in the GPUI app) must convert. A caret that is not on a
//! char boundary is moved back to the previous one.
//!
//! Character classes follow the JS regexes of the original: identifiers are
//! ASCII letters, digits, `_`, `$` and every character from U+00C0 up
//! (which in UTF-16 also covered astral characters through their surrogates);
//! "blank" is the JS `\s` set.

mod chars;
pub mod complete;
pub mod guard;
pub mod statements;
pub mod tokenize;

pub use crate::model::{
    Column, Constraint, ConstraintKind, ForeignRef, Index, Schema, Table, TableKind,
};
pub use complete::{
    CompleteOptions, Completion, CompletionKind, CompletionResult, complete, match_tier,
};
pub use guard::{
    ConfirmOptions, Risk, RiskKind, destructive_risks, needs_confirm, read_only_escape,
};
pub use statements::{
    Span, Statement, TextMetrics, preview, range_outline, range_path, split_statements,
    statements_at,
};
pub use tokenize::{
    Catalog, KEYWORDS, TYPES, Token, TokenKind, is_keyword, is_type, qualified, quote_ident,
    tokenize, tokenize_into,
};
