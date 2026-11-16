//! Indexes, constraints and DDL.
//!
//! Introspection reads indexes and constraints of the picked schemas with one
//! catalog query each (no per-table round trips) and stitches them onto the
//! tables `RELATIONS` returned. `table_ddl` assembles one relation's DDL from
//! the server's own renderings (`format_type`, `pg_get_expr`,
//! `pg_get_constraintdef`, `pg_get_indexdef`, `pg_get_viewdef`,
//! `pg_get_triggerdef`): Postgres has no single "get table DDL" function.
//! Identifiers and comment literals are quoted by the server
//! (`quote_ident`, `quote_literal`), so the assembly here is plain text work.
//!
//! Needs PostgreSQL 12+ (`indnkeyatts`, `conparentid`, `attgenerated`).

use super::{Pg, Schema, Table};
use crate::error::{CmdError, CmdResult};
use serde::Serialize;
use std::collections::HashMap;
use std::fmt::Write as _;
use tokio_postgres::Row;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Index {
    pub name: String,
    /// Key columns in order: a column name, or the expression for an
    /// expression column (`lower(email)`). INCLUDE columns are left out.
    pub columns: Vec<String>,
    pub unique: bool,
    pub primary: bool,
    /// `CREATE [UNIQUE] INDEX … ON … USING …` (`pg_get_indexdef`).
    pub definition: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ForeignRef {
    pub schema: String,
    pub table: String,
    /// Referenced columns, in the order of the constraint's local columns.
    pub columns: Vec<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Constraint {
    pub name: String,
    /// `pg_constraint.contype`: `p` primary key, `u` unique, `f` foreign
    /// key, `c` check, `x` exclusion.
    pub kind: String,
    /// `PRIMARY KEY (id)`, `FOREIGN KEY (org_id) REFERENCES orgs(id)`, …
    /// (`pg_get_constraintdef`).
    pub definition: String,
    /// The constrained columns (`conkey`), in order.
    pub columns: Vec<String>,
    /// Foreign keys only: the referenced table and columns.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub references: Option<ForeignRef>,
}

/// Indexes of the tables (and materialized views) in the schemas `$1`.
/// Key columns: `indkey` holds attnums (0 for an expression), the first
/// `indnkeyatts` of them are keys; `pg_get_indexdef(oid, n, true)` renders
/// column `n` (1-based), name or expression.
const INDEXES: &str = r"
select n.nspname::text, t.relname::text, ic.relname::text,
       i.indisunique, i.indisprimary,
       pg_get_indexdef(i.indexrelid),
       array(select coalesce(a.attname::text, pg_get_indexdef(i.indexrelid, k.ord::int, true))
             from unnest(i.indkey::int2[]) with ordinality as k(attnum, ord)
             left join pg_attribute a on a.attrelid = i.indrelid and a.attnum = k.attnum
             where k.ord <= i.indnkeyatts
             order by k.ord)
from pg_index i
join pg_class ic on ic.oid = i.indexrelid
join pg_class t on t.oid = i.indrelid
join pg_namespace n on n.oid = t.relnamespace
where t.relkind in ('r', 'm', 'p')
  and not t.relispartition
  and n.nspname = any($1)
order by n.nspname, t.relname, i.indisprimary desc, ic.relname";

/// Constraints of the tables in the schemas `$1`, with the local (`conkey`)
/// and referenced (`confkey`) column names in key order. `conparentid = 0`
/// drops the per-partition copies a foreign key to a partitioned table
/// gets. NOT NULL constraints (`n`, Postgres 18) are columns' `nullable`.
const CONSTRAINTS: &str = r"
select n.nspname::text, t.relname::text, c.conname::text, c.contype::text,
       pg_get_constraintdef(c.oid, true),
       array(select a.attname::text
             from unnest(c.conkey) with ordinality as k(attnum, ord)
             join pg_attribute a on a.attrelid = c.conrelid and a.attnum = k.attnum
             order by k.ord),
       fn.nspname::text, ft.relname::text,
       array(select a.attname::text
             from unnest(c.confkey) with ordinality as k(attnum, ord)
             join pg_attribute a on a.attrelid = c.confrelid and a.attnum = k.attnum
             order by k.ord)
from pg_constraint c
join pg_class t on t.oid = c.conrelid
join pg_namespace n on n.oid = t.relnamespace
left join pg_class ft on ft.oid = c.confrelid
left join pg_namespace fn on fn.oid = ft.relnamespace
where c.contype in ('p', 'u', 'f', 'c', 'x')
  and c.conparentid = 0
  and t.relkind in ('r', 'p', 'f')
  and not t.relispartition
  and n.nspname = any($1)
order by n.nspname, t.relname, c.conname";

/// A text array that may hold NULLs (a definition of a concurrently dropped
/// index), without them.
fn names(r: &Row, i: usize) -> Vec<String> {
    r.get::<_, Option<Vec<Option<String>>>>(i)
        .unwrap_or_default()
        .into_iter()
        .flatten()
        .collect()
}

fn index_row(r: &Row) -> ((String, String), Index) {
    (
        (r.get(0), r.get(1)),
        Index {
            name: r.get(2),
            unique: r.get(3),
            primary: r.get(4),
            definition: r.get::<_, Option<String>>(5).unwrap_or_default(),
            columns: names(r, 6),
        },
    )
}

fn constraint_row(r: &Row) -> ((String, String), Constraint) {
    let kind: String = r.get(3);
    let references = match (r.get::<_, Option<String>>(6), r.get::<_, Option<String>>(7)) {
        (Some(schema), Some(table)) if kind == "f" => Some(ForeignRef {
            schema,
            table,
            columns: names(r, 8),
        }),
        _ => None,
    };
    (
        (r.get(0), r.get(1)),
        Constraint {
            name: r.get(2),
            kind,
            definition: r.get::<_, Option<String>>(4).unwrap_or_default(),
            columns: names(r, 5),
            references,
        },
    )
}

/// Puts indexes and constraints on their tables (keyed by schema, table);
/// rows for tables not in `schemas` are dropped.
pub(super) fn attach(
    schemas: &mut [Schema],
    indexes: impl IntoIterator<Item = ((String, String), Index)>,
    constraints: impl IntoIterator<Item = ((String, String), Constraint)>,
) {
    let mut by: HashMap<(String, String), &mut Table> = HashMap::new();
    for sc in schemas.iter_mut() {
        for t in &mut sc.tables {
            by.insert((sc.name.clone(), t.name.clone()), t);
        }
    }
    for (key, ix) in indexes {
        if let Some(t) = by.get_mut(&key) {
            t.indexes.push(ix);
        }
    }
    for (key, c) in constraints {
        if let Some(t) = by.get_mut(&key) {
            t.constraints.push(c);
        }
    }
}

// ── DDL ──────────────────────────────────────────────────────────────────

/// The relation, its name quoted and qualified, and the parts of the
/// statement that hang off `pg_class`.
const DDL_RELATION: &str = r"
select c.oid, c.relkind::text, c.relpersistence::text,
       quote_ident(n.nspname) || '.' || quote_ident(c.relname),
       case when c.relkind in ('v', 'm') then pg_get_viewdef(c.oid, true) end,
       case when c.relkind = 'p' then pg_get_partkeydef(c.oid) end,
       (select quote_ident(s.srvname) from pg_foreign_table f
          join pg_foreign_server s on s.oid = f.ftserver where f.ftrelid = c.oid),
       quote_literal(obj_description(c.oid, 'pg_class'))
from pg_class c
join pg_namespace n on n.oid = c.relnamespace
where n.nspname = $1 and c.relname = $2 and c.relkind in ('r', 'v', 'm', 'p', 'f')";

/// Columns: identity (`a` always, `d` by default) and generated (`s`
/// stored, `v` virtual) are '' when not set; the generation expression is
/// in `pg_attrdef` like a default.
const DDL_COLUMNS: &str = r"
select quote_ident(a.attname), format_type(a.atttypid, a.atttypmod), a.attnotnull,
       pg_get_expr(d.adbin, d.adrelid), a.attidentity::text, a.attgenerated::text,
       quote_literal(col_description(a.attrelid, a.attnum))
from pg_attribute a
left join pg_attrdef d on d.adrelid = a.attrelid and d.adnum = a.attnum
where a.attrelid = $1 and a.attnum > 0 and not a.attisdropped
order by a.attnum";

const DDL_CONSTRAINTS: &str = r"
select quote_ident(c.conname), pg_get_constraintdef(c.oid, true),
       quote_literal(obj_description(c.oid, 'pg_constraint'))
from pg_constraint c
where c.conrelid = $1 and c.contype in ('p', 'u', 'f', 'c', 'x') and c.conparentid = 0
order by case c.contype when 'p' then 0 when 'u' then 1 when 'x' then 2 when 'f' then 3 else 4 end,
         c.conname";

/// Indexes that no primary key / unique / exclusion constraint owns (those
/// come with their constraint).
const DDL_INDEXES: &str = r"
select pg_get_indexdef(i.indexrelid),
       quote_ident(n.nspname) || '.' || quote_ident(ic.relname),
       quote_literal(obj_description(i.indexrelid, 'pg_class'))
from pg_index i
join pg_class ic on ic.oid = i.indexrelid
join pg_namespace n on n.oid = ic.relnamespace
where i.indrelid = $1
  and not exists (select 1 from pg_constraint c
                  where c.conindid = i.indexrelid and c.conrelid = i.indrelid
                    and c.contype in ('p', 'u', 'x'))
order by ic.relname";

const DDL_TRIGGERS: &str = r"
select pg_get_triggerdef(t.oid, true)
from pg_trigger t
where t.tgrelid = $1 and not t.tgisinternal
order by t.tgname";

#[derive(Debug, Default)]
struct DdlColumn {
    /// Quoted.
    name: String,
    ty: String,
    not_null: bool,
    /// Default, or the generation expression of a generated column.
    default: Option<String>,
    identity: String,
    generated: String,
    /// A quoted literal.
    comment: Option<String>,
}

/// A constraint (`name` quoted) or an index (`name` qualified and quoted).
#[derive(Debug, Default)]
struct DdlItem {
    name: String,
    definition: String,
    comment: Option<String>,
}

#[derive(Debug, Default)]
struct DdlParts {
    /// `schema.name`, quoted.
    qualified: String,
    relkind: String,
    persistence: String,
    view_def: Option<String>,
    partition_key: Option<String>,
    foreign_server: Option<String>,
    comment: Option<String>,
    columns: Vec<DdlColumn>,
    constraints: Vec<DdlItem>,
    indexes: Vec<DdlItem>,
    triggers: Vec<String>,
}

fn column_line(c: &DdlColumn) -> String {
    let mut s = format!("{} {}", c.name, c.ty);
    match (c.generated.as_str(), c.identity.as_str(), &c.default) {
        ("s", _, Some(e)) => write!(s, " GENERATED ALWAYS AS ({e}) STORED"),
        ("v", _, Some(e)) => write!(s, " GENERATED ALWAYS AS ({e}) VIRTUAL"),
        (_, "a", _) => write!(s, " GENERATED ALWAYS AS IDENTITY"),
        (_, "d", _) => write!(s, " GENERATED BY DEFAULT AS IDENTITY"),
        (_, _, Some(e)) => write!(s, " DEFAULT {e}"),
        _ => Ok(()),
    }
    .ok();
    if c.not_null {
        s.push_str(" NOT NULL");
    }
    s
}

/// A statement from a `pg_get_*def` rendering: trailing `;` and blanks
/// normalised to one `;`.
fn statement(def: &str) -> String {
    format!("{};", def.trim_end().trim_end_matches(';').trim_end())
}

/// The DDL script: the CREATE statement, then indexes that aren't part of a
/// constraint, triggers and comments.
fn build_ddl(p: &DdlParts) -> String {
    let q = &p.qualified;
    let mut out = String::new();
    let what = match p.relkind.as_str() {
        "v" => "VIEW",
        "m" => "MATERIALIZED VIEW",
        "f" => "FOREIGN TABLE",
        _ => "TABLE",
    };
    if let Some(def) = p.view_def.as_deref().filter(|_| what.ends_with("VIEW")) {
        let def = def.trim_end().trim_end_matches(';').trim_end();
        let _ = writeln!(out, "CREATE {what} {q} AS\n{def};");
    } else {
        let lines: Vec<String> = p
            .columns
            .iter()
            .map(column_line)
            .chain(
                p.constraints
                    .iter()
                    .map(|c| format!("CONSTRAINT {} {}", c.name, c.definition)),
            )
            .collect();
        let unlogged = if p.persistence == "u" {
            "UNLOGGED "
        } else {
            ""
        };
        let _ = write!(out, "CREATE {unlogged}{what} {q}");
        if lines.is_empty() {
            out.push_str(" ()");
        } else {
            let _ = write!(out, "\n(\n    {}\n)", lines.join(",\n    "));
        }
        if let Some(key) = &p.partition_key {
            let _ = write!(out, " PARTITION BY {key}");
        }
        if let Some(server) = &p.foreign_server {
            let _ = write!(out, " SERVER {server}");
        }
        out.push_str(";\n");
    }
    let section = |out: &mut String, items: Vec<String>| {
        if !items.is_empty() {
            out.push('\n');
            for i in items {
                out.push_str(&i);
                out.push('\n');
            }
        }
    };
    // A partitioned table's indexes render as `ON ONLY` (pg_dump's form,
    // which leaves the index unattached); the DDL a user would write has none.
    let indexes = p
        .indexes
        .iter()
        .map(|i| {
            let def = if p.relkind == "p" {
                i.definition.replacen(" ON ONLY ", " ON ", 1)
            } else {
                i.definition.clone()
            };
            statement(&def)
        })
        .collect();
    section(&mut out, indexes);
    section(&mut out, p.triggers.iter().map(|t| statement(t)).collect());
    let mut comments = Vec::new();
    if let Some(c) = &p.comment {
        comments.push(format!("COMMENT ON {what} {q} IS {c};"));
    }
    for col in &p.columns {
        if let Some(c) = &col.comment {
            comments.push(format!("COMMENT ON COLUMN {q}.{} IS {c};", col.name));
        }
    }
    for con in &p.constraints {
        if let Some(c) = &con.comment {
            comments.push(format!("COMMENT ON CONSTRAINT {} ON {q} IS {c};", con.name));
        }
    }
    for ix in &p.indexes {
        if let Some(c) = &ix.comment {
            comments.push(format!("COMMENT ON INDEX {} IS {c};", ix.name));
        }
    }
    section(&mut out, comments);
    out
}

impl Pg {
    /// Indexes and constraints of the picked schemas' tables, put on
    /// `schemas` (already read by `RELATIONS`). Both queries are sent at
    /// once on the session's connection.
    pub(super) async fn introspect_keys(
        client: &tokio_postgres::Client,
        only: &[String],
        schemas: &mut [Schema],
    ) -> CmdResult<()> {
        let (indexes, constraints) = futures_util::future::try_join(
            client.query(INDEXES, &[&only]),
            client.query(CONSTRAINTS, &[&only]),
        )
        .await?;
        attach(
            schemas,
            indexes.iter().map(index_row),
            constraints.iter().map(constraint_row),
        );
        Ok(())
    }

    /// DDL of one table / view / materialized view / foreign table, as the
    /// server describes it.
    pub async fn table_ddl(
        &self,
        id: &str,
        database: &str,
        schema: &str,
        table: &str,
    ) -> CmdResult<String> {
        let s = self.session(id, database).await?;
        let c = &s.client;
        let rel = c
            .query_opt(DDL_RELATION, &[&schema, &table])
            .await?
            .ok_or_else(|| CmdError::msg(format!("{schema}.{table} not found")))?;
        let oid: u32 = rel.get(0);
        let (columns, constraints, indexes, triggers) = futures_util::future::try_join4(
            c.query(DDL_COLUMNS, &[&oid]),
            c.query(DDL_CONSTRAINTS, &[&oid]),
            c.query(DDL_INDEXES, &[&oid]),
            c.query(DDL_TRIGGERS, &[&oid]),
        )
        .await?;
        let item = |r: &Row, name: usize, def: usize| DdlItem {
            name: r.get(name),
            definition: r.get::<_, Option<String>>(def).unwrap_or_default(),
            comment: r.get(2),
        };
        Ok(build_ddl(&DdlParts {
            relkind: rel.get(1),
            persistence: rel.get(2),
            qualified: rel.get(3),
            view_def: rel.get(4),
            partition_key: rel.get(5),
            foreign_server: rel.get(6),
            comment: rel.get(7),
            columns: columns
                .iter()
                .map(|r| DdlColumn {
                    name: r.get(0),
                    ty: r.get(1),
                    not_null: r.get(2),
                    default: r.get(3),
                    identity: r.get(4),
                    generated: r.get(5),
                    comment: r.get(6),
                })
                .collect(),
            constraints: constraints.iter().map(|r| item(r, 0, 1)).collect(),
            indexes: indexes.iter().map(|r| item(r, 1, 0)).collect(),
            triggers: triggers
                .iter()
                .filter_map(|r| r.get::<_, Option<String>>(0))
                .collect(),
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn col(name: &str, ty: &str) -> DdlColumn {
        DdlColumn {
            name: name.into(),
            ty: ty.into(),
            ..DdlColumn::default()
        }
    }

    fn item(name: &str, definition: &str) -> DdlItem {
        DdlItem {
            name: name.into(),
            definition: definition.into(),
            comment: None,
        }
    }

    #[test]
    fn column_lines() {
        let mut c = col("id", "bigint");
        c.not_null = true;
        c.identity = "d".into();
        assert_eq!(
            column_line(&c),
            "id bigint GENERATED BY DEFAULT AS IDENTITY NOT NULL"
        );
        c.identity = "a".into();
        assert_eq!(
            column_line(&c),
            "id bigint GENERATED ALWAYS AS IDENTITY NOT NULL"
        );

        let mut c = col("total", "numeric(10,2)");
        c.default = Some("(price * qty)".into());
        c.generated = "s".into();
        assert_eq!(
            column_line(&c),
            "total numeric(10,2) GENERATED ALWAYS AS ((price * qty)) STORED"
        );
        c.generated = "v".into();
        assert!(column_line(&c).ends_with(" VIRTUAL"));

        let mut c = col("\"At\"", "timestamp with time zone");
        c.default = Some("now()".into());
        c.not_null = true;
        assert_eq!(
            column_line(&c),
            "\"At\" timestamp with time zone DEFAULT now() NOT NULL"
        );
        assert_eq!(column_line(&col("note", "text")), "note text");
    }

    #[test]
    fn table_ddl() {
        let mut id = col("id", "bigint");
        id.not_null = true;
        id.identity = "d".into();
        let mut email = col("email", "text");
        email.not_null = true;
        email.comment = Some("'Login, it''s unique'".into());
        let mut pkey = item("users_pkey", "PRIMARY KEY (id)");
        pkey.comment = Some("'the key'".into());
        let mut by_org = item(
            "public.users_org_idx",
            "CREATE INDEX users_org_idx ON public.users USING btree (org_id)",
        );
        by_org.comment = Some("'lookups'".into());
        let p = DdlParts {
            qualified: "public.users".into(),
            relkind: "r".into(),
            persistence: "p".into(),
            comment: Some("'People'".into()),
            columns: vec![id, email, col("org_id", "bigint")],
            constraints: vec![
                pkey,
                item(
                    "users_org_id_fkey",
                    "FOREIGN KEY (org_id) REFERENCES organizations(id) ON DELETE CASCADE",
                ),
            ],
            indexes: vec![
                by_org,
                item(
                    "public.users_lower_idx",
                    "CREATE INDEX users_lower_idx ON public.users USING btree (lower(email));",
                ),
            ],
            triggers: vec![
                "CREATE TRIGGER touch BEFORE UPDATE ON users FOR EACH ROW EXECUTE FUNCTION touch()"
                    .into(),
            ],
            ..DdlParts::default()
        };
        assert_eq!(
            build_ddl(&p),
            "CREATE TABLE public.users
(
    id bigint GENERATED BY DEFAULT AS IDENTITY NOT NULL,
    email text NOT NULL,
    org_id bigint,
    CONSTRAINT users_pkey PRIMARY KEY (id),
    CONSTRAINT users_org_id_fkey FOREIGN KEY (org_id) REFERENCES organizations(id) ON DELETE CASCADE
);

CREATE INDEX users_org_idx ON public.users USING btree (org_id);
CREATE INDEX users_lower_idx ON public.users USING btree (lower(email));

CREATE TRIGGER touch BEFORE UPDATE ON users FOR EACH ROW EXECUTE FUNCTION touch();

COMMENT ON TABLE public.users IS 'People';
COMMENT ON COLUMN public.users.email IS 'Login, it''s unique';
COMMENT ON CONSTRAINT users_pkey ON public.users IS 'the key';
COMMENT ON INDEX public.users_org_idx IS 'lookups';
"
        );
    }

    #[test]
    fn view_and_matview_ddl() {
        let p = DdlParts {
            qualified: "public.active_users".into(),
            relkind: "v".into(),
            view_def: Some(" SELECT id,\n    email\n   FROM users\n  WHERE active;".into()),
            columns: vec![col("id", "bigint")],
            comment: Some("'recent'".into()),
            ..DdlParts::default()
        };
        assert_eq!(
            build_ddl(&p),
            "CREATE VIEW public.active_users AS\n SELECT id,\n    email\n   FROM users\n  WHERE active;\n\nCOMMENT ON VIEW public.active_users IS 'recent';\n"
        );
        let p = DdlParts {
            qualified: "public.daily".into(),
            relkind: "m".into(),
            view_def: Some(" SELECT 1 AS x".into()),
            indexes: vec![item(
                "public.daily_x",
                "CREATE UNIQUE INDEX daily_x ON public.daily USING btree (x)",
            )],
            ..DdlParts::default()
        };
        assert_eq!(
            build_ddl(&p),
            "CREATE MATERIALIZED VIEW public.daily AS\n SELECT 1 AS x;\n\nCREATE UNIQUE INDEX daily_x ON public.daily USING btree (x);\n"
        );
    }

    #[test]
    fn partitioned_unlogged_foreign_and_empty() {
        let p = DdlParts {
            qualified: "public.events".into(),
            relkind: "p".into(),
            partition_key: Some("RANGE (at)".into()),
            columns: vec![col("at", "date")],
            indexes: vec![item(
                "public.events_at",
                "CREATE INDEX events_at ON ONLY public.events USING btree (at)",
            )],
            ..DdlParts::default()
        };
        assert_eq!(
            build_ddl(&p),
            "CREATE TABLE public.events\n(\n    at date\n) PARTITION BY RANGE (at);\n\nCREATE INDEX events_at ON public.events USING btree (at);\n"
        );
        let p = DdlParts {
            qualified: "public.scratch".into(),
            relkind: "r".into(),
            persistence: "u".into(),
            ..DdlParts::default()
        };
        assert_eq!(build_ddl(&p), "CREATE UNLOGGED TABLE public.scratch ();\n");
        let p = DdlParts {
            qualified: "public.remote".into(),
            relkind: "f".into(),
            foreign_server: Some("files".into()),
            columns: vec![col("line", "text")],
            ..DdlParts::default()
        };
        assert_eq!(
            build_ddl(&p),
            "CREATE FOREIGN TABLE public.remote\n(\n    line text\n) SERVER files;\n"
        );
    }

    fn table(name: &str) -> Table {
        Table {
            name: name.into(),
            kind: "table",
            rows: 0,
            size_bytes: 0,
            columns: Vec::new(),
            indexes: Vec::new(),
            constraints: Vec::new(),
        }
    }

    #[test]
    fn attaches_to_the_right_tables() {
        let mut schemas = vec![
            Schema {
                name: "public".into(),
                tables: vec![table("users"), table("orgs")],
            },
            Schema {
                name: "audit".into(),
                tables: vec![table("users")],
            },
        ];
        let key = |s: &str, t: &str| (s.to_owned(), t.to_owned());
        let ix = |name: &str| Index {
            name: name.into(),
            columns: vec!["id".into()],
            unique: true,
            primary: true,
            definition: String::new(),
        };
        let fk = Constraint {
            name: "users_org_fkey".into(),
            kind: "f".into(),
            definition: "FOREIGN KEY (org) REFERENCES orgs(id)".into(),
            columns: vec!["org".into()],
            references: Some(ForeignRef {
                schema: "public".into(),
                table: "orgs".into(),
                columns: vec!["id".into()],
            }),
        };
        attach(
            &mut schemas,
            vec![
                (key("public", "users"), ix("users_pkey")),
                (key("audit", "users"), ix("audit_users_pkey")),
                (key("public", "gone"), ix("gone_pkey")),
            ],
            vec![(key("public", "users"), fk.clone())],
        );
        assert_eq!(schemas[0].tables[0].indexes, vec![ix("users_pkey")]);
        assert_eq!(schemas[0].tables[0].constraints, vec![fk]);
        assert!(schemas[0].tables[1].indexes.is_empty());
        assert_eq!(schemas[1].tables[0].indexes, vec![ix("audit_users_pkey")]);
        assert!(schemas[1].tables[0].constraints.is_empty());

        let json = serde_json::to_value(&schemas[0].tables[0].constraints[0]).unwrap();
        assert_eq!(json["references"]["table"], "orgs");
        let json = serde_json::to_value(&schemas[0].tables[0].indexes[0]).unwrap();
        assert!(json.get("references").is_none() && json["primary"] == true);
    }
}
