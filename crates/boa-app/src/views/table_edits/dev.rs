//! Dev hooks of the table tab's filter bar and data editing (feature
//! `dev-hooks`, never in release builds), for screenshots and manual checks.
//! Needs a connected connection (`BOA_DEV_CONNECTION=1` seeds and connects
//! "Local dev").
//!
//! - `BOA_DEV_TABLE=schema.table`: once connected, add the schema to the
//!   database's scope, load the catalog and open the table (pinned).
//! - `BOA_DEV_TABLE_WHERE` / `BOA_DEV_TABLE_ORDER`: apply them (Enter in the bar).
//! - `BOA_DEV_TABLE_DRAFT=text`: type into the WHERE field without applying.
//! - `BOA_DEV_TABLE_ROW_FILTER=text`: type into "Filter loaded rows".
//! - `BOA_DEV_TABLE_EDITS=r,c,value;…`: stage edits on the first page
//!   (`value` `NULL` sets NULL; `del,r` deletes row r, `add` adds a row,
//!   `new,c,value` sets column c of the last new row).
//! - `BOA_DEV_TABLE_PREVIEW=1`: open the SQL preview; `BOA_DEV_TABLE_SUBMIT=1`:
//!   submit (the preview reopens on a failure).

use std::time::Duration;

use boa_core::grid::grid_edits::{Pending, RowRef};
use boa_core::model::Value;
use boa_core::state::prefs::Scope;
use gpui::{AsyncWindowContext, Context, Entity, WeakEntity, Window};

use crate::app_state::AppState;
use crate::grid::ResultGrid;
use crate::models::catalogs::DbKey;
use crate::models::table_tab::TableTab;
use crate::views::table_filter_bar::FilterField;
use crate::views::workspace::Workspace;

fn env(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.is_empty())
}

/// Starts the scripted session, if `BOA_DEV_TABLE` is set.
pub fn start(window: &mut Window, cx: &mut Context<Workspace>) {
    let Some(spec) = env("BOA_DEV_TABLE") else {
        return;
    };
    cx.spawn_in(window, async move |this, cx| {
        if let Err(err) = run(spec, this, cx).await {
            tracing::error!("dev table session: {err}");
        }
    })
    .detach();
}

async fn pause(cx: &mut AsyncWindowContext, ms: u64) {
    cx.background_executor()
        .timer(Duration::from_millis(ms))
        .await;
}

/// Polls `f` every 100 ms for up to 20 s.
async fn wait_for<T>(
    cx: &mut AsyncWindowContext,
    what: &str,
    mut f: impl FnMut(&mut gpui::App) -> Option<T>,
) -> Result<T, String> {
    for _ in 0..200 {
        if let Ok(Some(v)) = cx.update(|_, cx| f(cx)) {
            return Ok(v);
        }
        pause(cx, 100).await;
    }
    Err(format!("timed out waiting for {what}"))
}

