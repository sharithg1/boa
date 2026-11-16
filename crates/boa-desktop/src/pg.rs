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
use crate::ssh::Tunnel;
use crate::tx::{self, TxCell, TxState};
use postgres_native_tls::MakeTlsConnector;
use serde::Serialize;
use std::collections::HashMap;
use std::net::Ipv4Addr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, PoisonError};
use std::time::{Duration, Instant};
use tokio::sync::Mutex;
use tokio_postgres::{CancelToken, Client, SimpleQueryMessage};

mod catalog;
mod export;
use catalog::{Constraint, Index};
pub use export::{CsvOptions, ExportResult, write_file_atomic};

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
    /// SSH tunnels of open connections, shared by their sessions. Dropping
    /// one kills its ssh. A std mutex, never held across an await, so quitting
    /// can kill them synchronously.
    tunnels: std::sync::Mutex<HashMap<String, Tunnel>>,
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

/// `via`: the local port of the connection's SSH tunnel, connected to
/// instead of the host (which stays the TLS host name). The cancel token
/// keeps that address, so cancelling goes through the tunnel too.
async fn open_session(
    config: &ConnConfig,
    password: Option<&str>,
    database: &str,
    via: Option<u16>,
) -> CmdResult<Session> {
    let mut c = tokio_postgres::Config::new();
    if via.is_some() {
        c.hostaddr(Ipv4Addr::LOCALHOST.into());
    }
    c.host(&config.host)
        .port(via.unwrap_or(config.port))
        .user(&config.user)
        .dbname(database)
        .application_name("boa")
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
    /// Tables and materialized views: every index, primary key's included.
    pub indexes: Vec<Index>,
    /// Tables: primary key, unique, foreign key, check and exclusion constraints.
    pub constraints: Vec<Constraint>,
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
        let first = async {
            let via = self.tunnel_port(&config).await?;
            let session = open_session(&config, password.as_deref(), &config.database, via).await?;
            let server_version: String =
                session.client.query_one(SERVER_VERSION, &[]).await?.get(0);
            let databases: Vec<String> = session
                .client
                .query(DATABASES, &[])
                .await?
                .iter()
                .map(|r| r.get(0))
                .collect();
            Ok::<_, CmdError>((session, server_version, databases))
        };
        let (session, server_version, databases) = match first.await {
            Ok(x) => x,
            Err(e) => {
                return Err(match self.close_tunnel(&config.id) {
                    Some(t) => t.explain(e).await,
                    None => e,
                });
            }
        };
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

    /// Connects (through a tunnel of its own), reads the version and disconnects.
    pub async fn test(config: &ConnConfig, password: Option<&str>) -> CmdResult<String> {
        let tunnel = match config.tunnel() {
            Some(t) => Some(Tunnel::open(t, &config.host, config.port).await?),
            None => None,
        };
        let version = async {
            let via = tunnel.as_ref().map(|t| t.port);
            let session = open_session(config, password, &config.database, via).await?;
            Ok::<String, CmdError>(session.client.query_one(SERVER_VERSION, &[]).await?.get(0))
            // The session closes here.
        };
        match (version.await, &tunnel) {
            (Err(e), Some(t)) => Err(t.explain(e).await),
            (r, _) => r,
        }
        // Dropping the tunnel kills ssh.
    }

    /// The local port of the connection's SSH tunnel (none without one),
    /// starting ssh if it isn't running (first connect, or it exited).
    async fn tunnel_port(&self, config: &ConnConfig) -> CmdResult<Option<u16>> {
        let Some(ssh) = config.tunnel() else {
            return Ok(None);
        };
        if let Some(t) = self.tunnels().get(&config.id) {
            if t.is_alive() {
                return Ok(Some(t.port));
            }
            tracing::warn!("SSH tunnel of {} exited; restarting it", config.name);
        }
        let t = Tunnel::open(ssh, &config.host, config.port).await?;
        let port = t.port;
        // Replaces (and so kills) a dead one.
        self.tunnels().insert(config.id.clone(), t);
        Ok(Some(port))
    }

    fn tunnels(&self) -> std::sync::MutexGuard<'_, HashMap<String, Tunnel>> {
        self.tunnels.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Kills the connection's tunnel; returns it for its stderr.
    fn close_tunnel(&self, id: &str) -> Option<Tunnel> {
        let t = self.tunnels().remove(id);
        if let Some(t) = &t {
            t.kill();
        }
        t
    }

    /// Kills every SSH tunnel (at quit, when nothing is dropped).
    pub fn close_tunnels(&self) {
        for (_, t) in self.tunnels().drain() {
            t.kill();
        }
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
        // Dropping the clients closes their connections; the tunnel goes
        // after the rollbacks, which ran through it.
        self.close_tunnel(id);
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
        let via = self.tunnel_port(&open.config).await?;
        let s =
            Arc::new(open_session(&open.config, open.password.as_deref(), database, via).await?);
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

    /// Tables, views, columns, indexes and constraints of the given schemas
    /// only (those that exist).
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
                    indexes: Vec::new(),
                    constraints: Vec::new(),
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
        Self::introspect_keys(&s.client, only, &mut schemas).await?;
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
                .batch_execute(&format!("declare __boa no scroll cursor for {sql}"))
                .await?;
            // Same work on the server as OFFSET, without rewriting the SQL.
            if offset > 0 {
                s.client
                    .batch_execute(&format!("move forward {offset} in __boa"))
                    .await?;
            }
            let mut r =
                run_simple(s, &format!("fetch forward {} from __boa", limit + 1), limit).await?;
            s.client.batch_execute("close __boa").await?;
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

    /// Applies grid edits as one unit (see `edits::apply`).
    pub async fn apply_edits(
        &self,
        id: &str,
        database: &str,
        statements: &[crate::edits::EditStatement],
    ) -> CmdResult<crate::edits::ApplyOutcome> {
        let s = self.session(id, database).await?;
        let manual = s.manual.load(Ordering::Relaxed);
        crate::edits::apply(&s.client, &s.tx, manual, statements)
            .await
            .map_err(|mut e| {
                e.tx = Some(s.tx.get());
                e
            })
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
    use super::{CsvOptions, Pg, leading_keyword};
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
    /// Creates and drops a `boa_test` schema.
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
            ssh: None,
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
        q("drop schema if exists boa_test cascade", 10)
            .await
            .unwrap();
        q("create schema boa_test", 10).await.unwrap();
        q(r#"create table boa_test."Mixed" (id int primary key, note text, at timestamptz default now())"#, 10)
            .await
            .unwrap();
        let ins = q(r#"insert into boa_test."Mixed" (id, note) select g, 'n' || g from generate_series(1, 25) g"#, 10)
            .await
            .unwrap();
        assert_eq!(ins.affected, Some(25));
        assert!(ins.columns.is_empty());

        // Row cap through the cursor, with types from prepare and text values.
        let r = q(
            r#"select id, note, null::int as nothing from boa_test."Mixed" order by id;"#,
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
                r#"select id from boa_test."Mixed" order by id"#,
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
            r#"insert into boa_test."Mixed" (id) values (100) returning id"#,
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
                r#"insert into boa_test."Mixed" (id) values (101) returning id"#,
                10,
                10
            )
            .await
            .is_err()
        );

        // Server errors carry SQLSTATE and position; the session stays usable.
        let e = q("select * from boa_test.nope", 10).await.unwrap_err();
        assert_eq!(e.code.as_deref(), Some("42P01"));
        assert_eq!(e.position, Some(15));
        assert_eq!(
            q("select 1", 10).await.unwrap().rows,
            vec![vec![Some("1".into())]]
        );

        // Explicit transaction: our cursor must not commit it.
        q("begin", 10).await.unwrap();
        q(r#"delete from boa_test."Mixed""#, 10).await.unwrap();
        assert_eq!(
            q(r#"select count(*) from boa_test."Mixed""#, 10)
                .await
                .unwrap()
                .rows[0][0]
                .as_deref(),
            Some("0")
        );
        q("rollback", 10).await.unwrap();
        assert_eq!(
            q(r#"select count(*) from boa_test."Mixed""#, 10)
                .await
                .unwrap()
                .rows[0][0]
                .as_deref(),
            Some("26")
        );

        // CSV export: the whole result streamed to a file, NULL vs '' and quoting.
        let dir = std::env::temp_dir().join(format!("boa-live-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let dest = dir.join("out.csv");
        let csv = CsvOptions {
            delimiter: ',',
            header: true,
        };
        let export = |sql: &'static str| pg.export_csv("test", &db, sql, &dest, csv, |_| {});
        let r = export(
            r#"select id, note, case when id = 1 then '' end as e from boa_test."Mixed" order by id;"#,
        )
        .await
        .unwrap();
        assert_eq!(r.rows, 26);
        let text = std::fs::read_to_string(&dest).unwrap();
        assert!(text.starts_with("id,note,e\n1,n1,\"\"\n2,n2,\n"));
        assert_eq!(r.bytes, text.len() as u64);
        export(r#"select 'a,b' as "x y", 'say "hi"' as q, E'l1\nl2' as nl -- trailing comment"#)
            .await
            .unwrap();
        assert_eq!(
            std::fs::read_to_string(&dest).unwrap(),
            "x y,q,nl\n\"a,b\",\"say \"\"hi\"\"\",\"l1\nl2\"\n"
        );
        // Anything that writes is refused and not run; the file is untouched.
        assert!(
            export(r#"with x as (insert into boa_test."Mixed" (id) values (200) returning id) select * from x"#)
                .await
                .is_err()
        );
        assert!(std::fs::read_to_string(&dest).unwrap().starts_with("x y,"));
        // Inside a user transaction: sees its uncommitted rows, and a failed
        // export leaves the transaction usable.
        q("begin", 10).await.unwrap();
        q(r#"insert into boa_test."Mixed" (id) values (300)"#, 10)
            .await
            .unwrap();
        let r = export(r#"select id from boa_test."Mixed""#).await.unwrap();
        assert_eq!(r.rows, 27);
        assert!(export("select 1 / 0").await.is_err());
        assert_eq!(
            q(r#"select count(*) from boa_test."Mixed""#, 10)
                .await
                .unwrap()
                .rows[0][0]
                .as_deref(),
            Some("27")
        );
        q("rollback", 10).await.unwrap();
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
        std::fs::remove_dir_all(&dir).unwrap();

        // The state comes from the server: typed BEGIN, an error inside the
        // block (25P02 until rollback), COMMIT of an aborted block.
        use crate::tx::TxState;
        assert_eq!(q("begin", 10).await.unwrap().tx, TxState::Active);
        let e = q("select * from boa_test.nope", 10).await.unwrap_err();
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
        let r = q(r#"delete from boa_test."Mixed" where id > 20"#, 10)
            .await
            .unwrap();
        assert_eq!(r.tx, TxState::Active);
        let p = pg
            .query(
                "test",
                &db,
                r#"select id from boa_test."Mixed" order by id"#,
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
            q(r#"select count(*) from boa_test."Mixed""#, 10)
                .await
                .unwrap()
                .rows[0][0]
                .as_deref(),
            Some("26")
        );

        let schemas = pg
            .introspect("test", &db, &["boa_test".to_owned()])
            .await
            .unwrap();
        assert_eq!(schemas.len(), 1);
        assert!(
            pg.list_schemas("test", &db)
                .await
                .unwrap()
                .contains(&"boa_test".to_owned())
        );
        let t = &schemas
            .iter()
            .find(|s| s.name == "boa_test")
            .unwrap()
            .tables[0];
        assert_eq!(t.name, "Mixed");
        assert_eq!(t.columns.len(), 3);
        assert!(t.columns[0].pk && !t.columns[0].nullable);
        assert_eq!(t.columns[2].ty, "timestamp with time zone");
        assert_eq!(t.columns[2].default.as_deref(), Some("now()"));

        // Keys, foreign keys, checks and indexes, and the DDL built from them.
        for sql in [
            "create table boa_test.parent (a int, b text, primary key (a, b), unique (b))",
            "create table boa_test.child (id int generated always as identity primary key, \
             pa int not null, pb text, n int check (n > 0), \
             total int generated always as (n * 2) stored, \
             constraint child_parent_fkey foreign key (pa, pb) references boa_test.parent (a, b))",
            "create index child_lower on boa_test.child (lower(pb)) include (n)",
            "comment on table boa_test.child is 'kids'",
            "create view boa_test.v as select id from boa_test.child",
        ] {
            q(sql, 10).await.unwrap();
        }
        let schemas = pg
            .introspect("test", &db, &["boa_test".to_owned()])
            .await
            .unwrap();
        let find = |name: &str| schemas[0].tables.iter().find(|t| t.name == name).unwrap();
        let (parent, child) = (find("parent"), find("child"));
        let pk = parent.constraints.iter().find(|c| c.kind == "p").unwrap();
        assert_eq!(pk.columns, ["a", "b"]);
        assert!(
            parent
                .constraints
                .iter()
                .any(|c| c.kind == "u" && c.columns == ["b"])
        );
        let fk = child.constraints.iter().find(|c| c.kind == "f").unwrap();
        assert_eq!(fk.name, "child_parent_fkey");
        assert_eq!(fk.columns, ["pa", "pb"]);
        let r = fk.references.as_ref().unwrap();
        assert_eq!(
            (r.schema.as_str(), r.table.as_str()),
            ("boa_test", "parent")
        );
        assert_eq!(r.columns, ["a", "b"]);
        let check = child.constraints.iter().find(|c| c.kind == "c").unwrap();
        assert_eq!(check.definition, "CHECK (n > 0)");
        assert_eq!(check.columns, ["n"]);
        assert_eq!(
            child
                .constraints
                .iter()
                .filter(|c| c.references.is_some())
                .count(),
            1
        );
        let ix = child
            .indexes
            .iter()
            .find(|i| i.name == "child_lower")
            .unwrap();
        assert_eq!(ix.columns, ["lower(pb)"]);
        assert!(!ix.unique && !ix.primary);
        assert!(
            ix.definition
                .starts_with("CREATE INDEX child_lower ON boa_test.child")
        );
        assert!(
            child
                .indexes
                .iter()
                .any(|i| i.primary && i.columns == ["id"])
        );
        assert!(find("v").indexes.is_empty() && find("v").constraints.is_empty());

        let ddl = pg
            .table_ddl("test", &db, "boa_test", "child")
            .await
            .unwrap();
        assert!(
            ddl.starts_with(
                "CREATE TABLE boa_test.child\n(\n    id integer GENERATED ALWAYS AS IDENTITY NOT NULL,\n    pa integer NOT NULL,\n    pb text,\n"
            ),
            "{ddl}"
        );
        assert!(
            ddl.contains("\n    total integer GENERATED ALWAYS AS ("),
            "{ddl}"
        );
        assert!(
            ddl.contains("CONSTRAINT child_pkey PRIMARY KEY (id)"),
            "{ddl}"
        );
        assert!(
            ddl.contains(
                "CONSTRAINT child_parent_fkey FOREIGN KEY (pa, pb) REFERENCES boa_test.parent(a, b)"
            ),
            "{ddl}"
        );
        assert!(
            ddl.contains(
                "\nCREATE INDEX child_lower ON boa_test.child USING btree (lower(pb)) INCLUDE (n);\n"
            ),
            "{ddl}"
        );
        // The primary key's index comes with its constraint.
        assert!(!ddl.contains("INDEX child_pkey"), "{ddl}");
        assert!(
            ddl.ends_with("COMMENT ON TABLE boa_test.child IS 'kids';\n"),
            "{ddl}"
        );
        let ddl = pg.table_ddl("test", &db, "boa_test", "v").await.unwrap();
        assert!(ddl.starts_with("CREATE VIEW boa_test.v AS\n"), "{ddl}");
        assert!(
            ddl.ends_with(";\n") && ddl.contains("FROM boa_test.child"),
            "{ddl}"
        );
        let ddl = pg
            .table_ddl("test", &db, "boa_test", "Mixed")
            .await
            .unwrap();
        assert!(
            ddl.starts_with("CREATE TABLE boa_test.\"Mixed\"\n(\n    id integer NOT NULL,"),
            "{ddl}"
        );
        assert!(pg.table_ddl("test", &db, "boa_test", "nope").await.is_err());

        // Cancel a running statement.
        let slow = q("select pg_sleep(10)", 10);
        let cancel = async {
            tokio::time::sleep(std::time::Duration::from_millis(300)).await;
            pg.cancel("test", &db).await.unwrap();
        };
        let (r, ()) = tokio::join!(slow, cancel);
        assert_eq!(r.unwrap_err().code.as_deref(), Some("57014"));

        // Grid edits: one transaction, exactly one row each, rolled back on failure.
        use crate::edits::EditStatement;
        let st = |sql: &str| EditStatement {
            sql: sql.into(),
            exactly_one: true,
        };
        let count = || async {
            pg.query(
                "test",
                &db,
                r#"select count(*) from boa_test."Mixed""#,
                10,
                0,
            )
            .await
            .unwrap()
            .rows[0][0]
                .clone()
                .unwrap()
        };
        let out = pg
            .apply_edits(
                "test",
                &db,
                &[
                    st(r#"DELETE FROM boa_test."Mixed" WHERE id = '1'"#),
                    st(r#"UPDATE boa_test."Mixed" SET note = E'it''s \\ x' WHERE id = '2'"#),
                    st(r#"INSERT INTO boa_test."Mixed" (id) VALUES ('500')"#),
                ],
            )
            .await
            .unwrap();
        assert!(out.failed.is_none() && !out.in_transaction);
        assert_eq!(out.runs.len(), 3);
        assert_eq!(count().await, "26");
        let note = pg
            .query("test", &db, r#"select note, at is not null from boa_test."Mixed" where id in (2, 500) order by id"#, 10, 0)
            .await
            .unwrap();
        assert_eq!(note.rows[0][0].as_deref(), Some("it's \\ x"));
        assert_eq!(note.rows[1][1].as_deref(), Some("t"));
        // A missing row fails the batch and undoes the earlier statements.
        let out = pg
            .apply_edits(
                "test",
                &db,
                &[
                    st(r#"DELETE FROM boa_test."Mixed" WHERE id = '3'"#),
                    st(r#"UPDATE boa_test."Mixed" SET note = 'x' WHERE id = '9999'"#),
                ],
            )
            .await
            .unwrap();
        let f = out.failed.unwrap();
        assert_eq!(f.index, 1);
        assert_eq!(f.error.code.as_deref(), Some("row_count"));
        assert_eq!(count().await, "26");
        // Inside a user transaction: a savepoint, nothing committed.
        q("begin", 10).await.unwrap();
        let out = pg
            .apply_edits(
                "test",
                &db,
                &[
                    st(r#"INSERT INTO boa_test."Mixed" (id) VALUES ('1')"#),
                    st("select 1/0"),
                ],
            )
            .await
            .unwrap();
        assert!(out.in_transaction);
        assert_eq!(out.failed.unwrap().error.code.as_deref(), Some("22012"));
        assert_eq!(count().await, "26");
        pg.apply_edits(
            "test",
            &db,
            &[st(r#"DELETE FROM boa_test."Mixed" WHERE id = '500'"#)],
        )
        .await
        .unwrap();
        assert_eq!(count().await, "25");
        q("rollback", 10).await.unwrap();
        assert_eq!(count().await, "26");
        // Manual mode: submitting opens the transaction and leaves it open.
        pg.set_tx_mode("test", &db, true).await.unwrap();
        let out = pg
            .apply_edits(
                "test",
                &db,
                &[st(r#"DELETE FROM boa_test."Mixed" WHERE id = '500'"#)],
            )
            .await
            .unwrap();
        assert!(out.failed.is_none() && out.in_transaction);
        assert_eq!(out.tx, TxState::Active);
        assert_eq!(count().await, "25");
        assert_eq!(q("rollback", 10).await.unwrap().tx, TxState::Idle);
        assert_eq!(count().await, "26");
        pg.set_tx_mode("test", &db, false).await.unwrap();
        // Auto mode, no transaction: committed, and the session is idle again.
        let out = pg
            .apply_edits(
                "test",
                &db,
                &[st(
                    r#"UPDATE boa_test."Mixed" SET note = 'n2' WHERE id = '2'"#,
                )],
            )
            .await
            .unwrap();
        assert!(!out.in_transaction);
        assert_eq!(out.tx, TxState::Idle);

        // Disconnecting rolls an open transaction back.
        q("begin", 10).await.unwrap();
        q(r#"delete from boa_test."Mixed""#, 10).await.unwrap();
        pg.disconnect("test").await;
        pg.connect(config.clone(), std::env::var("PGPASSWORD").ok())
            .await
            .unwrap();
        let count = q(r#"select count(*) from boa_test."Mixed""#, 10)
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
        let e = q(r#"delete from boa_test."Mixed""#, 10).await.unwrap_err();
        assert_eq!(e.code.as_deref(), Some("25006"));
        assert!(e.hint.is_some());
        let e = q("set default_transaction_read_only = off", 10)
            .await
            .unwrap_err();
        assert_eq!(e.code.as_deref(), Some("read_only"));
        let e = q("begin read write", 10).await.unwrap_err();
        assert_eq!(e.code.as_deref(), Some("read_only"));
        assert_eq!(e.tx, Some(TxState::Idle));
        let e = q(r#"delete from boa_test."Mixed""#, 10).await.unwrap_err();
        assert_eq!(e.code.as_deref(), Some("25006"));
        pg.disconnect("test").await;

        pg.connect(config, std::env::var("PGPASSWORD").ok())
            .await
            .unwrap();
        q("drop schema boa_test cascade", 10).await.unwrap();
        pg.disconnect("test").await;
    }
}
