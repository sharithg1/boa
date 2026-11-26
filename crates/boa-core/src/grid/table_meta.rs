//! Keys, foreign keys, checks and indexes of an introspected table, shaped
//! for the Database Explorer's folders; and DDL built from that metadata
//! (`tableMeta.ts`; the real app asks the server: `table_ddl`).

use super::js::is_js_space;
use crate::model::{Constraint, ConstraintKind, Table, TableKind};
use crate::sql::{qualified, quote_ident};
use std::collections::BTreeMap;

/// A folder under a table in the explorer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MetaFolderKind {
    Keys,
    ForeignKeys,
    Indexes,
    Checks,
}

impl MetaFolderKind {
    /// Folder label: "keys", "foreign keys", "indexes", "checks".
    pub fn label(self) -> &'static str {
        match self {
            Self::Keys => "keys",
            Self::ForeignKeys => "foreign keys",
            Self::Indexes => "indexes",
            Self::Checks => "checks",
        }
    }
}

/// The key icon's flavour.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum KeyKind {
    Primary,
    Unique,
    Foreign,
}

/// One entry of a folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MetaItem {
    pub name: String,
    /// Short trailing hint: `(org_id) → organizations(id)`.
    pub detail: String,
    /// The server's definition (constraint or CREATE INDEX).
    pub definition: String,
    /// For keys / foreign keys / unique indexes.
    pub key: Option<KeyKind>,
    pub constraint: Option<Constraint>,
}

/// A folder and its entries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MetaFolder {
    pub kind: MetaFolderKind,
    pub items: Vec<MetaItem>,
}

fn cols<S: AsRef<str>>(names: &[S]) -> String {
    let names: Vec<&str> = names.iter().map(AsRef::as_ref).collect();
    format!("({})", names.join(", "))
}

/// Primary key, unique, exclusion, foreign keys, checks (the server's DDL
/// order); stable within a kind.
fn by_kind(cs: &[Constraint]) -> Vec<&Constraint> {
    let rank = |c: &Constraint| "puxfc".find(c.kind.code()).unwrap_or(5);
    let mut out: Vec<&Constraint> = cs.iter().collect();
    out.sort_by_key(|c| rank(c));
    out
}

/// DataGrip's folders under a table, in its order; empty ones are left out.
pub fn meta_folders(schema: &str, t: &Table) -> Vec<MetaFolder> {
    let cons = by_kind(&t.constraints);
    let of = |kinds: &[ConstraintKind]| -> Vec<&Constraint> {
        cons.iter()
            .copied()
            .filter(|c| kinds.contains(&c.kind))
            .collect()
    };
    let keys = of(&[
        ConstraintKind::PrimaryKey,
        ConstraintKind::Unique,
        ConstraintKind::Exclusion,
    ])
    .into_iter()
    .map(|c| MetaItem {
        name: c.name.clone(),
        detail: if c.kind == ConstraintKind::Exclusion {
            c.definition.clone()
        } else {
            cols(&c.columns)
        },
        definition: c.definition.clone(),
        key: Some(if c.kind == ConstraintKind::PrimaryKey {
            KeyKind::Primary
        } else {
            KeyKind::Unique
        }),
        constraint: Some(c.clone()),
    })
    .collect();
    let foreign = of(&[ConstraintKind::ForeignKey])
        .into_iter()
        .map(|c| {
            let target = match &c.references {
                Some(r) if r.schema == schema => {
                    format!("{}{}", quote_ident(&r.table), cols(&r.columns))
                }
                Some(r) => format!("{}{}", qualified(&r.schema, &r.table), cols(&r.columns)),
                None => "?".into(),
            };
            MetaItem {
                name: c.name.clone(),
                detail: format!("{} → {target}", cols(&c.columns)),
                definition: c.definition.clone(),
                key: Some(KeyKind::Foreign),
                constraint: Some(c.clone()),
            }
        })
        .collect();
    let indexes = t
        .indexes
        .iter()
        .map(|i| MetaItem {
            name: i.name.clone(),
            detail: format!(
                "{}{}",
                cols(&i.columns),
                if i.unique && !i.primary {
                    " unique"
                } else {
                    ""
                }
            ),
            definition: i.definition.clone(),
            key: if i.primary {
                Some(KeyKind::Primary)
            } else if i.unique {
                Some(KeyKind::Unique)
            } else {
                None
            },
            constraint: None,
        })
        .collect();
    let checks = of(&[ConstraintKind::Check])
        .into_iter()
        .map(|c| MetaItem {
            name: c.name.clone(),
            detail: strip_check(&c.definition).to_owned(),
            definition: c.definition.clone(),
            key: None,
            constraint: Some(c.clone()),
        })
        .collect();
    [
        (MetaFolderKind::Keys, keys),
        (MetaFolderKind::ForeignKeys, foreign),
        (MetaFolderKind::Indexes, indexes),
        (MetaFolderKind::Checks, checks),
    ]
    .into_iter()
    .filter(|(_, items): &(_, Vec<MetaItem>)| !items.is_empty())
    .map(|(kind, items)| MetaFolder { kind, items })
    .collect()
}