async fn run(
    spec: String,
    this: WeakEntity<Workspace>,
    cx: &mut AsyncWindowContext,
) -> Result<(), String> {
    let (schema, table) = spec
        .split_once('.')
        .map(|(s, t)| (s.to_owned(), t.to_owned()))
        .unwrap_or_else(|| ("public".to_owned(), spec.clone()));
    let state = cx
        .update(|_, cx| AppState::global(cx).clone())
        .map_err(|e| e.to_string())?;
    let key: DbKey = wait_for(cx, "a connected connection", |cx| {
        let conns = state.connections.read(cx);
        conns
            .saved
            .iter()
            .find(|c| conns.is_connected(&c.config.id))
            .map(|c| DbKey::new(c.config.id.clone(), c.config.database.clone()))
    })
    .await?;
    let scope_schema = schema.clone();
    let scope_key = key.clone();
    cx.update(|_, cx| {
        state.prefs.update(cx, |p, cx| {
            p.update(cx, |p| {
                let mut names = vec!["public".to_owned()];
                if scope_schema != "public" {
                    names.push(scope_schema.clone());
                }
                p.scopes.insert(scope_key.scope_key(), Scope::Some(names));
            })
        });
        state
            .catalogs
            .update(cx, |c, cx| c.load(scope_key.clone(), true, cx));
    })
    .map_err(|e| e.to_string())?;
    let (s, t) = (schema.clone(), table.clone());
    let k = key.clone();
    wait_for(cx, "the catalog", |cx| {
        state.catalogs.read(cx).find_table(&k, &s, &t).map(|_| ())
    })
    .await?;
    cx.update(|_, cx| {
        state.workspaces.update(cx, |w, cx| {
            w.open_table(key.clone(), &schema, &table, false, cx)
        });
    })
    .map_err(|e| e.to_string())?;
    let tab: Entity<TableTab> = wait_for(cx, "the table tab", |cx| {
        state.workspaces.read(cx).active_tab()?.table().cloned()
    })
    .await?;
    wait_for(cx, "the first page", |cx| {
        tab.read(cx).result.as_ref().map(|_| ())
    })
    .await?;
    let bar = wait_for(cx, "the filter bar", |cx| {
        this.upgrade()?.read(cx).filter_bar_of(&tab)
    })
    .await?;
    let (where_, order) = (env("BOA_DEV_TABLE_WHERE"), env("BOA_DEV_TABLE_ORDER"));
    if where_.is_some() || order.is_some() {
        cx.update(|window, cx| {
            bar.update(cx, |b, cx| {
                if let Some(w) = &where_ {
                    b.set_draft(FilterField::Where, w, window, cx);
                }
                if let Some(o) = &order {
                    b.set_draft(FilterField::OrderBy, o, window, cx);
                }
                b.apply(cx);
            })
        })
        .map_err(|e| e.to_string())?;
        pause(cx, 300).await;
        wait_for(cx, "the filtered page", |cx| {
            let t = tab.read(cx);
            (!t.loading).then_some(())
        })
        .await?;
    }
    if let Some(draft) = env("BOA_DEV_TABLE_DRAFT") {
        cx.update(|window, cx| {
            bar.update(cx, |b, cx| {
                b.set_draft(FilterField::Where, &draft, window, cx)
            })
        })
        .map_err(|e| e.to_string())?;
    }
    let grid: Entity<ResultGrid> =
        wait_for(cx, "the grid", |cx| this.upgrade()?.read(cx).grid_of(&tab)).await?;
    if let Some(text) = env("BOA_DEV_TABLE_ROW_FILTER") {
        let input = wait_for(cx, "the row filter", |cx| {
            this.upgrade()?.read(cx).row_filter_of(&tab)
        })
        .await?;
        cx.update(|window, cx| {
            input.update(cx, |i, cx| i.set_value(text.clone(), window, cx));
            grid.update(cx, |g, cx| g.set_row_filter(&text, cx));
        })
        .map_err(|e| e.to_string())?;
    }
    if let Some(edits) = env("BOA_DEV_TABLE_EDITS") {
        // Editable once the catalog reached the grid.
        wait_for(cx, "editable rows", |cx| {
            matches!(
                grid.read(cx).editability(),
                boa_core::grid::grid_edits::Editability::Editable { .. }
            )
            .then_some(())
        })
        .await?;
        cx.update(|_, cx| {
            let Some(rows) = tab.read(cx).result.clone() else {
                return;
            };
            let mut pending = grid.read(cx).pending();
            stage(&mut pending, &rows.rows, &edits);
            grid.update(cx, |g, cx| g.set_pending(pending.clone(), cx));
            tab.update(cx, |t, cx| t.set_pending(pending, cx));
        })
        .map_err(|e| e.to_string())?;
    }
    pause(cx, 200).await;
    if env("BOA_DEV_TABLE_PREVIEW").is_some() || env("BOA_DEV_TABLE_SUBMIT").is_some() {
        cx.update(|_, cx| tab.update(cx, |t, cx| t.open_preview(cx)))
            .map_err(|e| e.to_string())?;
    }
    if env("BOA_DEV_TABLE_SUBMIT").is_some() {
        pause(cx, 300).await;
        cx.update(|_, cx| tab.update(cx, |t, cx| t.submit_edits(cx)))
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Applies `BOA_DEV_TABLE_EDITS` to `pending`.
fn stage(pending: &mut Pending, rows: &[Vec<Value>], spec: &str) {
    let value = |v: &str| (v != "NULL").then(|| v.to_owned());
    for item in spec.split(';').map(str::trim).filter(|s| !s.is_empty()) {
        let parts: Vec<&str> = item.splitn(3, ',').collect();
        match parts[..] {
            ["add"] => {
                pending.add_row();
            }
            ["del", r] => {
                if let Ok(r) = r.parse() {
                    pending.delete_row(RowRef::Base(r));
                }
            }
            ["new", c, v] => {
                if let (Some(row), Ok(c)) = (pending.inserts.last().map(|n| n.id), c.parse()) {
                    pending.set_cell(rows, RowRef::New(row), c, value(v));
                }
            }
            [r, c, v] => {
                if let (Ok(r), Ok(c)) = (r.parse(), c.parse()) {
                    pending.set_cell(rows, RowRef::Base(r), c, value(v));
                }
            }
            _ => tracing::warn!("BOA_DEV_TABLE_EDITS: can't read {item:?}"),
        }
    }
}
