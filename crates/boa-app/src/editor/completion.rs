//! Completion for the SQL editor: the ported `boa_core::sql::complete`,
//! and the rules of the Tauri console around it (Workspace.tsx `suggest` /
//! `accept`, CompletionPopup.tsx): when typing opens the list, how the
//! selection moves, what Enter and Tab replace and where the caret goes.
//! The popup itself is drawn by `crate::views::completion_popup`.

use std::ops::Range;

use boa_core::sql::{CompleteOptions, Completion, CompletionResult, Schema, complete};

/// Rows the popup shows before scrolling; PageUp / PageDown move this many - 1.
pub const VISIBLE_ROWS: usize = 10;

/// The completions at byte `pos` of `sql` (Workspace.tsx `suggest`:
/// `searchPath: ['public']`, the console's default schema ranked first;
/// `explicit` after Ctrl+Space, so an empty prefix lists items too). None
/// when there is nothing to offer.
pub fn complete_at(
    sql: &str,
    pos: usize,
    schemas: &[Schema],
    default_schema: Option<&str>,
    explicit: bool,
) -> Option<CompletionResult> {
    let opts = CompleteOptions {
        default_schema: default_schema.map(str::to_owned),
        search_path: vec!["public".to_owned()],
        explicit,
        ..CompleteOptions::default()
    };
    complete(sql, pos, schemas, &opts).filter(|r| !r.items.is_empty())
}

/// Whether typing `new_text` should (re)open the list: an ASCII name
/// character, `$`, `.` or `"` (the Tauri editor's `/[\w$."]$/`).
pub fn is_trigger(new_text: &str) -> bool {
    new_text
        .chars()
        .last()
        .is_some_and(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '$' | '.' | '"'))
}

/// The selection after a key: ↑ / ↓ wrap around, PageUp / PageDown move up
/// to [`VISIBLE_ROWS`] - 1 rows without wrapping (Workspace.tsx `move`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Move {
    Up,
    Down,
    PageUp,
    PageDown,
}

pub fn moved(selected: usize, len: usize, step: Move) -> usize {
    if len == 0 {
        return 0;
    }
    let page = VISIBLE_ROWS - 1;
    match step {
        Move::Up => (selected + len - 1) % len,
        Move::Down => (selected + 1) % len,
        Move::PageDown => selected + page.min(len - 1 - selected.min(len - 1)),
        Move::PageUp => selected - page.min(selected),
    }
}

/// What accepting `item` does (Workspace.tsx `accept`): replace `range`
/// with `insert`, then put the caret at `caret`. Enter replaces from the
/// word's start up to the caret (`whole` false); Tab replaces the whole word.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Accept {
    pub range: Range<usize>,
    pub insert: String,
    pub caret: usize,
    /// A schema was picked (`billing.`): go on to its tables.
    pub reopen: bool,
}

