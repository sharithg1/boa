//! Postgres sessions, introspection and query execution.
//!
//! A connection (`ConnConfig`) is opened once with `connect`; after that one
//! session per database is opened on demand (Postgres connections are bound
//! to a database). Statements run over the simple-query protocol, so every
//! value comes back as the server's own text rendering (what psql shows),
//! with column types taken from a `prepare` of the same statement.
//! Row-returning statements go through a cursor so only one page (`limit`
//! rows after skipping `offset`) ever leaves the server. Pages are stateless:
//! each one re-runs the statement, like DataGrip's LIMIT/OFFSET paging.

use crate::connections::{ConnConfig, SslMode};
use crate::error::{CmdError, CmdResult};
use crate::tx::{self, TxCell, TxState};
use postgres_native_tls::MakeTlsConnector;
use serde::Serialize;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use tokio::sync::Mutex;
use tokio_postgres::{CancelToken, Client, SimpleQueryMessage};

struct Session {
    client: Client,
    cancel: CancelToken,
    tls: MakeTlsConnector,
    /// Transaction state after the last statement (probed from the server):
    /// inside a block, the cursor doesn't open its own transaction.
    tx: TxCell,
    /// Manual transaction mode: a BEGIN is sent before the first statement.
    manual: AtomicBool,
    /// Opened with `default_transaction_read_only = on`.
    read_only: bool,
}

struct Open {
    config: ConnConfig,
    password: Option<String>,
}

/// Tauri state: open connections and their per-database sessions.
#[derive(Default)]
pub struct Pg {
    open: Mutex<HashMap<String, Arc<Open>>>,
    sessions: Mutex<HashMap<(String, String), Arc<Session>>>,
}

impl std::fmt::Debug for Pg {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Pg")
    }
}

fn tls(mode: SslMode) -> CmdResult<MakeTlsConnector> {
    let verify = mode == SslMode::VerifyFull;
    let connector = native_tls::TlsConnector::builder()
        .danger_accept_invalid_certs(!verify)
        .danger_accept_invalid_hostnames(!verify)
        .build()?;
    Ok(MakeTlsConnector::new(connector))
}