/// `definition` without a leading `CHECK` (`/^CHECK\s*/i`).
fn strip_check(definition: &str) -> &str {
    match definition.get(..5) {
        Some(head) if head.eq_ignore_ascii_case("check") => {
            definition[5..].trim_start_matches(is_js_space)
        }
        _ => definition,
    }
}

/// Which key each column is part of: the primary key first, then a foreign key.
pub fn column_keys(t: &Table) -> BTreeMap<String, KeyKind> {
    let mut out = BTreeMap::new();
    for c in t
        .constraints
        .iter()
        .filter(|c| c.kind == ConstraintKind::ForeignKey)
    {
        for n in &c.columns {
            out.insert(n.clone(), KeyKind::Foreign);
        }
    }
    for c in t.columns.iter().filter(|c| c.pk) {
        out.insert(c.name.clone(), KeyKind::Primary);
    }
    for c in t
        .constraints
        .iter()
        .filter(|c| c.kind == ConstraintKind::PrimaryKey)
    {
        for n in &c.columns {
            out.insert(n.clone(), KeyKind::Primary);
        }
    }
    out
}

/// DDL shaped like the server's (`table_ddl` in `pg/catalog.rs`), from the
/// introspected metadata. Views have no definition in the metadata.
pub fn ddl_from_metadata(schema: &str, t: &Table) -> String {
    let q = qualified(schema, &t.name);
    if t.kind == TableKind::View {
        let names: Vec<String> = t.columns.iter().map(|c| quote_ident(&c.name)).collect();
        return format!(
            "CREATE VIEW {q} AS\n SELECT {}\n   FROM …;\n",
            names.join(",\n    ")
        );
    }
    let cons = by_kind(&t.constraints);
    let mut lines: Vec<String> = t
        .columns
        .iter()
        .map(|c| {
            let mut l = format!("{} {}", quote_ident(&c.name), c.ty);
            if let Some(d) = c.default.as_deref().filter(|d| !d.is_empty()) {
                l.push_str(" DEFAULT ");
                l.push_str(d);
            }
            if !c.nullable {
                l.push_str(" NOT NULL");
            }
            l
        })
        .collect();
    lines.extend(
        cons.iter()
            .map(|c| format!("CONSTRAINT {} {}", quote_ident(&c.name), c.definition)),
    );
    // Metadata without constraints: the columns' primary-key flags.
    let pk: Vec<String> = t
        .columns
        .iter()
        .filter(|c| c.pk)
        .map(|c| quote_ident(&c.name))
        .collect();
    if !cons.iter().any(|c| c.kind == ConstraintKind::PrimaryKey) && !pk.is_empty() {
        lines.push(format!("PRIMARY KEY {}", cols(&pk)));
    }
    let mut out = if lines.is_empty() {
        format!("CREATE TABLE {q} ();\n")
    } else {
        format!("CREATE TABLE {q}\n(\n    {}\n);\n", lines.join(",\n    "))
    };
    // Indexes of primary key / unique / exclusion constraints come with them.
    let owned: Vec<&str> = cons
        .iter()
        .filter(|c| {
            matches!(
                c.kind,
                ConstraintKind::PrimaryKey | ConstraintKind::Unique | ConstraintKind::Exclusion
            )
        })
        .map(|c| c.name.as_str())
        .collect();
    let indexes: Vec<String> = t
        .indexes
        .iter()
        .filter(|i| !owned.contains(&i.name.as_str()))
        .map(|i| format!("{};", i.definition))
        .collect();
    if !indexes.is_empty() {
        out.push_str(&format!("\n{}\n", indexes.join("\n")));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Column, ForeignRef, Index};

    fn col(name: &str, ty: &str) -> Column {
        Column {
            name: name.into(),
            ty: ty.into(),
            nullable: false,
            pk: false,
            default: None,
        }
    }

    fn strings(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| (*s).to_owned()).collect()
    }

    fn invoices() -> Table {
        Table {
            name: "invoices".into(),
            kind: TableKind::Table,
            rows: 0,
            size_bytes: 0,
            columns: vec![
                Column { pk: true, ..col("id", "bigint") },
                col("org_id", "bigint"),
                Column { nullable: true, ..col("actor_id", "bigint") },
                Column { default: Some("0".into()), ..col("amount", "integer") },
            ],
            constraints: vec![
                Constraint {
                    name: "invoices_pkey".into(),
                    kind: ConstraintKind::PrimaryKey,
                    columns: strings(&["id"]),
                    definition: "PRIMARY KEY (id)".into(),
                    references: None,
                },
                Constraint {
                    name: "invoices_org_id_fkey".into(),
                    kind: ConstraintKind::ForeignKey,
                    columns: strings(&["org_id"]),
                    definition: "FOREIGN KEY (org_id) REFERENCES orgs(id)".into(),
                    references: Some(ForeignRef {
                        schema: "public".into(),
                        table: "orgs".into(),
                        columns: strings(&["id"]),
                    }),
                },
                Constraint {
                    name: "invoices_actor_id_fkey".into(),
                    kind: ConstraintKind::ForeignKey,
                    columns: strings(&["actor_id"]),
                    definition: "FOREIGN KEY (actor_id) REFERENCES auth.\"Users\"(id)".into(),
                    references: Some(ForeignRef {
                        schema: "auth".into(),
                        table: "Users".into(),
                        columns: strings(&["id"]),
                    }),
                },
                Constraint {
                    name: "invoices_amount_check".into(),
                    kind: ConstraintKind::Check,
                    columns: strings(&["amount"]),
                    definition: "CHECK (amount >= 0)".into(),
                    references: None,
                },
            ],
            indexes: vec![
                Index {
                    name: "invoices_pkey".into(),
                    columns: strings(&["id"]),
                    unique: true,
                    primary: true,
                    definition: "CREATE UNIQUE INDEX invoices_pkey ON public.invoices USING btree (id)".into(),
                },
                Index {
                    name: "invoices_lower_idx".into(),
                    columns: strings(&["org_id", "lower(note)"]),
                    unique: true,
                    primary: false,
                    definition: "CREATE UNIQUE INDEX invoices_lower_idx ON public.invoices USING btree (org_id, lower(note))".into(),
                },
            ],
        }
    }

    #[test]
    fn groups_keys_foreign_keys_indexes_and_checks_in_order() {
        let f = meta_folders("public", &invoices());
        assert_eq!(
            f.iter().map(|x| x.kind).collect::<Vec<_>>(),
            [
                MetaFolderKind::Keys,
                MetaFolderKind::ForeignKeys,
                MetaFolderKind::Indexes,
                MetaFolderKind::Checks
            ]
        );
        assert_eq!(f[0].items.len(), 1);
        assert_eq!(f[0].items[0].name, "invoices_pkey");
        assert_eq!(f[0].items[0].detail, "(id)");
        assert_eq!(f[0].items[0].key, Some(KeyKind::Primary));
        assert_eq!(
            f[1].items
                .iter()
                .map(|i| i.detail.as_str())
                .collect::<Vec<_>>(),
            ["(org_id) → orgs(id)", "(actor_id) → auth.\"Users\"(id)"]
        );
        assert_eq!(
            f[2].items
                .iter()
                .map(|i| (i.detail.as_str(), i.key))
                .collect::<Vec<_>>(),
            [
                ("(id)", Some(KeyKind::Primary)),
                ("(org_id, lower(note)) unique", Some(KeyKind::Unique))
            ]
        );
        assert_eq!(f[3].items[0].detail, "(amount >= 0)");
    }

    #[test]
    fn leaves_out_empty_folders() {
        let bare = Table {
            constraints: vec![],
            indexes: vec![],
            ..invoices()
        };
        assert_eq!(meta_folders("public", &bare), []);
    }

    #[test]
    fn marks_primary_key_and_foreign_key_columns() {
        let k = column_keys(&invoices());
        assert_eq!(
            k,
            BTreeMap::from([
                ("id".to_owned(), KeyKind::Primary),
                ("org_id".to_owned(), KeyKind::Foreign),
                ("actor_id".to_owned(), KeyKind::Foreign),
            ])
        );
    }

    #[test]
    fn builds_create_table_with_constraints_then_the_other_indexes() {
        assert_eq!(
            ddl_from_metadata("public", &invoices()),
            "CREATE TABLE public.invoices
(
    id bigint NOT NULL,
    org_id bigint NOT NULL,
    actor_id bigint,
    amount integer DEFAULT 0 NOT NULL,
    CONSTRAINT invoices_pkey PRIMARY KEY (id),
    CONSTRAINT invoices_org_id_fkey FOREIGN KEY (org_id) REFERENCES orgs(id),
    CONSTRAINT invoices_actor_id_fkey FOREIGN KEY (actor_id) REFERENCES auth.\"Users\"(id),
    CONSTRAINT invoices_amount_check CHECK (amount >= 0)
);

CREATE UNIQUE INDEX invoices_lower_idx ON public.invoices USING btree (org_id, lower(note));
"
        );
    }

    #[test]
    fn falls_back_to_the_columns_primary_key_flags_and_quotes_names() {
        let t = Table {
            name: "Odd".into(),
            kind: TableKind::Table,
            rows: 0,
            size_bytes: 0,
            columns: vec![
                Column {
                    pk: true,
                    ..col("id", "int")
                },
                Column {
                    nullable: true,
                    ..col("select", "text")
                },
            ],
            indexes: vec![],
            constraints: vec![],
        };
        assert_eq!(
            ddl_from_metadata("my schema", &t),
            "CREATE TABLE \"my schema\".\"Odd\"\n(\n    id int NOT NULL,\n    \"select\" text,\n    PRIMARY KEY (id)\n);\n"
        );
    }

    #[test]
    fn outlines_a_view() {
        let v = Table {
            kind: TableKind::View,
            columns: vec![col("id", "bigint")],
            ..invoices()
        };
        assert_eq!(
            ddl_from_metadata("public", &v),
            "CREATE VIEW public.invoices AS\n SELECT id\n   FROM …;\n"
        );
    }
}
