//! Local sorting of a console result (DataGrid.tsx `cycleSort` / `view`):
//! a header click cycles a column ascending → descending → loaded order.
//! Numbers compare numerically, everything else as text; NULLs always go
//! last. Table tabs sort through the query instead (`GridEvent::ServerSortRequested`).

use std::cmp::Ordering;

use boa_core::grid::values::ValueKind;
use boa_core::model::Value;

/// The column a console result is sorted by.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LocalSort {
    pub col: usize,
    pub descending: bool,
}

/// The next sort after a click on `col`: another column starts ascending,
/// the same column goes descending, then back to none.
pub fn next_sort(current: Option<LocalSort>, col: usize) -> Option<LocalSort> {
    match current {
        Some(s) if s.col == col && !s.descending => Some(LocalSort {
            col,
            descending: true,
        }),
        Some(s) if s.col == col => None,
        _ => Some(LocalSort {
            col,
            descending: false,
        }),
    }
}

/// A precomputed sort key (parsing / lowercasing once per row, not per comparison).
enum Key {
    Null,
    Number(f64),
    Text(String),
}

fn compare(a: &Key, b: &Key, original: (&str, &str)) -> Ordering {
    match (a, b) {
        (Key::Number(x), Key::Number(y)) => x.partial_cmp(y).unwrap_or(Ordering::Equal),
        // `localeCompare` in an English locale: case-insensitive first, then by case.
        (Key::Text(x), Key::Text(y)) => x.cmp(y).then_with(|| original.0.cmp(original.1)),
        _ => Ordering::Equal,
    }
}

/// The row order for `sort` over `rows` (indices into `rows`). Stable, so
/// equal values keep their loaded order; NULLs last in both directions.
pub fn sorted_order(rows: &[Vec<Value>], sort: LocalSort, kind: ValueKind) -> Vec<usize> {
    let numeric = kind == ValueKind::Number;
    let keys: Vec<Key> = rows
        .iter()
        .map(|r| match r.get(sort.col).and_then(|v| v.as_deref()) {
            None => Key::Null,
            // `Number(x)`: unparsable text is NaN, which compares as equal.
            Some(v) if numeric => Key::Number(v.trim().parse().unwrap_or(f64::NAN)),
            Some(v) => Key::Text(v.to_lowercase()),
        })
        .collect();
    let text = |i: usize| {
        rows[i]
            .get(sort.col)
            .and_then(|v| v.as_deref())
            .unwrap_or_default()
    };
    let mut order: Vec<usize> = (0..rows.len()).collect();
    order.sort_by(|&a, &b| match (&keys[a], &keys[b]) {
        (Key::Null, Key::Null) => Ordering::Equal,
        (Key::Null, _) => Ordering::Greater,
        (_, Key::Null) => Ordering::Less,
        (x, y) => {
            let o = compare(x, y, (text(a), text(b)));
            if sort.descending { o.reverse() } else { o }
        }
    });
    order
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows(values: &[Option<&str>]) -> Vec<Vec<Value>> {
        values.iter().map(|v| vec![v.map(str::to_owned)]).collect()
    }

    #[test]
    fn header_clicks_cycle_asc_desc_none() {
        let asc = Some(LocalSort {
            col: 1,
            descending: false,
        });
        let desc = Some(LocalSort {
            col: 1,
            descending: true,
        });
        assert_eq!(next_sort(None, 1), asc);
        assert_eq!(next_sort(asc, 1), desc);
        assert_eq!(next_sort(desc, 1), None);
        // Another column starts over ascending.
        assert_eq!(
            next_sort(desc, 2),
            Some(LocalSort {
                col: 2,
                descending: false
            })
        );
    }

    #[test]
    fn numbers_sort_numerically_and_nulls_go_last() {
        let r = rows(&[Some("10"), None, Some("9"), Some("-1"), Some("100")]);
        let asc = LocalSort {
            col: 0,
            descending: false,
        };
        assert_eq!(sorted_order(&r, asc, ValueKind::Number), [3, 2, 0, 4, 1]);
        let desc = LocalSort {
            descending: true,
            ..asc
        };
        assert_eq!(sorted_order(&r, desc, ValueKind::Number), [4, 0, 2, 3, 1]);
        // As text, "10" < "100" < "9".
        assert_eq!(sorted_order(&r, asc, ValueKind::Text), [3, 0, 4, 2, 1]);
    }

    #[test]
    fn text_sorts_case_insensitively_and_stably() {
        let r = rows(&[Some("b"), Some("A"), Some("a"), Some("B"), Some("a")]);
        let asc = LocalSort {
            col: 0,
            descending: false,
        };
        // "A" before "a" (by case after the case-insensitive comparison), equal ones keep their order.
        assert_eq!(sorted_order(&r, asc, ValueKind::Text), [1, 2, 4, 3, 0]);
    }
}
