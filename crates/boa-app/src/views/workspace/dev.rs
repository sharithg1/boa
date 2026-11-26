//! Dev hooks (feature `dev-hooks`, never in release builds): script the
//! consoles from the environment for screenshots and manual checks, until
//! the Connections / Catalogs models of Phase 2 / 3 do it for real.
//!
//! - `BOA_DEV_LIVE=1`: save the libpq-style server (`PGHOST` … `PGDATABASE`,
//!   defaults: the local dev server) as connection "Local" in the config dir
//!   (`BOA_CONFIG_DIR`!), connect, load the catalog, select the database.
//! - `BOA_EDITOR_TEXT`: a console with this SQL (`\n` for newlines).
//! - `BOA_DEV_CONSOLES=n`: n more consoles before it; `BOA_DEV_TABLES=a,b`:
//!   table tabs (`schema.table` or `table` in public); `BOA_DEV_PREVIEW=t`:
//!   a preview tab after them.
//! - `BOA_DEV_CARET=<byte>`: the caret; then `BOA_DEV_ARM=1` arms (statement
//!   chooser), `BOA_DEV_RUN=1` arms and runs, `BOA_EDITOR_TYPE=…` types at the
//!   caret (completion popup), `BOA_DEV_EXPLICIT=1` asks for completion.
//! - `BOA_DEV_OUTPUT=1`: show the Output log after the run.
//! - `BOA_EDITOR_BENCH_LINES=n`: a console with n lines of generated SQL
//!   (instead of `BOA_EDITOR_TEXT`), the caret in the middle.
//! - `BOA_EDITOR_BENCH=n`: then type n keystrokes there through the real
//!   keymap, one per frame, and log the time each one's handling took and the
//!   frame intervals (`BOA_LOG=info`; with `ZED_MEASUREMENTS=1` GPUI also prints
//!   each frame's draw + present time). `BOA_EDITOR_BENCH_QUIT=1` quits after.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use boa_core::connections::{ConnConfig, SavedConnection};
use gpui::{App, AsyncWindowContext, Context, Keystroke, WeakEntity, Window};

use super::Workspace;
use crate::app_state::AppState;
use crate::models::catalogs::{Catalog, DbKey};
use crate::models::connections::ConnState;

fn env(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.is_empty())
}

fn env_or(name: &str, default: &str) -> String {
    env(name).unwrap_or_else(|| default.to_owned())
}

/// Starts the scripted session, if `BOA_DEV_LIVE` is set.
pub fn start(window: &mut Window, cx: &mut Context<Workspace>) {
    if env("BOA_DEV_LIVE").is_none() {
        return;
    }
    cx.spawn_in(window, async move |this, cx| {
        if let Err(err) = run(this, cx).await {
            tracing::error!("dev session: {err}");
        }
    })
    .detach();
}

async fn pause(cx: &mut AsyncWindowContext, ms: u64) {
    cx.background_executor()
        .timer(Duration::from_millis(ms))
        .await;
}

