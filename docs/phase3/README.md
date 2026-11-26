# Phase 3: the Database Explorer (p3/explorer)

The explorer tree, the "N of M" scope picker, the context menus, the
environment tag on connection rows, and the right panel's Structure / Info
tabs. Spec: source `docs/USAGE.md` "Layout" / "Data sources", `Sidebar.tsx`,
`ScopePicker.tsx`, `RightPanel.tsx`, `ConnectionTag.tsx`, `lib/tableMeta.ts`,
`state/ddl.ts`, `store.ts` (`loadDatabase`, `setScope`, scope helpers),
`state/scope.test.ts`.

## Screenshots

Rendered with `--features screenshot` against the local dev database (three
saved connections in a temp `BOA_CONFIG_DIR`: "Local dev", "Production
replica" tagged Production + read-only as `boa_ro`, "Staging"). The explorer's
dev hooks (`views/sidebar/dev.rs`: `BOA_DEV_EXPLORER=connect`,
`BOA_DEV_EXPAND`, `BOA_DEV_ACTIVE_TABLE`, `BOA_DEV_SCOPE_PICKER`,
`BOA_DEV_CONTEXT_MENU`, `BOA_DEV_FOCUS`) script the state, since the
Connections and Workspaces models are still stubs on this branch.

| File | Shows |
|---|---|
| `explorer-tree.png` | connection › database › schema › tables › `orders` expanded to its columns (PK yellow key, FK blue key, types, `not null`), then the keys / foreign keys / indexes folders; row counts (5k, 100k); the focused row; "1 of 3" / "1 of 5" badges; PROD + lock and STAGE tags; right panel Structure of the open table |
| `scope-picker.png` | the schemas picker of `boa_dev`: filter box, "All schemas · includes new ones" (mixed), `public (default)` ticked, the non-ASCII schema, the footer hint |
| `context-menu.png` | the table context menu |
| `production-tag.png` | the production-tagged, read-only connection expanded (red server icon + PROD badge + lock), Staging (orange), Structure of `customers` |

Compared with the source's `docs/images/boa.png`: same rows, indents, icons
and colours, child counts and badges. Differences: the source's screenshot is
older (it shows a status dot and "offline" after the name; current
`Sidebar.tsx` puts the state as a badge on the server icon, which is what
this port does); context-menu items use gpui-component's menu metrics;
italic for preview tabs is not visible because no italic Geist Mono is bundled.

## How it is built

- `models/catalogs.rs` — `Catalogs`: per `DbKey` the schema names, the
  introspected schemas in scope (`Arc<Vec<Schema>>`), `loading` / `error`,
  `names_loaded` / `loaded`. `load(key, force)` ports `loadDatabase` (drops
  unpicked, introspects missing, re-runs if the pick changed; a forced load
  asked for during a load runs after it). Watches prefs (`scopes` changes →
  `setScope` semantics) and Connections (connect / reconnect → names re-read,
  first shown + expanded databases load; deleted connections dropped).
- `views/sidebar/tree.rs` — pure `build(&TreeInput) -> Vec<Node>` (the
  `useNodes` port, node ids exactly the TS `nodeId`s = `treeExpanded` keys) and
  `nav(nodes, focus, key, expanded)` (the `onKeyDown` port).
- `views/sidebar.rs` (+ `row.rs`) — the view: `uniform_list` rows, click /
  double-click / chevron, ↑ ↓ → ← ↵ (key context `Sidebar`), context menus
  (gpui-component), connection drag-reorder, the toolbar.
- `views/scope_picker.rs` — `ScopeDraft` (pure picker rules) + the popover view.
- `views/right_panel.rs` — Structure / Info from Workspaces `active` and the catalog.

## Verified

- Tests: Catalogs live against `boa_dev` (default pick on connect, scope
  changes incl. `boa_fx_many` ≥ 50 tables and `boa_fx_ünïcødé`, `'*'`, empty
  pick, connection pick + expanded database, refresh vs cached load, queued
  refresh, reconnect re-reads names); tree ids / depths / parents, counts,
  key icons, views toggle, row-count format, unconnected states, scope picks,
  current / preview rows, keyboard nav (unit); keyboard + mouse + prefs.json
  persistence + scope picker Enter / Esc / filter through the real keymap
  (`#[gpui::test]`).
- Screenshots above.

## Not verified / later

- Clicking tables / databases calls `Workspaces::open_table` /
  `select_database` / `new_query_tab(_with)` (p4 stubs on this branch);
  connect / disconnect / reorder call `Connections` (p2 stubs); Edit / Delete
  / "+ Data source" call `connection_dialog` (p2 stub). Visible only after the merge.
- Show DDL end to end (needs `new_query_tab_with`); the DDL fetch + toast path compiles against `ddl_action`.
- Export to CSV is `not_yet` (Phase 7). The right panel's History tab and its
  search (Esc clears it) are Phase 7: Structure / Info have no search in the source.
- Real-window check (`screencapture`) of the context menu and the picker.