async fn open_session(
    config: &ConnConfig,
    password: Option<&str>,
    database: &str,
) -> CmdResult<Session> {
    let mut c = tokio_postgres::Config::new();
    c.host(&config.host)
        .port(config.port)
        .user(&config.user)
        .dbname(database)
        .application_name("dbclient")
        .connect_timeout(Duration::from_secs(10))
        .ssl_mode(match config.ssl_mode {
            SslMode::Disable => tokio_postgres::config::SslMode::Disable,
            SslMode::Prefer => tokio_postgres::config::SslMode::Prefer,
            SslMode::Require | SslMode::VerifyFull => tokio_postgres::config::SslMode::Require,
        });
    if let Some(p) = password {
        c.password(p);
    }
    if config.read_only {
        c.options("-c default_transaction_read_only=on");
    }
    let tls = tls(config.ssl_mode)?;
    let (client, connection) = c.connect(tls.clone()).await?;
    let label = format!("{}/{database}", config.name);
    tauri::async_runtime::spawn(async move {
        if let Err(e) = connection.await {
            tracing::warn!("connection {label} closed: {e}");
        }
    });
    Ok(Session {
        cancel: client.cancel_token(),
        client,
        tls,
        tx: TxCell::default(),
        manual: AtomicBool::new(false),
        read_only: config.read_only,
    })
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectInfo {
    pub server_version: String,
    pub databases: Vec<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Column {
    pub name: String,
    #[serde(rename = "type")]
    pub ty: String,
    pub nullable: bool,
    pub pk: bool,
    pub default: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Table {
    pub name: String,
    /// `table` or `view` (materialized views count as views).
    pub kind: &'static str,
    /// Planner estimate.
    pub rows: i64,
    pub size_bytes: i64,
    pub columns: Vec<Column>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Schema {
    pub name: String,
    pub tables: Vec<Table>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResultColumn {
    pub name: String,
    #[serde(rename = "type")]
    pub ty: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QueryResult {
    pub columns: Vec<ResultColumn>,
    pub rows: Vec<Vec<Option<String>>>,
    /// Rows affected (or returned) as the server reported it.
    pub affected: Option<u64>,
    /// More rows existed after this page.
    pub truncated: bool,
    /// Read-only cursor statement: other pages can be fetched by re-running
    /// it with an offset. False for DML (`INSERT … RETURNING`), which must
    /// never be re-run.
    pub pageable: bool,
    pub ms: u64,
    /// Transaction state after the statement.
    pub tx: TxState,
}

const SERVER_VERSION: &str = "select 'PostgreSQL ' || current_setting('server_version')";

const DATABASES: &str = "select datname::text from pg_database \
     where not datistemplate and datallowconn order by datname";

const SCHEMAS: &str = "select nspname::text from pg_namespace \
     where nspname not in ('information_schema') and nspname not like 'pg\\_%' \
     order by nspname = 'public' desc, nspname";

const RELATIONS: &str = r"
select n.nspname::text, c.relname::text, c.relkind::text,
       greatest(c.reltuples, 0)::int8,
       case when c.relkind in ('r', 'm', 'p') then pg_total_relation_size(c.oid) else 0 end::int8,
       a.attname::text,
       format_type(a.atttypid, a.atttypmod),
       not a.attnotnull,
       pg_get_expr(d.adbin, d.adrelid),
       coalesce(a.attnum = any(i.indkey), false)
from pg_class c
join pg_namespace n on n.oid = c.relnamespace
left join pg_attribute a on a.attrelid = c.oid and a.attnum > 0 and not a.attisdropped
left join pg_attrdef d on d.adrelid = c.oid and d.adnum = a.attnum
left join pg_index i on i.indrelid = c.oid and i.indisprimary
where c.relkind in ('r', 'v', 'm', 'p', 'f')
  and n.nspname <> 'information_schema' and n.nspname not like 'pg\_%'
  and not c.relispartition
  and n.nspname = any($1)
order by n.nspname, c.relname, a.attnum";

/// First keyword of a statement, lower-cased, skipping comments and parentheses.
fn leading_keyword(sql: &str) -> String {
    let mut s = sql.trim_start();
    loop {
        if let Some(rest) = s.strip_prefix("--") {
            s = rest.split_once('\n').map_or("", |(_, r)| r).trim_start();
        } else if let Some(rest) = s.strip_prefix("/*") {
            s = rest.split_once("*/").map_or("", |(_, r)| r).trim_start();
        } else if let Some(rest) = s.strip_prefix('(') {
            s = rest.trim_start();
        } else {
            break;
        }
    }
    s.chars()
        .take_while(|c| c.is_ascii_alphabetic())
        .collect::<String>()
        .to_ascii_lowercase()
}

impl Pg {
    /// Opens the connection's default database, remembering the credentials
    /// for the other databases' sessions.
    pub async fn connect(
        &self,
        config: ConnConfig,
        password: Option<String>,
    ) -> CmdResult<ConnectInfo> {
        self.disconnect(&config.id).await;
        let session = open_session(&config, password.as_deref(), &config.database).await?;
        let server_version = session.client.query_one(SERVER_VERSION, &[]).await?.get(0);
        let databases = session
            .client
            .query(DATABASES, &[])
            .await?
            .iter()
            .map(|r| r.get(0))
            .collect();
        let id = config.id.clone();
        let database = config.database.clone();
        self.sessions
            .lock()
            .await
            .insert((id.clone(), database), Arc::new(session));
        self.open
            .lock()
            .await
            .insert(id, Arc::new(Open { config, password }));
        Ok(ConnectInfo {
            server_version,
            databases,
        })
    }

    /// Connects, reads the version and disconnects.
    pub async fn test(config: &ConnConfig, password: Option<&str>) -> CmdResult<String> {
        let session = open_session(config, password, &config.database).await?;
        Ok(session.client.query_one(SERVER_VERSION, &[]).await?.get(0))
    }

    pub async fn disconnect(&self, id: &str) {
        self.open.lock().await.remove(id);
        let mut gone = Vec::new();
        self.sessions.lock().await.retain(|(cid, _), s| {
            if cid == id {
                gone.push(s.clone());
            }
            cid != id
        });
        // Roll open transactions back explicitly (the server would anyway
        // once the connection closes), without waiting on a busy session.
        for s in gone {
            if s.tx.get() != TxState::Idle {
                let _ = tokio::time::timeout(
                    Duration::from_secs(2),
                    s.client.batch_execute("rollback"),
                )
                .await;
            }
        }
        // Dropping the clients closes their connections.
    }

    /// Switches the session to Manual (`true`) or Auto transaction mode.
    /// An open transaction is left as it is.
    pub async fn set_tx_mode(&self, id: &str, database: &str, manual: bool) -> CmdResult<TxState> {
        let s = self.session(id, database).await?;
        s.manual.store(manual, Ordering::Relaxed);
        Ok(s.tx.get())
    }

    async fn session(&self, id: &str, database: &str) -> CmdResult<Arc<Session>> {
        let key = (id.to_owned(), database.to_owned());
        // Held across the connect so two callers don't open the same session twice.
        let mut sessions = self.sessions.lock().await;
        let mut manual = false;
        if let Some(s) = sessions.get(&key) {
            if !s.client.is_closed() {
                return Ok(s.clone());
            }
            // Reopened after the connection dropped: same mode, no transaction.
            manual = s.manual.load(Ordering::Relaxed);
        }
        let open = self
            .open
            .lock()
            .await
            .get(id)
            .cloned()
            .ok_or_else(|| CmdError::code("not_connected", "Not connected"))?;
        let s = Arc::new(open_session(&open.config, open.password.as_deref(), database).await?);
        s.manual.store(manual, Ordering::Relaxed);
        sessions.insert(key, s.clone());
        Ok(s)
    }

    /// Names of the database's schemas (cheap: for the schema picker).
    pub async fn list_schemas(&self, id: &str, database: &str) -> CmdResult<Vec<String>> {
        let s = self.session(id, database).await?;
        Ok(s.client
            .query(SCHEMAS, &[])
            .await?
            .iter()
            .map(|r| r.get(0))
            .collect())
    }

    /// Tables, views and columns of the given schemas only (those that exist).
    pub async fn introspect(
        &self,
        id: &str,
        database: &str,
        only: &[String],
    ) -> CmdResult<Vec<Schema>> {
        let s = self.session(id, database).await?;
        let mut schemas: Vec<Schema> = s
            .client
            .query(SCHEMAS, &[])
            .await?
            .iter()
            .map(|r| r.get::<_, String>(0))
            .filter(|name| only.contains(name))
            .map(|name| Schema {
                name,
                tables: Vec::new(),
            })
            .collect();
        if schemas.is_empty() {
            return Ok(schemas);
        }
        for r in s.client.query(RELATIONS, &[&only]).await? {
            let schema: String = r.get(0);
            let table: String = r.get(1);
            let Some(sc) = schemas.iter_mut().find(|x| x.name == schema) else {
                continue;
            };
            if sc.tables.last().is_none_or(|t| t.name != table) {
                let kind: String = r.get(2);
                sc.tables.push(Table {
                    name: table,
                    kind: if kind == "v" || kind == "m" {
                        "view"
                    } else {
                        "table"
                    },
                    rows: r.get(3),
                    size_bytes: r.get(4),
                    columns: Vec::new(),
                });
            }
            let column: Option<String> = r.get(5);
            if let (Some(name), Some(t)) = (column, sc.tables.last_mut()) {
                t.columns.push(Column {
                    name,
                    ty: r.get(6),
                    nullable: r.get(7),
                    pk: r.get(9),
                    default: r.get(8),
                });
            }
        }
        Ok(schemas)
    }

    /// Runs one statement; row-returning ones return `limit` rows starting
    /// after the first `offset`. Only pageable statements accept an offset.
    /// In Manual mode a transaction is opened first; either way the result
    /// (or error) carries the transaction state the server is left in.
    pub async fn query(
        &self,
        id: &str,
        database: &str,
        sql: &str,
        limit: usize,
        offset: usize,
    ) -> CmdResult<QueryResult> {
        let s = self.session(id, database).await?;
        let text = sql.trim().trim_end_matches(';').trim_end();
        if s.manual.load(Ordering::Relaxed)
            && s.tx.get() == TxState::Idle
            && offset == 0
            && !text.is_empty()
            && tx::opens_tx(&leading_keyword(text), text)
        {
            s.client.batch_execute("begin").await?;
            s.tx.set(TxState::Active);
        }
        let run = self.execute(&s, sql, limit, offset).await;
        let probe = tx::probe(&s.client).await;
        s.tx.set(probe.state);
        let reverted = if s.read_only && probe.writable {
            Some(restore_read_only(&s).await)
        } else {
            None
        };
        match (run, reverted) {
            (Ok(_), Some(e)) => Err(e),
            (Ok(mut r), None) => {
                r.tx = s.tx.get();
                Ok(r)
            }
            (Err(mut e), _) => {
                if s.read_only && e.code.as_deref() == Some("25006") {
                    e.hint = Some("The connection is read-only (data source settings).".into());
                }
                e.tx = Some(s.tx.get());
                Err(e)
            }
        }
    }

    async fn execute(
        &self,
        s: &Session,
        sql: &str,
        limit: usize,
        offset: usize,
    ) -> CmdResult<QueryResult> {
        let started = Instant::now();
        let sql = sql.trim().trim_end_matches(';').trim_end();
        if sql.is_empty() {
            return Err(CmdError::msg("Empty statement"));
        }
        // Column types (the simple protocol only gives names). Statements that
        // can't be prepared (several at once, utility commands with $ quoting…)
        // just run without types; real errors surface from the run itself.
        let typed = s.client.prepare(sql).await.ok().map(|st| {
            st.columns()
                .iter()
                .map(|c| ResultColumn {
                    name: c.name().to_owned(),
                    ty: c.type_().name().to_owned(),
                })
                .collect::<Vec<_>>()
        });
        let kw = leading_keyword(sql);
        let cursorable = matches!(kw.as_str(), "select" | "with" | "values" | "table")
            && typed.as_ref().is_some_and(|c| !c.is_empty());

        let mut result = if cursorable {
            match self.fetch_with_cursor(s, sql, limit, offset).await {
                Ok(r) => r,
                // `WITH … INSERT` can't be a cursor (0A000): run it plainly,
                // but never re-run it for another page.
                Err(e) if e.code.as_deref() == Some("0A000") && offset == 0 => {
                    run_simple(s, sql, limit).await?
                }
                Err(e) => return Err(e),
            }
        } else if offset > 0 {
            return Err(CmdError::msg("This statement can't be paged"));
        } else {
            run_simple(s, sql, limit).await?
        };
        if let Some(typed) = typed {
            if typed.len() == result.columns.len() {
                result.columns = typed;
            }
        }
        result.ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
        Ok(result)
    }

    async fn fetch_with_cursor(
        &self,
        s: &Session,
        sql: &str,
        limit: usize,
        offset: usize,
    ) -> CmdResult<QueryResult> {
        let own_tx = s.tx.get() == TxState::Idle;
        if own_tx {
            s.client.batch_execute("begin").await?;
        }
        let run = async {
            s.client
                .batch_execute(&format!("declare __dbclient no scroll cursor for {sql}"))
                .await?;
            // Same work on the server as OFFSET, without rewriting the SQL.
            if offset > 0 {
                s.client
                    .batch_execute(&format!("move forward {offset} in __dbclient"))
                    .await?;
            }
            let mut r = run_simple(
                s,
                &format!("fetch forward {} from __dbclient", limit + 1),
                limit,
            )
            .await?;
            s.client.batch_execute("close __dbclient").await?;
            r.pageable = true;
            Ok::<_, CmdError>(r)
        };
        match run.await {
            Ok(r) => {
                if own_tx {
                    s.client.batch_execute("commit").await?;
                }
                Ok(r)
            }
            Err(e) => {
                // Leave the session usable: our own transaction is rolled
                // back; the user's stays aborted, as psql would leave it.
                if own_tx {
                    let _ = s.client.batch_execute("rollback").await;
                }
                Err(e)
            }
        }
    }

    pub async fn cancel(&self, id: &str, database: &str) -> CmdResult<()> {
        let s = self
            .sessions
            .lock()
            .await
            .get(&(id.to_owned(), database.to_owned()))
            .cloned();
        if let Some(s) = s {
            s.cancel.cancel_query(s.tls.clone()).await?;
        }
        Ok(())
    }
}

/// Runs `sql` over the simple protocol, keeping the last result set (at most `limit` rows).
async fn run_simple(s: &Session, sql: &str, limit: usize) -> CmdResult<QueryResult> {
    let mut columns: Vec<ResultColumn> = Vec::new();
    let mut rows: Vec<Vec<Option<String>>> = Vec::new();
    let mut affected = None;
    let mut truncated = false;
    for msg in s.client.simple_query(sql).await? {
        match msg {
            SimpleQueryMessage::RowDescription(cols) => {
                columns = cols
                    .iter()
                    .map(|c| ResultColumn {
                        name: c.name().to_owned(),
                        ty: String::new(),
                    })
                    .collect();
                rows.clear();
                truncated = false;
            }
            SimpleQueryMessage::Row(row) => {
                if rows.len() >= limit {
                    truncated = true;
                    continue;
                }
                rows.push(
                    (0..row.len())
                        .map(|i| row.get(i).map(str::to_owned))
                        .collect(),
                );
            }
            SimpleQueryMessage::CommandComplete(n) => affected = Some(n),
            _ => {}
        }
    }
    Ok(QueryResult {
        columns,
        rows,
        affected,
        truncated,
        pageable: false,
        ms: 0,
        tx: TxState::Idle,
    })
}

/// A read-only session ended up able to write: roll back the read-write
/// transaction, if any, and turn the default back on.
async fn restore_read_only(s: &Session) -> CmdError {
    let rolled_back = s.tx.get() != TxState::Idle;
    if rolled_back {
        let _ = s.client.batch_execute("rollback").await;
    }
    let _ = s
        .client
        .batch_execute("set default_transaction_read_only = on")
        .await;
    s.tx.set(tx::probe(&s.client).await.state);
    CmdError {
        tx: Some(s.tx.get()),
        hint: Some("Turn read-only off in the data source settings to write.".into()),
        ..CmdError::code(
            "read_only",
            if rolled_back {
                "The connection is read-only: the read-write transaction was rolled back"
            } else {
                "The connection is read-only: read-write mode was turned back off"
            },
        )
    }
}

#[cfg(test)]
mod tests {
    use super::{Pg, leading_keyword};
    use crate::connections::{ConnConfig, SslMode};

    #[test]
    fn keyword_skips_comments_and_parens() {
        assert_eq!(leading_keyword("  -- hi\n/* x */ (SELECT 1)"), "select");
        assert_eq!(
            leading_keyword("WITH a as (select 1) select * from a"),
            "with"
        );
        assert_eq!(leading_keyword("insert into t values (1)"), "insert");
        assert_eq!(leading_keyword(""), "");
    }

    /// Against a real server, configured with the libpq variables (PGHOST,
    /// PGPORT, PGUSER, PGPASSWORD, PGDATABASE). Skipped when PGHOST is unset.
    /// Creates and drops a `dbclient_test` schema.
    #[tokio::test(flavor = "multi_thread")]
    async fn live_postgres() {
        let Ok(host) = std::env::var("PGHOST") else {
            eprintln!("PGHOST not set: skipping live Postgres test");
            return;
        };
        let env = |k: &str, d: &str| std::env::var(k).unwrap_or_else(|_| d.to_owned());
        let config = ConnConfig {
            id: "test".into(),
            name: "test".into(),
            host,
            port: env("PGPORT", "5432").parse().unwrap(),
            user: env("PGUSER", "postgres"),
            database: env("PGDATABASE", "postgres"),
            ssl_mode: SslMode::Prefer,
            read_only: false,
            environment: None,
            color: None,
        };
        let db = config.database.clone();
        let pg = Pg::default();
        let info = pg
            .connect(config.clone(), std::env::var("PGPASSWORD").ok())
            .await
            .unwrap();
        assert!(info.server_version.starts_with("PostgreSQL"));
        assert!(info.databases.contains(&db));

        let q = |sql: &'static str, limit: usize| pg.query("test", &db, sql, limit, 0);
        q("drop schema if exists dbclient_test cascade", 10)
            .await
            .unwrap();
        q("create schema dbclient_test", 10).await.unwrap();
        q(r#"create table dbclient_test."Mixed" (id int primary key, note text, at timestamptz default now())"#, 10)
            .await
            .unwrap();
        let ins = q(r#"insert into dbclient_test."Mixed" (id, note) select g, 'n' || g from generate_series(1, 25) g"#, 10)
            .await
            .unwrap();
        assert_eq!(ins.affected, Some(25));
        assert!(ins.columns.is_empty());

        // Row cap through the cursor, with types from prepare and text values.
        let r = q(
            r#"select id, note, null::int as nothing from dbclient_test."Mixed" order by id;"#,
            10,
        )
        .await
        .unwrap();
        assert_eq!(r.rows.len(), 10);
        assert!(r.truncated);
        assert_eq!(r.columns[0].name, "id");
        assert_eq!(r.columns[0].ty, "int4");
        assert_eq!(r.rows[0], vec![Some("1".into()), Some("n1".into()), None]);
        assert!(r.pageable);

        // Later pages re-run the statement with an offset.
        let page = |offset| {
            pg.query(
                "test",
                &db,
                r#"select id from dbclient_test."Mixed" order by id"#,
                10,
                offset,
            )
        };
        let p2 = page(10).await.unwrap();
        assert_eq!(p2.rows.first().unwrap()[0].as_deref(), Some("11"));
        assert!(p2.truncated);
        let p3 = page(20).await.unwrap();
        assert_eq!(p3.rows.len(), 5);
        assert!(!p3.truncated);
        assert!(page(30).await.unwrap().rows.is_empty());

        // INSERT … RETURNING: not a cursor, still returns rows.
        let r = q(
            r#"insert into dbclient_test."Mixed" (id) values (100) returning id"#,
            10,
        )
        .await
        .unwrap();
        assert_eq!(r.rows, vec![vec![Some("100".into())]]);
        assert!(!r.pageable);
        // …and is never re-run for another page.
        assert!(
            pg.query(
                "test",
                &db,
                r#"insert into dbclient_test."Mixed" (id) values (101) returning id"#,
                10,
                10
            )
            .await
            .is_err()
        );

        // Server errors carry SQLSTATE and position; the session stays usable.
        let e = q("select * from dbclient_test.nope", 10).await.unwrap_err();
        assert_eq!(e.code.as_deref(), Some("42P01"));
        assert_eq!(e.position, Some(15));
        assert_eq!(
            q("select 1", 10).await.unwrap().rows,
            vec![vec![Some("1".into())]]
        );

        // Explicit transaction: our cursor must not commit it.
        q("begin", 10).await.unwrap();
        q(r#"delete from dbclient_test."Mixed""#, 10).await.unwrap();
        assert_eq!(
            q(r#"select count(*) from dbclient_test."Mixed""#, 10)
                .await
                .unwrap()
                .rows[0][0]
                .as_deref(),
            Some("0")
        );
        q("rollback", 10).await.unwrap();
        assert_eq!(
            q(r#"select count(*) from dbclient_test."Mixed""#, 10)
                .await
                .unwrap()
                .rows[0][0]
                .as_deref(),
            Some("26")
        );

        // The state comes from the server: typed BEGIN, an error inside the
        // block (25P02 until rollback), COMMIT of an aborted block.
        use crate::tx::TxState;
        assert_eq!(q("begin", 10).await.unwrap().tx, TxState::Active);
        let e = q("select * from dbclient_test.nope", 10).await.unwrap_err();
        assert_eq!(e.tx, Some(TxState::Failed));
        let e = q("select 1", 10).await.unwrap_err();
        assert_eq!(e.code.as_deref(), Some("25P02"));
        assert_eq!(e.tx, Some(TxState::Failed));
        assert_eq!(q("commit", 10).await.unwrap().tx, TxState::Idle);
        assert_eq!(q("select 1", 10).await.unwrap().tx, TxState::Idle);

        // Manual mode: the first statement opens a transaction, pages of a
        // cursor run inside it, and ROLLBACK undoes the work.
        assert_eq!(
            pg.set_tx_mode("test", &db, true).await.unwrap(),
            TxState::Idle
        );
        let r = q(r#"delete from dbclient_test."Mixed" where id > 20"#, 10)
            .await
            .unwrap();
        assert_eq!(r.tx, TxState::Active);
        let p = pg
            .query(
                "test",
                &db,
                r#"select id from dbclient_test."Mixed" order by id"#,
                5,
                5,
            )
            .await
            .unwrap();
        assert_eq!(p.rows.len(), 5);
        assert_eq!(p.tx, TxState::Active);
        assert_eq!(q("rollback", 10).await.unwrap().tx, TxState::Idle);
        pg.set_tx_mode("test", &db, false).await.unwrap();
        assert_eq!(
            q(r#"select count(*) from dbclient_test."Mixed""#, 10)
                .await
                .unwrap()
                .rows[0][0]
                .as_deref(),
            Some("26")
        );

        let schemas = pg
            .introspect("test", &db, &["dbclient_test".to_owned()])
            .await
            .unwrap();
        assert_eq!(schemas.len(), 1);
        assert!(
            pg.list_schemas("test", &db)
                .await
                .unwrap()
                .contains(&"dbclient_test".to_owned())
        );
        let t = &schemas
            .iter()
            .find(|s| s.name == "dbclient_test")
            .unwrap()
            .tables[0];
        assert_eq!(t.name, "Mixed");
        assert_eq!(t.columns.len(), 3);
        assert!(t.columns[0].pk && !t.columns[0].nullable);
        assert_eq!(t.columns[2].ty, "timestamp with time zone");
        assert_eq!(t.columns[2].default.as_deref(), Some("now()"));

        // Cancel a running statement.
        let slow = q("select pg_sleep(10)", 10);
        let cancel = async {
            tokio::time::sleep(std::time::Duration::from_millis(300)).await;
            pg.cancel("test", &db).await.unwrap();
        };
        let (r, ()) = tokio::join!(slow, cancel);
        assert_eq!(r.unwrap_err().code.as_deref(), Some("57014"));

        // Disconnecting rolls an open transaction back.
        q("begin", 10).await.unwrap();
        q(r#"delete from dbclient_test."Mixed""#, 10).await.unwrap();
        pg.disconnect("test").await;
        pg.connect(config.clone(), std::env::var("PGPASSWORD").ok())
            .await
            .unwrap();
        let count = q(r#"select count(*) from dbclient_test."Mixed""#, 10)
            .await
            .unwrap();
        assert_eq!(count.rows[0][0].as_deref(), Some("26"));
        pg.disconnect("test").await;

        // Read-only connections: enforced by the server, and not undone by
        // SET or BEGIN READ WRITE.
        let mut ro = config.clone();
        ro.read_only = true;
        pg.connect(ro, std::env::var("PGPASSWORD").ok())
            .await
            .unwrap();
        let e = q(r#"delete from dbclient_test."Mixed""#, 10)
            .await
            .unwrap_err();
        assert_eq!(e.code.as_deref(), Some("25006"));
        assert!(e.hint.is_some());
        let e = q("set default_transaction_read_only = off", 10)
            .await
            .unwrap_err();
        assert_eq!(e.code.as_deref(), Some("read_only"));
        let e = q("begin read write", 10).await.unwrap_err();
        assert_eq!(e.code.as_deref(), Some("read_only"));
        assert_eq!(e.tx, Some(TxState::Idle));
        let e = q(r#"delete from dbclient_test."Mixed""#, 10)
            .await
            .unwrap_err();
        assert_eq!(e.code.as_deref(), Some("25006"));
        pg.disconnect("test").await;

        pg.connect(config, std::env::var("PGPASSWORD").ok())
            .await
            .unwrap();
        q("drop schema dbclient_test cascade", 10).await.unwrap();
        pg.disconnect("test").await;
    }
}