async fn run(this: WeakEntity<Workspace>, cx: &mut AsyncWindowContext) -> Result<(), String> {
    let state = cx
        .update(|_, cx| AppState::global(cx).clone())
        .map_err(|e| e.to_string())?;
    let database = env_or("PGDATABASE", "boa_dev");
    let config = ConnConfig {
        id: "local".into(),
        name: "Local".into(),
        host: env_or("PGHOST", "localhost"),
        port: env_or("PGPORT", "5432").parse().unwrap_or(5432),
        user: env_or("PGUSER", "boa"),
        database: database.clone(),
        ssl_mode: Default::default(),
        read_only: false,
        environment: env("BOA_DEV_ENV")
            .and_then(|e| serde_json::from_value(serde_json::Value::String(e)).ok()),
        color: None,
        ssh: None,
    };
    let backend = state.backend.clone();
    backend
        .save_connection(config.clone(), Some(env_or("PGPASSWORD", "boa")), true)
        .await
        .map_err(|e| e.message)?;
    let info = backend
        .connect("local", None)
        .await
        .map_err(|e| e.message)?;
    let key = DbKey {
        connection_id: "local".into(),
        database: database.clone(),
    };
    let names = backend
        .list_schemas("local", &database)
        .await
        .map_err(|e| e.message)?;
    let schemas = backend
        .introspect("local", &database, vec!["public".into(), "boa_fx".into()])
        .await
        .map_err(|e| e.message)?;
    cx.update(|_, cx| {
        state.connections.update(cx, |c, cx| {
            c.saved.push(SavedConnection {
                config,
                has_password: true,
            });
            c.state.insert(
                "local".into(),
                ConnState::Connected {
                    server_version: info.server_version,
                    databases: info.databases,
                },
            );
            cx.notify();
        });
        state.catalogs.update(cx, |c, cx| {
            c.catalogs.insert(
                key.clone(),
                Catalog {
                    schema_names: names,
                    schemas: std::sync::Arc::new(schemas),
                    loading: false,
                    error: None,
                    names_loaded: true,
                    loaded: true,
                    ..Default::default()
                },
            );
            cx.notify();
        });
        state.workspaces.update(cx, |w, cx| {
            w.select_database(key.clone(), cx);
            for _ in 0..env("BOA_DEV_CONSOLES")
                .and_then(|n| n.parse().ok())
                .unwrap_or(0)
            {
                w.new_query_tab(key.clone(), cx);
            }
            for t in env("BOA_DEV_TABLES")
                .unwrap_or_default()
                .split(',')
                .filter(|t| !t.is_empty())
            {
                let (schema, table) = t.split_once('.').unwrap_or(("public", t));
                w.open_table(key.clone(), schema, table, false, cx);
            }
            if let Some(t) = env("BOA_DEV_PREVIEW") {
                let (schema, table) = t.split_once('.').unwrap_or(("public", &t));
                w.open_table(key.clone(), schema, table, true, cx);
            }
            if let Some(sql) = env("BOA_EDITOR_TEXT") {
                w.new_query_tab_with(key.clone(), sql.replace("\\n", "\n"), cx);
            } else if let Some(lines) = env("BOA_EDITOR_BENCH_LINES").and_then(|n| n.parse().ok()) {
                w.new_query_tab_with(key.clone(), bench_sql(lines), cx);
            }
        });
    })
    .map_err(|e| e.to_string())?;
    pause(cx, 300).await;
    let editor = this
        .update(cx, |w, cx| w.editor(cx))
        .map_err(|e| e.to_string())?;
    let Some(editor) = editor else { return Ok(()) };
    cx.update(|window, cx| {
        editor.update(cx, |e, cx| {
            let state = e.state().clone();
            let caret = env("BOA_DEV_CARET")
                .and_then(|c| c.parse::<usize>().ok())
                .or_else(|| {
                    let lines = env("BOA_EDITOR_BENCH_LINES")?.parse().ok()?;
                    Some(bench_caret(&bench_sql(lines)))
                });
            if let Some(at) = caret {
                state.update(cx, |s, cx| s.set_selected_range(at..at, cx));
            }
            state.update(cx, |s, cx| s.focus(window, cx));
        });
    })
    .map_err(|e| e.to_string())?;
    pause(cx, 100).await;
    cx.update(|window, cx| {
        editor.update(cx, |e, cx| {
            if env("BOA_DEV_ARM").is_some() || env("BOA_DEV_RUN").is_some() {
                e.run_or_arm(false, window, cx);
            }
            if env("BOA_DEV_RUN").is_some() {
                e.run_or_arm(false, window, cx);
            }
            if let Some(text) = env("BOA_EDITOR_TYPE") {
                e.type_text(&text.replace("\\n", "\n"), window, cx);
            }
            if env("BOA_DEV_EXPLICIT").is_some() {
                e.suggest(true, cx);
            }
        });
    })
    .map_err(|e| e.to_string())?;
    if let Some(n) = env("BOA_EDITOR_BENCH").and_then(|n| n.parse::<usize>().ok()) {
        // Let the editor settle (highlighting, outline) first.
        pause(cx, 1500).await;
        cx.update(|window, cx| TypingBench::start(n, window, cx))
            .map_err(|e| e.to_string())?;
    }
    if env("BOA_DEV_OUTPUT").is_some() {
        pause(cx, 1500).await;
        cx.update(|_, cx| {
            state
                .workspaces
                .update(cx, |w, cx| w.set_bottom_view(None, cx))
        })
        .map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// `lines` lines of SQL, four per statement (a typical console).
fn bench_sql(lines: usize) -> String {
    let mut sql = String::new();
    for i in 0..lines {
        let line = match i % 4 {
            0 => "select o.id, o.customer_id, o.total, o.status, o.created_at".to_owned(),
            1 => "from public.orders o join public.customers c on c.id = o.customer_id".to_owned(),
            2 => format!(
                "where o.customer_id = {} and o.status <> 'cancelled'",
                i / 4
            ),
            _ => "order by o.created_at desc limit 100;".to_owned(),
        };
        sql.push_str(&line);
        sql.push('\n');
    }
    sql
}

/// The end of the `where` line nearest the middle of `bench_sql`'s text.
fn bench_caret(sql: &str) -> usize {
    let lines: Vec<&str> = sql.split_inclusive('\n').collect();
    let mut at = 0;
    for (i, line) in lines.iter().enumerate() {
        if i >= lines.len() / 2 && i % 4 == 2 {
            return at + line.trim_end_matches('\n').len();
        }
        at += line.len();
    }
    at
}

/// `BOA_EDITOR_BENCH`: one keystroke per frame, from the frame callbacks.
struct TypingBench {
    keys: Vec<Keystroke>,
    left: usize,
    next: usize,
    last: Option<Instant>,
    /// Keystroke dispatch to handled (action + text input + model updates).
    handle: Vec<f64>,
    /// Frame callback to frame callback (a keystroke was drawn in between).
    interval: Vec<f64>,
}

impl TypingBench {
    fn start(n: usize, window: &mut Window, _: &mut App) {
        // " and o.total > 100 or c.name = x"
        const KEYS: &[&str] = &[
            "space", "a", "n", "d", "space", "o", ".", "t", "o", "t", "a", "l", "space", "shift->",
            "space", "1", "0", "0", "space", "o", "r", "space", "c", ".", "n", "a", "m", "e",
            "space", "=", "space", "x",
        ];
        let keys = KEYS
            .iter()
            .filter_map(|k| Keystroke::parse(k).ok())
            .collect();
        let bench = Rc::new(RefCell::new(Self {
            keys,
            left: n,
            next: 0,
            last: None,
            handle: Vec::new(),
            interval: Vec::new(),
        }));
        tracing::info!("typing bench: start ({n} keystrokes)");
        window.on_next_frame(move |window, cx| Self::step(bench, window, cx));
        window.refresh();
    }

    fn step(bench: Rc<RefCell<Self>>, window: &mut Window, cx: &mut App) {
        let now = Instant::now();
        let key = {
            let mut b = bench.borrow_mut();
            if let Some(last) = b.last {
                b.interval.push((now - last).as_secs_f64() * 1000.);
            }
            b.last = Some(now);
            if b.left == 0 {
                b.report();
                if env("BOA_EDITOR_BENCH_QUIT").is_some() {
                    cx.quit();
                }
                return;
            }
            b.left -= 1;
            let key = b.keys[b.next % b.keys.len()].clone();
            b.next += 1;
            key
        };
        let t0 = Instant::now();
        window.dispatch_keystroke(key, cx);
        let handled = t0.elapsed().as_secs_f64() * 1000.;
        bench.borrow_mut().handle.push(handled);
        // This frame draws the keystroke right after the callbacks; the
        // next one types the next key.
        window.on_next_frame(move |window, cx| Self::step(bench, window, cx));
    }

    fn report(&mut self) {
        let stats = |v: &mut Vec<f64>| {
            v.sort_by(f64::total_cmp);
            let p = |q: f64| v[((v.len() - 1) as f64 * q).round() as usize];
            format!(
                "p50 {:.2} p95 {:.2} p99 {:.2} max {:.2}",
                p(0.5),
                p(0.95),
                p(0.99),
                v.last().copied().unwrap_or(0.)
            )
        };
        if self.handle.is_empty() || self.interval.is_empty() {
            return;
        }
        let median = {
            let mut v = self.interval.clone();
            v.sort_by(f64::total_cmp);
            v[v.len() / 2]
        };
        let long = self.interval.iter().filter(|&&i| i > median * 1.5).count();
        tracing::info!(
            "typing bench: {} keystrokes; handling ms {}; frame interval ms {} ({long} over 1.5× median)",
            self.handle.len(),
            stats(&mut self.handle),
            stats(&mut self.interval),
        );
        tracing::info!("typing bench: end");
    }
}