pub fn accept(res: &CompletionResult, item: &Completion, caret: usize, whole: bool) -> Accept {
    let to = if whole { res.to } else { caret.max(res.from) };
    Accept {
        range: res.from..to,
        insert: item.insert.clone(),
        caret: res.from + item.caret.unwrap_or(item.insert.len()),
        reopen: item.insert.ends_with('.'),
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use boa_core::sql::{Column, CompletionKind, Constraint, ConstraintKind, ForeignRef, Table};

    use super::*;

    fn col(name: &str, ty: &str, pk: bool) -> Column {
        Column {
            name: name.into(),
            ty: ty.into(),
            pk,
            nullable: !pk,
            default: None,
        }
    }

    /// The demo database: customers and orders, orders.customer_id → customers.id;
    /// a second schema `billing` with `invoices`.
    pub(crate) fn demo_schemas() -> Vec<Schema> {
        let customers = Table {
            name: "customers".into(),
            columns: vec![
                col("id", "integer", true),
                col("name", "text", false),
                col("email", "text", false),
                col("country", "text", false),
                col("created_at", "timestamp with time zone", false),
            ],
            ..Table::default()
        };
        let orders = Table {
            name: "orders".into(),
            columns: vec![
                col("id", "bigint", true),
                col("customer_id", "integer", false),
                col("total", "numeric(12,2)", false),
                col("status", "text", false),
                col("payload", "jsonb", false),
            ],
            constraints: vec![Constraint {
                name: "orders_customer_id_fkey".into(),
                kind: ConstraintKind::ForeignKey,
                definition: "FOREIGN KEY (customer_id) REFERENCES customers(id)".into(),
                columns: vec!["customer_id".into()],
                references: Some(ForeignRef {
                    schema: "public".into(),
                    table: "customers".into(),
                    columns: vec!["id".into()],
                }),
            }],
            ..Table::default()
        };
        let invoices = Table {
            name: "invoices".into(),
            columns: vec![col("id", "bigint", true), col("amount", "numeric", false)],
            ..Table::default()
        };
        vec![
            Schema {
                name: "public".into(),
                tables: vec![customers, orders],
            },
            Schema {
                name: "billing".into(),
                tables: vec![invoices],
            },
        ]
    }

    /// The result at the `|` of `sql`.
    fn at(sql: &str, explicit: bool) -> Option<CompletionResult> {
        let pos = sql.find('|').unwrap();
        let sql = sql.replace('|', "");
        complete_at(&sql, pos, &demo_schemas(), Some("public"), explicit)
    }

    fn labels(res: &Option<CompletionResult>) -> Vec<String> {
        res.as_ref()
            .map(|r| r.items.iter().map(|c| c.label.clone()).collect())
            .unwrap_or_default()
    }

    #[test]
    fn tables_after_from() {
        let res = at("select * from cu|", false).expect("items for cu");
        let first = &res.items[0];
        assert_eq!(first.label, "customers");
        assert_eq!(first.kind, CompletionKind::Table);
        assert_eq!(first.insert, "customers");
        // Replaces the typed "cu".
        assert_eq!((res.from, res.to), (14, 16));
        assert!(!res.items.iter().any(|i| i.label == "orders"));
    }

    #[test]
    fn foreign_key_join_conditions_first_after_on() {
        let res = at("select * from orders o join customers c on |", true);
        let first = &res.as_ref().expect("join items").items[0];
        assert_eq!(first.label, "c.id = o.customer_id");
        assert_eq!(first.kind, CompletionKind::Join);
        // Without Ctrl+Space an empty prefix offers nothing.
        assert!(at("select * from orders o join customers c on |", false).is_none());
    }

    #[test]
    fn columns_of_the_tables_in_the_statement() {
        assert_eq!(
            labels(&at("select cust| from orders", false))[0],
            "customer_id"
        );
        let pk = at("select i| from customers", false).unwrap();
        assert_eq!(pk.items[0].label, "id");
        assert!(pk.items[0].pk);
    }

    #[test]
    fn triggers_on_name_characters_dot_and_quote() {
        assert!(is_trigger("c"));
        assert!(is_trigger("."));
        assert!(is_trigger("\""));
        assert!(is_trigger("_"));
        assert!(is_trigger("$"));
        assert!(!is_trigger(" "));
        assert!(!is_trigger("("));
        assert!(!is_trigger(""));
        // JS \w is ASCII only.
        assert!(!is_trigger("한"));
    }

    #[test]
    fn arrows_wrap_and_pages_stop_at_the_ends() {
        assert_eq!(moved(0, 5, Move::Up), 4);
        assert_eq!(moved(4, 5, Move::Down), 0);
        assert_eq!(moved(0, 30, Move::PageDown), 9);
        assert_eq!(moved(25, 30, Move::PageDown), 29);
        assert_eq!(moved(29, 30, Move::PageDown), 29);
        assert_eq!(moved(12, 30, Move::PageUp), 3);
        assert_eq!(moved(3, 30, Move::PageUp), 0);
        assert_eq!(moved(0, 0, Move::Down), 0);
    }

    #[test]
    fn enter_replaces_up_to_the_caret_tab_the_whole_word() {
        // "select * from cu|stomers_old": caret after "cu".
        let sql = "select * from custmers";
        let res = complete_at(sql, 16, &demo_schemas(), None, false).unwrap();
        assert_eq!((res.from, res.to), (14, 22));
        let item = res.items.iter().find(|c| c.label == "customers").unwrap();
        let enter = accept(&res, item, 16, false);
        assert_eq!(enter.range, 14..16);
        assert_eq!(enter.caret, 14 + "customers".len());
        let tab = accept(&res, item, 16, true);
        assert_eq!(tab.range, 14..22);
        assert!(!tab.reopen);
    }

    #[test]
    fn functions_put_the_caret_inside_and_schemas_reopen() {
        let res = at("select cou|", false).unwrap();
        let count = res
            .items
            .iter()
            .find(|c| c.insert.starts_with("count("))
            .unwrap();
        let a = accept(&res, count, 10, false);
        assert_eq!(&a.insert, "count()");
        assert_eq!(a.caret, 7 + "count(".len(), "count(|)");
        let res = at("select * from bil|", false).unwrap();
        let schema = res
            .items
            .iter()
            .find(|c| c.kind == CompletionKind::Schema)
            .unwrap();
        assert_eq!(schema.insert, "billing.");
        assert!(accept(&res, schema, 17, false).reopen);
    }
}
