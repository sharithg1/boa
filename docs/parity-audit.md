# Parity audit: GPUI app vs. the Tauri app (p9/audit)

Audited `main` at `b56309a` (phases 1–7 merged) against the spec: source repo
`agents/dbclient` at `61984bb` (`docs/USAGE.md`, `crates/boa-desktop/ui/src`,
`crates/boa-desktop/src`). This is an audit only. No feature code was changed.

**How it was checked**

- **Code and tests.** Every claim was traced to the GPUI code and to the test that
  covers it. The full suite was run with `PGHOST` set (all green; see the report).
- **Real app.** A dev build (`--features screenshot`, `BOA_CONFIG_DIR` in a scratch
  dir, `BOA_DEV_CONNECTION=1` with the connection tagged Production) was driven
  through `BOA_DEV_INPUT` (`views/dev_input.rs`). Captures are `screencapture -l`
  of the real window and are in `docs/audit/`.
- **Source UI, side by side.** The source React UI was built (unmodified, from a
  copy in a scratch dir) and run in its browser-preview mode (`state/mock.ts`
  sample data) in headless Chrome at the same 1280×820 window size and 2× scale.
  It was driven over the DevTools protocol, and the captures are in
  `docs/audit/src/`. Both sets show the same chrome at the same scale, so
  they can be compared pixel for pixel. Only the data differs (mock
  `app_production` vs. local `boa_dev`). The browser preview has no traffic
  lights and a few mock-only texts; these are ignored below.
- Sections 1, 2, 3 and 5 were compiled by four read-only sub-audits, then
  spot-checked here against the code and the running app.

Status words: **works**, **partial**, **missing**, **differs**.
Evidence: `T:` test fn, `S:` screenshot, `code-only`, `none`.

**Priorities**

- **P0**: data loss, a crash or wrong data.
- **P1**: a feature is missing or behaves wrongly, or a required verification is missing.
- **P2**: visual or polish.

Items already being fixed elsewhere are marked:

- *(p9/fixes1)*: grid `<default>`, narrow header, table-toolbar filter and DDL,
  preview-tab italics, explorer chevron focus, status-bar fallback, export `.part`
  on quit, export-menu disable rule, flaky sidebar test, CI all-features.
- *(p9/core-conc)*: per-session concurrency.
- *(p8)*: bundle and updater.

## Status since the audit (2026-11-26, `main` at `087179f`, then waves 4–5)

Updated by p10/docs-migration from the merged branches and the code, then
by p11/behaviour for waves 4–5 (p10/keys, p10/updates-wire, p10/visual,
p11/behaviour); the tables below are the audit as it was. Test names are
`#[gpui::test]` / `#[test]` fns (`T:`); the shortcut ones are in
`crates/boa-app/src/views/root/shortcut_tests.rs`.

| Gap | Now |
|---|---|
| P0-1 export shares the session with consoles | **Fixed** by p9/core-conc: `crates/boa-core/src/pg/ops.rs` (one operation at a time per session, per-operation `CancelHandle`), live tests in `pg/concurrency_tests.rs`. |
| P0-2 Dock quit / logout not held | **Fixed** by p12/dock-quit (user-approved isolated `unsafe`): `crates/boa-app/src/platform/terminate.rs` adds `applicationShouldTerminate:` to GPUI's `GPUIApplicationDelegate` at run time; with a transaction open (and not asked within 3 s, one gate with ⌘Q) it answers `NSTerminateCancel` and shows the ⌘Q prompt; else `NSTerminateNow`. T:`a_dock_quit_with_an_open_transaction_is_held_and_asks`, T:`a_held_dock_quit_rolls_back_and_quits_when_confirmed`, T:`after_cmd_q_asked_a_dock_quit_goes_through` (`models/transactions/tests.rs`) + the decision's unit tests in `terminate.rs`. Seen live with the Dock's quit Apple event on a background dev build; the real Dock, logout and restart are M9. |
| P1-1 overlays don't block global shortcuts | **Fixed** by p10/keys: T:`the_value_editor_blocks_cmd_w_and_the_other_global_shortcuts`, T:`the_json_viewer_history_and_palette_block_them_too`, T:`over_settings_cmd_o_and_the_tx_keys_work_and_cmd_w_does_nothing`. |
| P1-2 SSH tunnels not closed at quit | **Fixed** by p10/keys (`on_app_quit` in `app.rs`): T:`quitting_closes_the_ssh_tunnels`. The real `ssh -N` processes after ⌘Q are manual check M2. |
| P1-3 ⌘W never closes a window | **Fixed** by p10/keys: T:`cmd_w_closes_the_about_window`, T:`cmd_w_without_a_tab_closes_the_main_window_asking_with_a_transaction_open`. |
| P1-4 updates install is a stub | **Fixed** by p8/release-prep + p10/updates-wire: the Updates model drives `crates/boa-app/src/updater/` (check, download, minisign verify, guarded restart): T:`a_newer_release_is_downloaded_verified_and_offered`, T:`a_tampered_download_is_rejected`, T:`restart_with_an_open_transaction_asks_first`, T:`a_failed_install_says_so_and_can_be_retried` (`models/updates/tests.rs`). A real release feed is still a manual step (RELEASING.md). |
| P1-5 table toolbar DDL / Filter loaded rows / export rule | **Fixed** by p9/fixes1 (`the_ddl_button_shows_for_known_tables_and_opens_their_ddl`, `filter_loaded_rows_filters_the_grid_and_starts_empty_when_shown_again`). |
| P1-6 `<default>` on every new-row column | **Fixed** by p9/fixes1 (`new_rows_show_default_only_in_columns_with_a_default`). |
| P1-7 scope change during a table fetch | Addressed by `pg/ops.rs` (the catalog read now waits for the fetch; one operation at a time per session, T: `pg/concurrency_tests.rs`); still no test of this exact case. |
| P1-8 manual checks never run | Collected in [MANUAL-CHECKS.md](MANUAL-CHECKS.md) (M1–M23); still not run. |
| P1-9 context menus untested | **Fixed** by p10/keys: T:`explorer_menus_copy_open_and_show_what_they_say`, T:`title_bar_menus_open_tabs_the_palette_and_the_connection_actions` (`views/root/menu_tests.rs`), the grid's edit section T:`the_menus_edit_items_each_do_their_edit` and its Copy as / hints (p11, below). |
| P1-10 migration not proven both ways | **T done:** `crates/boa-app/src/tests/migration.rs` (synthetic Tauri install → app → every written file checked with the Tauri rules, and with `BOA_TAURI_UI_SRC` by the source's own TS parsers) and `real_install_copy` run on a copy of the user's install (HANDOFF-PARITY §13). The Tauri app itself opened on GPUI-written files is M20. |
| P1-11 no full-app performance numbers | **Done:** measured in [perf.md](perf.md): memory, scroll and typing meet the targets; startup (window on screen) is ~40 ms behind Tauri's. |
| P2 USAGE "about half a minute" / console results "say why" | **Fixed** in this repo's [USAGE.md](USAGE.md) (about two minutes; console results are read-only without a reason). Checked again by p11: the source's USAGE says console results "say why", but DataGrid.tsx gets no `editing` for them and nothing shows a reason; here only a table tab's strip shows one: T:`read_only_content_blocks_every_edit` (`shown_read_only_reason` is None). |
| P2 `docs/phase1/README.md` stale | Annotated. |
| P2 Ctrl variants of grid edit keys; guard dialogs answering ⌘⇧↵ / ⌘⇧⌫ | **Fixed** by p10/keys: T:`ctrl_works_like_cmd_for_the_grid_keys` (`grid/tests.rs`), T:`guard_dialogs_answer_enter_and_escape_but_not_the_tx_keys`. |
| P2 ⌘K/⌘P, ⌘⌥E, ⌘, dead over gpui-component dialogs | **Fixed** by p11/behaviour: bound again in the `Dialog` context (`keymap.rs`), handled app-wide (`app.rs`). As in App.tsx (checked before its `overlay` test): the palette and History open over the dialog (drawn above it) and give it the focus back; Settings opens under it (`z-40` vs `z-50`), the dialog keeps the focus, and Settings gets it when the dialog closes. ⌘O stays "not over a dialog". T:`cmd_k_cmd_alt_e_and_cmd_comma_work_over_a_dialog`. |
| P2 grid menu Copy as › never disabled, no reason tooltips | **Fixed** by p11/behaviour (`grid/interact.rs`): the trigger is disabled with nothing selected (DataGrid.tsx:725); a format that can't copy the selection is disabled with the reason as its tooltip. T:`copy_as_is_off_without_a_selection_and_formats_say_why`. |
| P2 "Revert row" shows no ⌘⌥Z | **Fixed** by p11/behaviour: as in the source, "Revert row" (reverts the row) shows ⌘⌥Z and ⌘⌥Z is bound to Revert (the cell if edited, else the row). The grid's other menu hints showed the Ctrl / ⌦ / F2 / ⌥Insert variants (the last binding wins); the ⌘ ones are now bound last. T:`the_grid_menu_hints_are_the_sources_keys`. |
| P2 View › Enter Full Screen fixed label | **Fixed** by p11/behaviour: "Exit Full Screen" while the main window is in full screen (the menu bar is set again when it changes). T:`the_view_menu_says_exit_full_screen_in_full_screen`. |
| P2 Enter in About repeats on a held key | **Fixed** by p11/behaviour: a key-down listener skips repeats (`!e.repeat`). T:`enter_in_about_copies_the_version_info`. |
| P2 extra keys (grid / chooser PgUp PgDn) | p11/behaviour: grid PgUp / PgDn kept (harmless; the source's browser only scrolls the grid), listed in USAGE "Differences", T:`page_down_and_up_move_by_a_screen_of_rows`. The statement chooser's were dead (the editor's Input took the keys, and PgDn closed the chooser and moved the caret, as in the source): bindings removed, T:`page_keys_close_the_statement_chooser_and_move_the_caret`. |

Also found during the migration rehearsal: `prefs.json` is written with the
default mode (0644; `Prefs::save` in `crates/boa-core/src/state/prefs.rs`),
while `connections.json`, the history files, `session.json` and
`csv-export.json` are owner-only (0600). It holds no secrets, but it names
connections' databases and schemas (`scopes`, `treeExpanded`). P2.

---

## Gap list (consolidated, deduplicated)

### P0: data loss / crash / wrong data

| # | Gap | Pointer | Notes |
|---|---|---|---|
| P0-1 | An export shares the database session with the consoles. A console statement that runs during an export loses its cursor ("cursor __boa does not exist"). | `crates/boa-core/src/pg/export.rs:101`, `docs/phase7/README-csv.md:91-94` | *(p9/core-conc)*. The Tauri app has the same shape, so this is a shared bug, not a port regression. |
| P0-2 | **Quit from the Dock, logout or restart does not ask about open transactions.** The open transaction is rolled back silently. The source held these through `RunEvent::ExitRequested`. Only ⌘Q, the menu and closing the last window are guarded. | `crates/boa-app/src/app.rs:204-216`, `docs/phase6/README.md` (Deviations); src `crates/boa-desktop/src/lib.rs:345-360` | GPUI has no `applicationShouldTerminate:` hook. Fixing it needs an isolated `unsafe` delegate, which needs the user's OK (HANDOFF §12). |

No crash was seen in any of the live runs. Every view, menu and dialog that was
exercised rendered and responded.

### P1: missing feature / wrong behaviour / missing verification

| # | Gap | Pointer | Notes |
|---|---|---|---|
| P1-1 | **Open overlays don't block the global shortcuts.** In the source, App.tsx returns early whenever `[role=dialog]` is open, and ⌘⇧↵ / ⌘⇧⌫ are ignored over dialogs. In GPUI, `pass_through` only checks Settings and the palette. **Verified live:** with the value editor open on `orders` and text typed into it, ⌘W closed the `orders` tab and dropped the text. From the code (not run): ⌘⇧↵ / ⌘⇧⌫ commit or roll back while History, the value editor or the JSON viewer is open; ⌘O opens the dialog over History; ⌥B/⌥E/⌥J typed in the History search toggle panels. | `crates/boa-app/src/views/root.rs:334-343`, `:380-384`, `:596`; `grid/render.rs:910-912`; src `App.tsx:104,114`, `GuardDialogs.tsx:193` | No test covers it |
| P1-2 | **SSH tunnels are not closed at quit.** The source calls `close_tunnels()` on `RunEvent::Exit`. GPUI never calls `Backend::close_tunnels`, and `Runtime::drop` doesn't run because the `AppState` global is never dropped. Orphaned `ssh -N` processes (and their local ports) are likely left running after ⌘Q. | `crates/boa-core/src/backend.rs:127-134,239`; no caller in `crates/boa-app`; src `lib.rs:363-364` | Fix: an `on_app_quit` hook in `app.rs` |
| P1-3 | **⌘W never closes a window.** In Tauri, the menu's Close Window carries ⌘W. It closes the About window, and it closes the main window when no tab is open (App.tsx only calls `preventDefault` when it closes a tab). In GPUI, `CloseWindow` has no key and ⌘W is `CloseTab` in `Workspace` only. | `crates/boa-app/src/keymap.rs:173`, `actions.rs:42`, `app.rs:165,312,327` | The menu also shows no ⌘W |
| P1-4 | Updates: download, signature check, install and relaunch are a stub (`not_yet`). The open-transaction guard before a restart is not wired. Without `BOA_UPDATE_ENDPOINT` nothing is checked. | `crates/boa-app/src/models/updates.rs:31,180-190,204-210,225-229`; `boa-core state/guards.rs:100` | *(p8)* |
| P1-5 | The table toolbar has no **DDL** button and no **Filter loaded rows** box. The Export disable rule differs. | `crates/boa-app/src/views/workspace.rs:665-737` vs src `Workspace.tsx:638-649`; see `docs/audit/table-edits.png` vs `docs/audit/src/table-edits.png` | *(p9/fixes1)* |
| P1-6 | The new row (⌘N) shows `<default>` in every column. The source shows `<null>` for nullable columns without a default (seen side by side). | `docs/audit/src/table-edits.png` | *(p9/fixes1)* |
| P1-7 | A scope change reloads the catalog during a table fetch: "current transaction is aborted". | `docs/phase7/README-palette-settings.md:72-73` | *(p9/core-conc)* |
| P1-8 | **Manual checks never run:** SSH end to end; sleep/wake; network change; a real 28P01; real ⌘Q, menu quit and the close button with an open tx; reconnect/disconnect guards; ⌘⇧↵ / ⌘⇧⌫ on a real keyboard; the native Edit menu; the ⌥ rule on a German layout; trackpad scrolling; IME; drag-reordering tabs and connections. | `docs/phase2/README.md:50`, `docs/phase6/README.md`, `docs/phase4/integration.md` | The user's M list |
| P1-9 | **The context menus have no tests.** Nothing calls `Sidebar::build_menu`. The items, enable rules and click targets of the explorer menus, the title-bar "+" / "…" menus and the grid menu's edit section are covered only by screenshots. Show DDL from the explorer to a new console has no app-level test. | `crates/boa-app/src/views/sidebar.rs:544-718`, `views/title_bar.rs:363-460`, `grid/interact.rs:778-791` | Verification gap |
| P1-10 | **Migration not proven both ways.** `real_migration_copy` only parses a copy. The Tauri app has never been run on files the GPUI app wrote. | `crates/boa-app/src/models/session/tests.rs` (`#[ignore]`) | §11 item 25 |
| P1-11 | **No performance numbers for the full app:** startup and idle memory vs. Tauri, and typing latency in a 2,000-line console. Only the Phase 0 skeleton was measured, plus the grid at 5k rows. | `docs/phase0/numbers.md`, `docs/phase4/README-grid.md` | §11 item 27 |

### P2: visual / polish (behavioural ones first)

**Behaviour**

- **Guard dialogs accept keys the source ignores.** ⌘⇧↵ answers Commit and ⌘⇧⌫ answers Roll back.
  (`crates/boa-app/src/keymap.rs:188-189`, `views/guard_dialogs.rs:111`)
- **Ctrl variants of the grid edit keys are missing.** The source accepts Ctrl+N, Ctrl+⌥N, Ctrl+⌫ and Ctrl+↵ as well as ⌘.
  (src `DataGrid.tsx:259-266`; GPUI `crates/boa-app/src/keymap.rs:244-254`)
- **⌘K/⌘P, ⌘⌥E and ⌘, are dead while a gpui-component dialog has focus.** These dialogs are the connection dialog, password prompt, guard dialogs and edit preview. Their layer is outside the `Workspace` context. Inferred from the code, not run.
- **The grid menu's Copy as › trigger is never disabled.** Disabled formats show no reason tooltip.
  (`crates/boa-app/src/grid/interact.rs:767-776` vs src `DataGrid.tsx:725`)
- **"Revert row" in the grid menu shows no ⌘⌥Z hint.** The menu sends `RevertRow`, but the key is bound to `Revert`.
  (`grid/interact.rs:789`, `keymap.rs:249`)
- **Menu items the source doesn't have:**
  - Connection menu: New query console, Refresh, Delete…
  - Table menu: Copy name, Refresh.
  - App menu: Settings…, Show All.
  - Decide whether to keep them and record the decision in HANDOFF §11.
  - (`views/sidebar.rs:582-619,691-699`, `app.rs:302,308`)
- **Title-bar "…" menu:**
  - "Disconnect" is not styled destructive (`title_bar.rs:448`).
  - "Copy connection URL" copies an empty string when there is no saved config (`title_bar.rs:394-404`).
- **View › Enter Full Screen has a fixed label** (`app.rs:322`).
- **Enter in About repeats on a held key** (`keymap.rs:329` vs src `AboutWindow.tsx:53`, `!e.repeat`).
- **Extra keys the source doesn't have:** grid PgUp/PgDn and statement chooser PgUp/PgDn.
- **The save panel has no csv filter.** It opens in the last folder or `~/Downloads` (`models/exports.rs:155,209`).
- **Docs and verification:**
  - USAGE says session restore retries for "about half a minute"; it actually retries about 2 min, as in the source code (`boa-core state/reconnect.rs:34`). Fix the doc and §11.
  - USAGE says console results "say why" they are read-only. Neither app shows a reason.
  - HANDOFF §10 is missing a number of keys (explorer arrows, dialog Enter/Esc, Ctrl+↵, …). See §2.
  - `docs/phase1/README.md:107` is stale.
- **In flight** *(p9/fixes1)*: preview-tab italics (no italic Geist Mono), explorer chevron focus, status-bar fallback connection, export `.part` file on quit.

**Look:** see §4 for the side-by-side list.

---

## 1. USAGE.md, sentence by sentence

### Audit: Layout, Data sources, SSH tunnel, Editing table data, Filtering and sorting

**Test file aliases** (all `T:` names are exact fn names): **ST** `crates/boa-app/src/views/sidebar/tests.rs` · **TT** `crates/boa-app/src/views/sidebar/tree_tests.rs` · **WT** `crates/boa-app/src/views/workspace/tests.rs` · **AT** `crates/boa-app/src/tests.rs` · **CD** `crates/boa-app/src/views/connection_dialog/tests.rs` · **CT** `crates/boa-app/src/models/connections/tests.rs` · **TE** `crates/boa-app/src/views/table_edits/tests.rs` · **GT** `crates/boa-app/src/grid/tests.rs` · **SE** `crates/boa-app/src/models/session/tests.rs` · **TM** `crates/boa-core/src/grid/table_meta.rs` · **CAT** `crates/boa-core/src/pg/catalog.rs` · **SSH** `crates/boa-core/src/ssh.rs` · **GE** `crates/boa-core/src/grid/grid_edits.rs` · **ED** `crates/boa-core/src/edits.rs` · **EDS** `crates/boa-core/src/grid/edits.rs` · **TQ** `crates/boa-core/src/grid/table_query.rs`

The backend files `ssh.rs` and `connections.rs` are byte-identical to `src:crates/boa-desktop/src/`. The tunnel code in `pg.rs` and the SQL in `catalog.rs` are unchanged (the diffs only move types around).

#### Layout

| # | Claim (short) | Status | Evidence | Pointer / note |
|---|---|---|---|---|
| L1 | Explorer tree: connection › database › schema › tables › columns | works | T:node_ids_are_the_sidebar_tsx_ids (TT), S:docs/phase3/explorer-tree.png | `views/sidebar/tree.rs`; chevron focus fix in flight |
| L2 | Editor holds query consoles and table data editors | works | T:cmd_t_cmd_w_and_option_digits_drive_the_tabs, T:live_table_tabs_load_their_first_page (WT) | `views/workspace.rs` |
| L3 | Database Output panel is full-width | works | S:docs/phase1/main-result.png | `views/root.rs:3,584-590` |
| L4 | Output panel shows result grids and the Output log | works | T:live_run_shows_the_result_in_the_database_output, T:live_errors_show_their_position_in_the_output_log (WT) | `views/bottom_panel.rs` |
| L5 | ⌥J toggles it | works | T:option_b_e_j_toggle_the_panels_from_the_editor (AT) | `keymap.rs:179` |
| L6 | Breadcrumbs in the status bar | works | T:the_status_bar_shows_the_crumbs_and_the_caret (WT) | `views/status_bar.rs:65`; fallback fix in flight |

#### Data sources

| # | Claim (short) | Status | Evidence | Pointer / note |
|---|---|---|---|---|
| D1 | PostgreSQL only | works | code-only | — |
| D2 | ⌘O adds a data source | works | T:cmd_o_opens_the_dialog_in_the_main_window (CD) | — |
| D3 | + in explorer → Data source | works | code-only | `views/sidebar.rs:910-926` = src:Sidebar.tsx:185-194 |
| D4 | Pasting a `postgresql://` URL fills the form | works | T:pasting_a_url_fills_the_fields (CD), S:docs/phase2/dialog-url-pasted.png | `state/conn_url.rs` unit tests |
| D5 | Connect | works | T:connects_to_boa_dev (CT) | — |
| D6 | Browse schemas, tables, columns | works | T:catalogs_load_the_default_pick_on_connect (ST), T:child_counts_keys_and_details (TT) | — |
| D7 | Open table data | works | T:live_table_tabs_load_their_first_page (WT) | — |
| D8 | Run statements from a console with ⌘↵ | works | T:live_run_shows_the_result_in_the_database_output (WT) | — |
| D9 | Only picked databases and schemas are shown and introspected | works | T:databases_follow_the_scope_pick, T:schemas_follow_the_scope_pick_and_show_loading (TT), T:scope_changes_load_and_drop_schemas (ST) | — |
| D10 | "N of M" badge on a connection or database opens the picker | works | T:scope_picker_enter_applies_and_escape_cancels (ST), S:docs/phase3/scope-picker.png | `views/scope_picker.rs` |
| D11 | Defaults: the connection's database, `public` | works | T:catalogs_load_the_default_pick_on_connect (ST) | — |
| D12 | keys / foreign keys / indexes / checks folders after the columns | works | T:groups_keys_foreign_keys_indexes_and_checks_in_order (TM), S:explorer-tree.png | — |
| D13 | Empty folders are left out | works | T:leaves_out_empty_folders (TM) | — |
| D14 | Read with the schema, one catalog query each | works | T:attaches_to_the_right_tables (CAT) | Same SQL as the source |
| D15 | PK column: yellow key; FK column: blue key | works | T:marks_primary_key_and_foreign_key_columns (TM), T:child_counts_keys_and_details (TT), S:explorer-tree.png | Colour is code-only, `views/sidebar/row.rs:121-122` |
| D16 | Right-click → Copy name | works | code-only, S:docs/phase3/context-menu.png | `views/sidebar.rs:691,701,706`; table menu has extra items (see Gaps) |
| D17 | Right-click → Copy definition on key / index / check entries | works | code-only | `views/sidebar.rs:703-711` |
| D18 | DDL button in the table toolbar | missing | none | `views/workspace.rs:675-737` vs src:Workspace.tsx:640-649 (in flight, p9/fixes1) |
| D19 | "Show DDL" in the explorer opens a new console | works | T:opens_the_ddl_in_a_new_console_on_that_database (`state/ddl.rs`); view wiring code-only | `views/sidebar.rs:187-226`; no GPUI test |
| D20 | DDL: CREATE TABLE with types, defaults, identity / generated columns, NOT NULL, constraints | works | T:table_ddl (CAT), T:live_postgres (`pg.rs`) | — |
| D21 | Then non-constraint indexes, triggers, COMMENT ON | works | T:table_ddl (CAT) | — |
| D22 | Views and materialized views come from `pg_get_viewdef` | works | T:view_and_matview_ddl (CAT) | — |
| D23 | Ownership, grants, storage options, collations left out | works | code-only | Same SQL as the source |
| D24 | Needs PostgreSQL 12+ | works | code-only | `CAT:12`; no version check (same in the source) |
| D25 | `JOIN t ON` completion offers FK conditions | works | T:foreign_key_join_conditions_first_after_on (`editor/completion.rs:183`) | `sql/complete.rs:925` |
| D26 | Settings saved in `connections.json` in the config dir | works | T:saving_writes_connections_json_with_the_password (CD) | — |
| D27 | Passwords stored in plain text | works | same test | — |
| D28 | File is owner-only (0600) | works | T:file_commands_from_a_plain_thread (`crates/boa-core/src/backend/tests.rs:60`) | — |

#### SSH tunnel

| # | Claim (short) | Status | Evidence | Pointer / note |
|---|---|---|---|---|
| S1 | "SSH tunnel" checkbox in the dialog | works | T:validation_disables_test_and_save (CD), S:docs/phase2/dialog-ssh.png | `views/connection_dialog.rs:912-980` |
| S2 | SSH host may be a host name, IP or `~/.ssh/config` alias | works | T:builds_ssh_arguments (SSH) | — |
| S3 | Port optional (22 or the config's `Port`) | works | T:builds_ssh_arguments | `-p` is omitted when empty |
| S4 | User optional (config `User` or login name) | works | T:builds_ssh_arguments | — |
| S5 | Auth: agent or key file (Browse…) | works | T:rejects_bad_settings (SSH), T:validation_disables_test_and_save (CD) | — |
| S6 | Host / Port above are as the SSH server sees them; `localhost` and socket directories work | works | T:builds_ssh_arguments (IPv6 and socket cases) | — |
| S7 | Runs `/usr/bin/ssh -N -L 127.0.0.1:<free port>:…` | works | T:builds_ssh_arguments | `SSH:22,157` |
| S8 | Batch mode; `~/.ssh/config`, ProxyJump, agent, known_hosts apply | works | code-only | `SSH:113` |
| S9 | Nothing is prompted; the host key must already be known | works | code-only | stdin null + BatchMode |
| S10 | No SSH password or passphrase stored | works | T:not_saving_the_password_leaves_it_out (CD) | — |
| S11 | ssh's own error is shown | works | T:explains_common_failures, T:unreachable_ssh_server_fails_cleanly (SSH) | — |
| S12 | One tunnel per data source carries all its sessions | works | code-only | `crates/boa-core/src/pg.rs:386-401` |
| S13 | Cancel goes through the tunnel | works | code-only | `pg.rs:85-87,856` |
| S14 | Test connection opens a tunnel of its own | works | code-only | `pg.rs:366-382` |
| S15 | Closed on disconnect | works | code-only | `pg.rs:445` |
| S16 | Closed when connecting fails | works | code-only | `pg.rs:343` |
| S17 | Closed at quit | partial | none | See Gaps (P1) |
| S18 | If ssh exits, the next statement reopens it | works | code-only | `pg.rs:390-399` |
| S19 | TLS runs end to end; Verify full checks the database's host name | works | code-only | `pg.rs:96-99` (`hostaddr` + `host`) |
| S20 | End-to-end check against a real SSH server | missing | none | docs/phase2/README.md:50 |

#### Editing table data

| # | Claim (short) | Status | Evidence | Pointer / note |
|---|---|---|---|---|
| E1 | Tables with a primary key are editable | works | T:read_only_reasons_follow_the_catalog, T:live_edit_preview_submit_changes_the_row (TE) | — |
| E2 | Views and key-less tables are read-only and say why | works | T:read_only_reasons_follow_the_catalog (TE), T:editability_explains_why_other_tabs_are_read_only (GE) | — |
| E3 | Console results are read-only "and say why" | works | code-only | `views/bottom_panel.rs:133`; the reason is only drawn with a table context (`grid/render.rs:595-599`). The source shows no reason either; USAGE overstates |
| E4 | Double-click, Enter or typing starts editing | works | T:enter_edits_and_stages_the_change, T:double_click_typing_and_tab (GT) | — |
| E5 | ⇧↵ opens the larger editor | works | T:value_editor_saves_with_cmd_enter (GT), S:docs/phase4/value-editor.png | — |
| E6 | ⌘⌥N sets NULL (not the same as an empty string) | works | T:each_editing_shortcut_stages_its_change (GT), T:treats_null_and_empty_string_as_different_values (GE) | — |
| E7 | ⌘N adds a row; untouched columns left out of the INSERT | works | T:live_add_row_with_defaults_delete_and_set_null (TE), T:inserts_only_the_columns_that_were_set_or_default_values (GE) | `<default>` placeholder in flight |
| E8 | ⌘⌫ deletes the row | works | T:each_editing_shortcut_stages_its_change | — |
| E9 | ⌘⌥Z reverts (the cell, else the row) | works | T:each_editing_shortcut_stages_its_change, T:reverts_a_cell_and_a_row (GE) | — |
| E10 | Same actions from the right-click menu | works | code-only, S:docs/phase4/grid-context-menu.png | `grid/interact.rs:779-791` |
| E11 | Ctrl variants of these keys | differs | code-only | Source uses ⌘ or Ctrl (src:DataGrid.tsx:259); `keymap.rs:244-252` binds ⌘ only |
| E12 | Changes are only staged until Submit | works | T:revert_all_and_the_preview_follow_the_staged_edits (TE) | — |
| E13 | Staged changes tinted per kind | works | S:docs/phase5/staged-edits.png | code-only |
| E14 | Staged changes kept per tab | works | T:pending_edits_ask_before_paging_and_closing (switches tabs with ⌥1 / ⌥2) | — |
| E15 | Preview (⌘↵) shows the exact statements | works | T:revert_all_and_the_preview_follow_the_staged_edits, S:docs/phase5/sql-preview.png | — |
| E16 | UPDATE / DELETE by the original key, then INSERT | works | T:uses_the_original_key_when_the_key_itself_is_edited, T:orders_deletes_then_updates_then_inserts (GE) | — |
| E17 | Runs in one transaction | works | T:own_transaction_or_savepoint (ED), T:live_failed_submit_keeps_the_edits_and_points_at_the_statement (TE) | — |
| E18 | Savepoint inside an open transaction | works | T:own_transaction_or_savepoint (ED), T:reports_changes_applied_in_an_open_transaction (EDS) | Unit tests only; no live app test |
| E19 | Each UPDATE / DELETE must hit exactly one row | works | T:exactly_one_row (ED) | — |
| E20 | Any error rolls everything back and points at the failing statement | works | T:live_failed_submit_keeps_the_edits_and_points_at_the_statement, S:docs/phase5/submit-error.png | — |
| E21 | Reload with pending changes asks first | works | code-only (button) | `views/workspace.rs:695` → `request`; model guard tested |
| E22 | Paging asks first | works | T:pending_edits_ask_before_paging_and_closing, T:live_paging_with_edits_asks_then_pages (TE) | — |
| E23 | Changing the filter or sort asks first | works | T:live_header_clicks_sort_on_the_server_shift_adds_columns (TE) | Asking on filter-bar Enter is code-only |
| E24 | Closing the tab asks first | works | T:pending_edits_ask_before_paging_and_closing | — |

#### Filtering and sorting table data

| # | Claim (short) | Status | Evidence | Pointer / note |
|---|---|---|---|---|
| F1 | WHERE and ORDER BY fields above the data | works | S:docs/phase5/filter-bar.png | `views/table_filter_bar.rs` matches src:TableFilterBar.tsx |
| F2 | Enter applies | works | T:live_filter_bar_applies_where_and_shows_server_errors_in_place (TE) | — |
| F3 | Emptying a field and pressing Enter removes it | works | same test (`apply ""`) | — |
| F4 | × clears the field and applies | works | T:clauses_that_escape_their_place_never_run (TE) | The test calls `clear()`, not a mouse click |
| F5 | Escape puts back the applied text | works | T:clauses_that_escape_their_place_never_run | — |
| F6 | Runs on the server as `select * from t where … order by …` | works | T:live_filter_bar_applies_where_and_shows_server_errors_in_place, T:table_select_adds_where_and_order_by_one_per_line (TQ) | — |
| F7 | Data starts again from the first page | works | code-only | `models/table_tab.rs:362` |
| F8 | Header click: ascending → descending → off | works | T:cycle_sort_goes_ascending_descending_off (TQ), T:table_tab_header_clicks_ask_for_a_server_sort (GT) | — |
| F9 | A column already sorted changes in place; another column replaces the sort | works | T:cycle_sort_changes_a_column_in_place_keeping_other_items (TQ), T:live_header_clicks_sort_on_the_server_shift_adds_columns | — |
| F10 | ⇧-click adds the column after the others | works | T:live_header_clicks_sort_on_the_server_shift_adds_columns | — |
| F11 | Header arrow, with its position when sorting by several columns | works | same test (ranks), S:filter-bar.png | — |
| F12 | Bad condition shows the server's error in place of the rows, keeping the typed text | works | T:live_filter_bar_applies_where_and_shows_server_errors_in_place, S:docs/phase5/where-error.png | — |
| F13 | `;`, unbalanced parentheses, unterminated string or comment refused before running | works | T:clauses_that_escape_their_place_never_run, T:clause_error_rejects_a_second_statement_and_broken_nesting (TQ) | — |
| F14 | Filter and sort kept per tab | works | code-only | `models/table_tab.rs:102-103` |
| F15 | Restored with the session | works | T:the_session_is_saved_and_restored_through_the_app (SE) | — |
| F16 | Export uses the filtered, sorted statement | works | code-only | `views/workspace.rs:665-673`; disable rule differs (see Gaps) |
| F17 | "Open query console" uses the filtered, sorted statement | works | code-only | `views/workspace.rs:662,729-736` |

#### Gaps

- **P1, SSH tunnels are not closed at quit (S17).** The Tauri app calls `close_tunnels()` on `RunEvent::Exit` (`src:crates/boa-desktop/src/lib.rs:363-364`). The GPUI app relies only on `Runtime::drop` (`crates/boa-core/src/backend.rs:127-134`). That drop never runs, because the `AppState` global is not dropped: gpui `App::shutdown` only clears windows (`gpui-pre-0.3.14/src/app.rs:1280-1310`). Nothing in `crates/boa-app` calls `Backend::close_tunnels` (`backend.rs:239`). So `ssh -N` processes (`kill_on_drop`, `ssh.rs:169`) are likely left running after ⌘Q. Fix: an `on_app_quit` hook in `app.rs` that calls `state.backend.close_tunnels()`.
- **P1, DDL button missing from the table toolbar (D18).** `views/workspace.rs:675-737` vs `src:Workspace.tsx:640-649`. In flight (p9/fixes1); the "Filter loaded rows" box is missing from the same toolbar.
- **P1 (verification), SSH never tested end to end (S20).** Every runtime SSH claim (S12–S19) is code-only. The code is identical to the source, but nothing has run against a real SSH server (`docs/phase2/README.md:50`).
- **P2, Ctrl variants of the grid edit keys (E11).** The source accepts Ctrl as well as ⌘ (Ctrl+N, Ctrl+⌥N, Ctrl+⌫, Ctrl+↵, `src:DataGrid.tsx:259-266`). `keymap.rs:244-252` binds ⌘ only.
- **P2, table context menu has extra items (D16).** GPUI adds "Copy name" and "Refresh" (`views/sidebar.rs:691,698`). The source table menu only has "Copy qualified name" (`src:Sidebar.tsx:612-615`).
- **P2, Export disable rule (F16).** GPUI disables Export on any load error (`t.error.is_some()`, `workspace.rs:672`); the source disables it only on a refused clause (`src:Workspace.tsx:638`). In flight.
- **P2, USAGE overstates console results (E3).** Console grids are read-only, but neither app shows a reason: GPUI draws it only with a table context (`grid/render.rs:595-599`). Fix the doc or the view.
- **P2 (test coverage).** These are wired but have no GPUI test:
  - "Show DDL" through to a new console (`views/sidebar.rs:187`; also listed as not verified in `docs/phase3/README.md:69`).
  - The Copy name / Copy definition clipboard actions.
  - Clicking the Reload button, and the × button.
  - Asking before discarding on filter-bar Enter.
  - Export and console using the filtered statement.
  - Resetting to the first page after a filter (F7).
- **Known in-flight work, not re-audited here:** grid `<default>` placeholders (E7), explorer chevron focus (L1), status-bar fallback (L6), `.part` file on quit during export, flaky sidebar test, and the p9/core-conc concurrency work.

Legend: `A/` = `crates/boa-app/src/`, `C/` = `crates/boa-core/src/`, `src:` = source repo. Tests marked *(live)* skip without a message when PGHOST is unset.

#### Selecting and copying rows

| # | Claim (short) | Status | Evidence | Pointer / note |
|---|---|---|---|---|
| 1 | Click selects a cell | works | T:click_shift_click_and_copy_tsv (A/grid/tests.rs) | A/grid/interact.rs:96 |
| 2 | Drag selects a rectangle | works | T:drag_selects_a_range | interact.rs:182 |
| 3 | ⇧-click extends | works | T:click_shift_click_and_copy_tsv | A/grid/selection.rs:147 |
| 4 | ⇧-arrow extends | works | T:keyboard_selection_cmd_a_and_esc, T:shift_arrows_extend_and_arrows_collapse | A/keymap.rs:236 |
| 5 | Row number selects the whole row | works | T:row_numbers_pick_rows | selection.rs:158 |
| 6 | Drag or ⇧-click on row numbers selects a run | works | T:row_numbers_pick_rows | |
| 7 | ⌘-click adds or removes a row | works | T:row_numbers_pick_rows, T:cmd_click_after_cmd_a_removes_one_row | interact.rs:136 |
| 8 | ⌘A selects every loaded row | works | T:keyboard_selection_cmd_a_and_esc | Needs an active cell, as in src DataGrid.tsx:303 |
| 9 | Esc goes back to one cell | works | same | selection.rs:247 |
| 10 | ⌘C copies TSV | works | T:click_shift_click_and_copy_tsv | interact.rs:288 |
| 11 | One cell copies just its text | works | same ("Copied value") | |
| 12 | Right-click menu has Copy as › | works | T:right_click_opens_the_menu_and_copy_runs_from_it; S:docs/phase4/grid-context-menu.png | A right-click inside the range keeps it |
| 13 | TSV ± header, CSV, MD, JSON, INSERT | works | T:every_copy_as_format_via_its_menu_action | |
| 14 | UPDATE by PK, non-key columns only | works | T:writes_updates_by_primary_key, T:makes_update_unavailable_without_a_table_a_key_or_a_non_key_column (C/grid/copy_as.rs) | Disabled without a table or PK |
| 15 | IN list, one column only | works | T:writes_an_in_list_for_a_single_column_only | |
| 16 | Copies the shown values, pending edits included | works | T:staged_edits_show_and_copy, T:new_rows_show_defaults_and_copy_as_default (A/grid/page.rs) | Model test only |
| 17 | NULL written per format | works | T:writes_a_markdown_table, T:writes_sql_literals_per_type, T:json_parses_with_the_right_types | |
| 18 | Numbers and booleans bare in SQL and JSON | works | T:writes_sql_literals_per_type, T:quotes_numbers_that_are_not_json_numbers_and_invalid_json | |
| 19 | json embedded, arrays become JSON arrays | works | T:writes_postgres_arrays_as_json_arrays_of_the_element_type | |
| 20 | Escaping per format (SQL `'`, MD `\|`/br, RFC 4180) | works | T:quotes_per_rfc_4180_in_csv, T:writes_a_markdown_table | |
| 21 | INSERT goes to the table, else MY_TABLE | works | T:uses_the_table_name_quotes_identifiers_and_writes_default, T:writes_one_insert_per_row_into_my_table_for_console_results | |

#### Transactions and guard rails

| # | Claim | Status | Evidence | Pointer / note |
|---|---|---|---|---|
| 1 | One session per database, shared by consoles and tables | works | T:sessions_follow_outcomes_and_connection_events (A/models/transactions/tests.rs) | A/models/transactions.rs:83 |
| 2 | Tx: Auto / Manual menu | works | S:docs/phase6/tx-menu.png | A/views/tx_controls.rs |
| 3 | Manual mode opens a tx before the first statement | works | T:live_manual_mode_commit_and_rollback *(live)* | C/pg.rs:674, C/tx.rs opens_tx |
| 4 | ⌘⇧↵ / ⌘⇧⌫ commit / roll back | works | T:shortcuts_apply_only_with_a_transaction_open, T:live_manual_mode_commit_and_rollback | keymap.rs:181 |
| 5 | Typed commit / rollback | works | T:live_typed_begin_commit_and_rollback | |
| 6 | Server probed after every statement | works | T:outcomes_report_the_state_on_success_and_in_errors | C/tx.rs:84 |
| 7 | Aborted (25P02) until rolled back | works | T:live_aborted_transaction_and_leaving_manual_mode | |
| 8 | State shown in console toolbar and status bar | works | T:the_status_bar_shows_manual_mode_and_open_transactions; S:docs/phase6/tx-open.png | |
| 9 | Typed `begin` works in either mode | works | T:live_typed_begin_commit_and_rollback | |
| 10 | Disconnect with a tx open asks, then rolls back | works | T:disconnecting_with_an_open_transaction_asks, T:live_disconnect_with_an_open_transaction_asks_and_rolls_back | A/models/connections.rs:914 |
| 11 | Reconnect after editing asks | works | T:the_close_guard_can_keep_a_connection_open (connections/tests.rs) | connections.rs:619 |
| 12 | Quit asks | partial | T:quitting_with_an_open_transaction_asks_and_a_second_cmd_q_quits, T:quit_cancel_stays_roll_back_and_quit_quits; S:quit-guard.png | Dock quit, logout and restart are not held (GPUI has no applicationShouldTerminate). Source held them (src:lib.rs:357) |
| 13 | Tx lost with a drop is reported | works | T:warns_about_transactions_rolled_back_with_a_dropped_session | |
| 14 | UPDATE / DELETE without WHERE asks | works | T:flags_update_and_delete_without_where_naming_the_table, T:live_destructive_confirmation | A/models/console.rs:192 |
| 15 | TRUNCATE, DROP, DROP COLUMN ask | works | T:flags_truncate_and_drop, T:flags_dropped_columns_not_dropped_defaults_or_constraints | C/sql/guard.rs |
| 16 | Tokenizer not fooled by comments, strings, CTEs | works | T:is_not_fooled_by_comments_strings_and_quoted_names, T:looks_into_ctes_in_both_directions | |
| 17 | Setting can turn the confirmation off | works | T:destructive_statements_ask_first_unless_turned_off_production_always | |
| 18 | …except on Production | works | same; S:destructive-confirm.png | |
| 19 | Confirmation dialog keys | differs | code-only | Enter = Cancel. ⌘⇧↵ / ⌘⇧⌫ answer the dialog; the source ignored them |
| 20 | Read-only enforced by the server | works | T:live_read_only_connections_refuse_writes, T:a_read_only_connection_is_enforced_by_the_server | C/pg.rs:120 |
| 21 | Read-only put back if turned off | works | T:live_postgres (C/pg.rs:1763) | C/pg.rs:926 restore_read_only |
| 22 | Env tag / colour on explorer row | works | T:the_production_tag_and_badges_render; S:docs/phase3/production-tag.png | A/views/sidebar/row.rs:173 |
| 23 | …on the editor (stripe) | works | code-only | A/views/workspace.rs:915 |
| 24 | …on console toolbar and status bar | works | code-only | workspace.rs:605, status_bar.rs:288 |

#### Query history

| # | Claim | Status | Evidence | Pointer / note |
|---|---|---|---|---|
| 1 | ⌘⌥E opens it | works | T:cmd_alt_e_toggles_it_and_escape_closes_it | keymap.rs:169 |
| 2 | Console toolbar button | works | code-only | workspace.rs:595 |
| 3 | "Query History" in ⌘K | works | T:lists_tables_fuzzy_matches_and_moves_with_arrows_and_tab (command_palette.rs) | |
| 4 | Saved per connection in history-<id>.jsonl, same format | works | T:writes_the_tauri_apps_jsonl_format | |
| 5 | File is owner-only | works | T:append_list_delete_clear (C/history.rs:277) | |
| 6 | Deleted with the connection | works | T:file_commands_from_a_plain_thread (C/backend/tests.rs) | C/backend.rs:270 |
| 7 | Failed runs saved with their error | works | T:records_runs_and_failures_from_the_output_log_not_page_fetches | |
| 8 | 30 days by default, 7 days to forever | works | T:retention_drops_old_runs_and_follows_the_setting | C/state/prefs.rs:54 |
| 9 | At most 5,000 per connection | works | T:keeps_at_most_five_thousand_per_connection | |
| 10 | A rerun replaces its entry, even with other values / IN list length | works | T:a_query_run_again_replaces_its_entry, T:ignores_values, T:counts_values_as_one_only_in_an_in_list | |
| 11 | Paging doesn't add entries | works | T:records_console_runs_and_failures_not_page_refetches *(live)* | |
| 12 | Search matches words as you type | works | T:matches_every_word_case_insensitively_in_any_order, T:arrows_and_pages_move_the_selection | |
| 13 | Errors-only filter | works | T:filters_errors_only | |
| 14 | All-connections toggle | works | T:all_connections_merge_newest_first | |
| 15 | ⏎ inserts at the console's caret | works | T:enter_inserts_at_the_consoles_caret | No console: opens one, as src |
| 16 | ⌘⏎ new console, ⌘C copies | works | T:cmd_enter_opens_a_new_console_and_cmd_c_copies | |
| 17 | ⌘⌫ deletes | works | T:cmd_backspace_deletes_the_selected_entry | |
| 18 | Clear history | works | T:deletes_entries_and_clears_a_connection (model) | Dialog is code-only: A/views/query_history.rs:560 |
| 19 | Recording off in Settings → Queries | works | T:records_nothing_when_turned_off, T:every_switch_and_choice_writes_prefs_json | |
| 20 | Output log (⌥J) is session-only | works | code-only | keymap.rs:179 |

#### CSV export

| # | Claim | Status | Evidence | Pointer / note |
|---|---|---|---|---|
| 1 | Export menu on result grid | works | T:the_export_menu_sets_options_copies_and_exports; S:docs/phase7/csv-export-menu.png | A/views/bottom_panel.rs:456 |
| 2 | …on table toolbar | partial | T:table_tab_sources_export_what_the_tab_shows (model) | workspace.rs:665. Disable rule in flight (p9/fixes1) |
| 3 | Explorer "Export to CSV…" | works | T:exports_the_whole_orders_table *(live)* | A/views/sidebar.rs:686 |
| 4 | Asks for a file | works | T:export_asks_where_to_save | No csv type filter. Opens in last folder or ~/Downloads |
| 5 | Writes the whole result, not the page | works | T:exports_the_whole_orders_table (100k rows) | |
| 6 | Uses COPY … TO STDOUT (FORMAT csv) | works | T:copy_statement_options | C/pg/export.rs:51 |
| 7 | Temp file renamed into place | works | T:atomic_write_replaces_and_leaves_no_temp, T:temp_file_is_a_hidden_sibling | `.part` left on quit (in flight, p9/fixes1) |
| 8 | Progress in the status bar | works | T:a_running_export_shows_progress_and_cancel; S:csv-export-progress.png | A/views/csv_export.rs:290 |
| 9 | Cancel in the status bar | works | T:cancel_leaves_no_file *(live)* | A/models/exports.rs:323 |
| 10 | Toast and Output log line at the end | works | T:statements_that_change_data_write_their_loaded_rows; S:csv-export-done.png | |
| 11 | Only read-only statements are re-run | works | T:inside_a_transaction_a_savepoint_keeps_it_intact | export.rs:164, cursor probe |
| 12 | Runs in its own transaction | works | code-only | export.rs:112 |
| 13 | Inside a user tx: savepoint, rolled back; uncommitted rows included | works | T:inside_a_transaction_a_savepoint_keeps_it_intact *(live)* | Shares the console's session (p9/core-conc) |
| 14 | INSERT … RETURNING writes its loaded rows | works | T:statements_that_change_data_write_their_loaded_rows, T:read_only_statements_stream_and_changes_write_the_loaded_rows | |
| 15 | Menu copies the loaded page as CSV | works | T:the_export_menu_sets_options_copies_and_exports | |
| 16 | Delimiter (comma / tab / semicolon) and header row | works | same + T:options_are_read_from_and_saved_to_csv_export_json | |
| 17 | NULL is empty, empty string is `""` | works | T:csv_field_writes_null_empty_and_empty_string_quoted | |

#### Updates

| # | Claim | Status | Evidence | Pointer / note |
|---|---|---|---|---|
| 1 | Checks shortly after launch | partial | code-only | 20 s (A/models/updates.rs:204). Only runs if BOA_UPDATE_ENDPOINT is set, so release builds never check (p8) |
| 2 | Checks every few hours | partial | code-only | 6 h. Same endpoint gate |
| 3 | Downloads in the background | missing (p8) | T:a_downloaded_update_shows_the_restart_pill (dev downloader only) | updates.rs:180 errors "can't download updates yet" |
| 4 | Status bar shows "Restart to update to X" | partial | T:a_downloaded_update_shows_the_restart_pill; S:docs/phase7/status-bar-update.png | Click → not_yet (updates.rs:228) |
| 5 | Asks first if a tx is open | missing (p8) | none | CloseAction::Update exists (C/state/guards.rs:100) but install() never calls the guard |
| 6 | Tabs come back after the restart | missing (p8) | none | No relaunch |
| 7 | Boa → Check for Updates… checks now | partial | T:check_for_updates_reports_in_about, T:a_newer_release_over_http_is_found | A/app.rs:139. Without an endpoint it reports "not available in this build yet" |
| 8 | About Boa shows the version | works | T:lists_version_commit_date_and_os; S:docs/phase7/about.png | |
| 9 | Copy version info | works | T:enter_in_about_copies_the_version_info | |

#### Session restore

| # | Claim | Status | Evidence | Pointer / note |
|---|---|---|---|---|
| 1 | Open tabs reopen | works | T:the_session_is_saved_and_restored_through_the_app; S:docs/phase7/session-restored.png | A/models/session.rs:113 |
| 2 | Each console's SQL | works | same | Not re-run |
| 3 | Table filter and sort | works | same | |
| 4 | Active tab | works | same | |
| 5 | Panel layout | works | same + T:a_scripted_layout_keeps_the_saved_panel_flags | Sizes are kept in prefs |
| 6 | Reconnects everything connected at quit | works | T:reopens_the_selected_connection_first_then_the_others_quietly, T:a_pending_save_is_written_on_quit | session.rs:176 |
| 7 | Selected connection first | works | same | |
| 8 | Others skip the password prompt | works | T:restore_reconnects_quietly | |
| 9 | Retries for about half a minute | differs | T:reconnects_retrying_while_the_server_is_unreachable, T:gives_up_after_the_retries_and_keeps_the_connection_wanted | About 2 min, same as src code. USAGE text is off |

#### Staying connected

| # | Claim | Status | Evidence | Pointer / note |
|---|---|---|---|---|
| 1 | Stays connected until you disconnect | works | T:disconnecting_stops_reconnecting_and_forgets_the_connection_was_wanted | |
| 2 | Drop noticed within about 1 min (keepalives) | works | code-only | C/pg.rs:107 (30 s + 3×10 s) |
| 3 | Session reopened on next use | works | T:live_dropped_session *(live)*, T:a_terminated_session_reconnects | |
| 4 | Safe statements re-run (pages, TABLE, SHOW) | works | T:only_rerunnable_statements_are_retried | pg.rs:210 |
| 5 | Explorer re-runs | works | code-only | pg.rs:496 |
| 6 | Anything else fails "connection lost" | works | T:live_dropped_session | pg.rs:695 |
| 7 | Drop during COMMIT: outcome unknown | works | T:tx_lost_and_commit_unknown_messages | Message mapping only, no live test |
| 8 | Server away shows "reconnecting" in amber | works | T:shows_reconnecting_while_the_server_is_away_and_says_when_it_is_back | A/views/sidebar/row.rs:202 |
| 9 | Growing delays for about 2 min | works | T:the_backoff_lasts_about_two_minutes | C/state/reconnect.rs:34 |
| 10 | Then failed, with a Retry toast | works | T:gives_up_after_the_last_retry_offering_to_retry | |
| 11 | Wake retries at once | works | T:a_wake_up_retries_a_failed_wanted_connection_right_away | A/platform/macos.rs |
| 12 | Network coming back retries | works | T:a_network_change_checks_once_it_settles | |
| 13 | "Transaction rolled back" on a lost tx | works | T:warns_about_transactions_rolled_back_with_a_dropped_session | |
| 14 | Next statement isn't run as if the tx were still open | works | T:live_dropped_session | pg.rs:669 |
| 15 | Running a statement connects first | works | T:running_a_query_connects_first | console.rs:152 |
| 16 | Selecting a database connects first | works | code-only | A/models/workspaces.rs:837 |
| 17 | Connect button on table tabs | works | code-only | workspace.rs:853 |

#### Gaps

- **P0 (in flight, p9/core-conc):** an export shares the database session with consoles. A console statement running at the same time loses its cursor ("cursor __boa does not exist"). `C/pg/export.rs:101`, `docs/phase7/README-csv.md:96`.
- **P1:** Dock → Quit, logout and restart don't ask about open transactions; the server rolls them back silently. The source held these. `A/app.rs:204`, `docs/phase6/README.md` (Deviations). Borderline P0, since uncommitted work is lost without a prompt.
- **P1 (p8):** downloading, verifying, installing and relaunching updates are missing. `A/models/updates.rs:180-190` (always errors), `:225-229` (`install` → `not_yet`).
- **P1 (p8):** the open-transaction guard before an update restart isn't wired. `C/state/guards.rs:100` exists, but `A/models/updates.rs:225` never calls `confirm_close_tx`.
- **P1 (p8):** automatic and manual update checks do nothing without `BOA_UPDATE_ENDPOINT`, and the real feed isn't the default. `A/models/updates.rs:31`, `:204-210`.
- **P1 (in flight, p9/fixes1):** the table toolbar Export menu's disable rule. `A/views/workspace.rs:665`.
- **P2 (in flight, p9/fixes1):** the export `.part` file is left behind on quit. `C/pg/export.rs:66`.
- **P2:** USAGE says session restore retries for "about half a minute", but the code (and the source code) retries for about 2 min. `C/state/reconnect.rs:34`. Fix the doc.
- **P2:** guard dialog keys differ from the source: Enter = Cancel, and ⌘⇧↵ / ⌘⇧⌫ answer the dialog (the source ignored them). `A/views/guard_dialogs.rs:113`.
- **P2:** the save panel has no csv/tsv filter, and opens in the last folder or ~/Downloads instead of letting macOS choose. `A/models/exports.rs:155`.
- **P2 (no view test):** these work but have no UI-level test:
  - editor stripe: `A/views/workspace.rs:915`
  - env tag on the console toolbar and status bar: `workspace.rs:605`, `status_bar.rs:288`
  - table-tab Connect / Retry now button: `workspace.rs:853`
  - selecting a database connects first: `A/models/workspaces.rs:837`
  - "Clear history…" dialog: `A/views/query_history.rs:560`
  - Copy as with staged edits (only model-tested): `A/grid/page.rs:581`
- **P2 (evidence):** the live tests (tx, export, dropped session, read-only) skip without PGHOST and print only to stderr, so a green CI run doesn't prove they ran.

---

## 2. Keyboard shortcuts (HANDOFF §10 + everything the source UI handles)

**Checked live in the real app** (dev build, `BOA_DEV_INPUT`, real keymap):

- ⌘T, ⌘↵ (arm, then run), ⌘K + typing, Esc, ⌘, (Settings), ⌘O over Settings, ⌥E.
- Double-click → pinned table tab.
- Grid: typing into a cell + Enter, ⌘⌥N, ⌘⌫, ⌘N, PgUp, ⇧↵ (value editor).
- **⌘W while the value editor is open**: closes the tab and loses the typed text (P1-1).

The rest is covered by the gpui tests listed below.

### Keyboard shortcut audit

Path prefixes: `K` = `crates/boa-app/src/keymap.rs`. `src:` = `crates/boa-desktop/ui/src/components/` in the source repo. `App.tsx` is directly under `ui/src/`, and `src:lib.rs` / `src:updater.rs` are under `crates/boa-desktop/src/`.

Test files (all under `crates/boa-app/src/`): **A** `tests.rs` · **WS** `views/workspace/tests.rs` · **RT** `views/root/tests.rs` · **TX** `models/transactions/tests.rs` · **ED** `editor/tests.rs` · **GR** `grid/tests.rs` · **TE** `views/table_edits/tests.rs` · **QH** `views/query_history/tests.rs` · **CP** `views/command_palette.rs` (tests mod) · **CD** `views/connection_dialog/tests.rs` · **SB** `views/sidebar/tests.rs` · **ST** `views/settings.rs` (tests mod) · **KU** `keymap.rs` unit tests.

#### Global (App.tsx `useShortcuts`, GuardDialogs, Tauri)

| Keys | Action | Source file:line | GPUI binding (context, file:line) | Verified by | Status |
|---|---|---|---|---|---|
| ⌘K, ⌘P | toggle palette, close Settings | src:App.tsx:92 | Workspace, K:167-168 | T:CP `cmd_k_and_cmd_p_toggle_the_palette_and_esc_closes_it`, `palette_closes_settings_and_settings_closes_the_palette` | works (dead over dialogs, see Gaps) |
| ⌘⌥E | toggle history, close palette and Settings | App.tsx:97 | Workspace, K:169 | T:QH `cmd_alt_e_toggles_it_and_escape_closes_it` | works |
| ⌘O | new connection, not over a dialog | App.tsx:104 | Workspace, K:170; gate root.rs:380-384 | T:CD `cmd_o_opens_the_dialog_in_the_main_window` (no test of the "not over a dialog" case) | partial |
| ⌘, | toggle Settings | App.tsx:109 | Workspace, K:171 | T:A `cmd_comma_toggles_settings_and_escape_closes_it` | works |
| ⌘T | new console | App.tsx:119 | Workspace, K:172 | T:WS `cmd_t_cmd_w_and_option_digits_drive_the_tabs`, `without_a_database_the_shortcuts_do_nothing` | partial (overlay gate) |
| ⌘W | close tab | App.tsx:122 | Workspace, K:173 | same | partial (overlay gate) |
| ⌥N | new console (`newQueryTarget`) | App.tsx:142 | Workspace, K:174 | same | partial (overlay gate) |
| ⌥1–⌥9 | activate tab n | App.tsx:140 | Workspace, K:192-197 | T:WS `cmd_t_cmd_w_…`, T:TE `pending_edits_ask_before_paging_and_closing` | partial (overlay gate) |
| ⌥↑ / ⌥↓ | previous / next database | App.tsx:134 | Workspace, K:175-176 | T:WS `option_arrows_cycle_databases` | partial (overlay gate) |
| ⌥B / ⌥E / ⌥J | sidebar / right panel / Output | App.tsx:136-138 | Workspace, K:177-179 | T:A `option_b_e_j_toggle_the_panels_from_the_editor`, T:RT `the_shortcuts_survive_the_focused_view_going_away` | partial (overlay gate) |
| ⌥ that types a character | leave it to text (`isOptionShortcut`) | App.tsx:75-79 | K:135-154, root.rs:334 | T:A `option_keys_that_compose_text_are_left_to_the_editor`; T:KU `option_composed_ascii_is_text`, `option_symbols_and_dead_keys_are_shortcuts` | works (synthetic key_char only) |
| ⌘Q, twice in 3 s | guarded quit | src:lib.rs:345-360 | no context, K:161; app.rs:204 | T:TX `quitting_with_an_open_transaction_asks_and_a_second_cmd_q_quits`, `quit_cancel_stays_roll_back_and_quit_quits`, `quitting_without_transactions_does_not_ask` | works |
| ⌘⇧↵ / ⌘⇧⌫ | commit / rollback if a tx is open; ignored over dialogs | GuardDialogs.tsx:184-205 | Workspace, K:181-182 → tx_controls.rs:100 | T:TX `shortcuts_apply_only_with_a_transaction_open`, `live_manual_mode_commit_and_rollback`, `live_aborted_transaction_and_leaving_manual_mode` | partial (no overlay gate) |
| ⌘⇧↵ / ⌘⇧⌫ in a guard dialog | source: nothing | GuardDialogs.tsx:193 | GuardDialog, K:188-189; guard_dialogs.rs:111 | T:TX `a_new_prompt_answers_the_earlier_one_no`, `disconnecting_with_an_open_transaction_asks` | differs |
| Enter / Esc in a guard dialog | Cancel (focused by default) | GuardDialogs.tsx:82,132 | GuardDialog, K:190 (Dismiss) | T:TX `destructive_statements_ask_first_unless_turned_off_production_always` | works |

#### Editor (Workspace.tsx onKeyDown)

| Keys | Action | Source | GPUI | Verified by | Status |
|---|---|---|---|---|---|
| ⌘↵ / Ctrl+↵ | arm, then run; a selection runs at once | Workspace.tsx:451 | `SqlEditor > Input`, K:206-207 | T:ED `cmd_enter_arms_then_runs_the_statement_under_the_caret`, `a_selection_runs_at_once`, `nothing_arms_on_a_blank_line_between_statements`; T:WS `arm_and_run` | works |
| Ctrl+Space | explicit completion | :408 | K:208 | T:ED `ctrl_space_offers_foreign_key_joins_and_stays_open_while_erasing` | works |
| Tab (no popup) | indent | :454 | K:209 → editor/mod.rs:666 | T:ED `tab_indents_without_a_menu_and_undo_redo_work` | works |
| ↑ ↓ PgUp PgDn Esc (completion) | move / close | :437-441 | `SqlCompletion > Input`, K:213-218 | T:ED `arrows_and_pages_move_in_the_list_and_escape_closes` | works |
| Enter / Tab (completion) | insert / replace the word | :445-448 | K:217, K:209 | T:ED `enter_inserts_before_the_rest_of_the_word`, `typing_opens_completion_and_tab_replaces_the_whole_word` | works |
| ↑ ↓ Enter Esc (chooser); any other key disarms | pick / run / close | :415-429 | `StatementChooser > Input`, K:222-225 (+K:302-307) | T:ED `the_statement_chooser_offers_the_subquery`, `enter_runs_the_armed_statement_and_escape_or_a_caret_move_disarms` | works |

#### Result grid (DataGrid.tsx)

| Keys | Action | Source | GPUI | Verified by | Status |
|---|---|---|---|---|---|
| arrows / ⇧+arrows | move / extend | DataGrid.tsx:334-339 | ResultGrid, K:232-239 | T:GR `keyboard_selection_cmd_a_and_esc` | works |
| PgUp / PgDn | not in source | — | K:240-241 | T:GR `paging_down_scrolls_100k_rows` (PgDn only) | differs (extra) |
| ⌘A / ⌘C / Esc | select all rows / copy TSV / clear | :308 / :303 / :314 | K:242-244 | T:GR `keyboard_selection_cmd_a_and_esc`, `click_shift_click_and_copy_tsv` | works |
| click, ⇧-click, drag; row-number ⇧/⌘-click | cell / range / row selection | :421, :439-442 | mouse handlers | T:GR `click_shift_click_and_copy_tsv`, `drag_selects_a_range`, `row_numbers_pick_rows` | works |
| header click / ⇧-click | sort / multi-sort | :503 | mouse handlers | T:GR `header_clicks_sort_a_console_result_locally`, `table_tab_header_clicks_ask_for_a_server_sort`; T:TE `live_header_clicks_sort_on_the_server_shift_adds_columns` | works |
| Enter / F2 | edit; long, multi-line or JSON values open the value editor | :268, :213-222 | K:245-246 | T:GR `enter_edits_and_stages_the_change`, `value_editor_saves_with_cmd_enter` | works |
| Enter on JSON (read-only grid) | JSON viewer | :324 | K:245 → interact.rs:330-339 | T:GR `json_viewer_opens_on_enter_and_double_click_and_closes` | works |
| typing a character | start editing with it | :269 | on_key_down, render.rs:887 / interact.rs:441 | T:GR `double_click_typing_and_tab` | works |
| ⇧↵ | value editor | :267 | K:247 | T:GR `value_editor_saves_with_cmd_enter` | works |
| ⌘⌥N / ⌘⌥Z | set NULL / revert | :260-261 | K:248-249 | T:GR `each_editing_shortcut_stages_its_change` | works |
| ⌘⌫ / Delete | delete row | :263 | K:250-251 | same | works |
| ⌘N / ⌥Insert | add row (⌘N also without a selection) | :262, :297 | K:252-253 | same; T:TE `live_add_row_with_defaults_delete_and_set_null` | works |
| ⌘↵ | preview and submit | :266 | K:254 | T:GR `each_editing_…`; T:TE `revert_all_and_the_preview_follow_the_staged_edits`, `live_edit_preview_submit_changes_the_row` | works |
| Enter / Esc / Tab / ⇧Tab in a cell | commit / cancel / commit and move | :634-642 | PressEnter; `GridCellEditor > Input`, K:256-258 | T:GR `enter_edits_and_stages_the_change`, `double_click_typing_and_tab` | works |
| ⇧↵ / ⌘⌥N in a cell | value editor / NULL | :625-633 | K:260; K:248 via the grid ancestor (interact.rs:365-370) | code-only | works |
| edits blocked in read-only grids | — | :257 | interact.rs:447 | T:GR `read_only_content_blocks_every_edit` | works |

#### Overlays, panels and dialogs

| Keys | Action | Source | GPUI | Verified by | Status |
|---|---|---|---|---|---|
| ⌘↵ / Esc (value editor) | save / cancel | ValueEditor.tsx:100; Radix Dialog | K:265-267 | T:GR `value_editor_saves_with_cmd_enter` | works |
| Esc (JSON viewer) | close | JsonViewer.tsx (Radix Dialog) | K:268 | T:GR `json_viewer_opens_on_enter_…` | works |
| Enter / Esc (filter bar) | apply / restore the applied text | TableFilterBar.tsx:42,45 | `FilterBar > Input`, K:269-270 | T:TE `clauses_that_escape_their_place_never_run`, `live_filter_bar_applies_where_and_shows_server_errors_in_place` | works |
| ↑ ↓ PgUp PgDn (history) | move | QueryHistory.tsx:242-251 | K:286-292 | T:QH `arrows_and_pages_move_the_selection` | works |
| Enter / ⌘↵ (history) | insert / open in a console | :254-257 | K:293, K:295 | T:QH `enter_inserts_at_the_consoles_caret`, `cmd_enter_opens_a_new_console_and_cmd_c_copies` | works |
| ⌘C / ⌘⌫ / Esc (history) | copy / delete / close | :258-263 | K:294-297 | T:QH `cmd_enter_opens_…`, `cmd_backspace_deletes_the_selected_entry`, `cmd_alt_e_toggles_it_…` | works |
| ↑ ↓ Tab ⇧Tab Enter Esc (palette) | move / open / close | CommandPalette.tsx:165-171 | K:275-285 | T:CP `lists_tables_fuzzy_matches_and_moves_with_arrows_and_tab`, `enter_opens_the_table_pinned_and_it_becomes_recent` | works |
| ⌘F / Esc (Settings) | focus search / close (stays if the field has text) | SettingsView.tsx:130-141 | K:324-327 | T:A `cmd_comma_toggles_settings_…`; T:ST `search_filters_the_rows_and_shows_general` | works (open-dropdown case untested) |
| Enter / Esc (scope picker) | apply / cancel | ScopePicker.tsx:87,92 | K:308-311 | T:SB `scope_picker_enter_applies_and_escape_cancels` | works |
| ↑ ↓ → ← Enter (explorer) | tree navigation | Sidebar.tsx:740-761 | Sidebar, K:313-317 | T:SB `arrow_keys_move_expand_and_collapse_as_the_source` | works (chevron focus fix in flight, p9/fixes1) |
| Esc (right-panel search) | clear it | RightPanel.tsx:236 | K:318 → query_history.rs:1400 | T:QH `the_right_panels_history_tab_searches_and_escape_clears` | works |
| Enter (About) | copy version info | AboutWindow.tsx:53 | About, K:329 | T:A `enter_in_about_copies_the_version_info` | works |
| Enter / Esc (connection dialog) | submit / close | ConnectionDialog.tsx:212 | connection_dialog.rs:262; gpui-component | Esc: T:CD `cmd_o_opens_…`; Enter: code-only | works |
| Enter / Esc (password prompt) | connect / cancel | ConnectionDialog.tsx:588 | password_prompt.rs:69 | Esc: T:CD `the_password_prompt_follows_the_model`; Enter: code-only (test calls `submit()`) | works |
| toasts | Sonner ⌥T hotkey turned off | ui/sonner.tsx:29 | nothing bound | — | works |
| ⌘Z ⌘⇧Z ⌘X ⌘C ⌘V ⌘A | editing, Edit menu | src:updater.rs:200 (Menu::default) | gpui-component Input; app.rs:313-320 | T:A `edit_menu_actions_reach_the_focused_editor`; T:ED `tab_indents_…` | works |

#### Tauri app menu (src:updater.rs:198-209, `Menu::default`)

| Keys | Action | GPUI | Verified by | Status |
|---|---|---|---|---|
| ⌘H / ⌥⌘H / Show All | hide | K:163-164; app.rs:159-161 | T:A `app_menu_matches_the_tauri_menu` (menu structure only) | works, code-only |
| ⌘M / Zoom | Window menu | K:162; app.rs:162-163 | same | works, code-only |
| ⌃⌘F | full screen | K:165; app.rs:164 | none | works, code-only |
| ⌘W (Close Window) | closes the key window | `CloseWindow` has no key (actions.rs:41) | none | differs |
| About Boa / Check for Updates… | no keys | app.rs:299-300 | T:A `check_for_updates_reports_in_about` | partial (install is a stub, p8 in flight) |
| Settings… (GPUI only) | — | app.rs:302 | `app_menu_matches_…` | differs (extra, harmless) |

#### Gaps

No P0 issues found.

- **P1 – overlays don't block the global shortcuts.** The source skips ⌘T, ⌘W, ⌥N, ⌥1–9, ⌥↑↓ and ⌥B/E/J whenever any `[role=dialog]` is open (App.tsx:114). It also skips ⌘O (App.tsx:104) and ⌘⇧↵ / ⌘⇧⌫ (GuardDialogs.tsx:193). In GPUI, `pass_through` (root.rs:334) only checks Settings and the palette. The ⌘O gate (root.rs:382) only checks the palette and gpui-component dialogs. Query history (root.rs:596) and the grid's value editor and JSON viewer overlays (grid/render.rs:910-912) are all inside the `Workspace` context. From reading the code (not run), this means:
  - ⌘W closes the tab under an open value editor and loses its unapplied text.
  - ⌘⇧↵ commits while history is open.
  - ⌘O opens the connection dialog over history.
  - ⌥B typed in the history search toggles the sidebar instead of typing "∫".
  - No test covers any of this.
- **P2 – shortcuts in guard dialogs go beyond the source** (K:188-189, guard_dialogs.rs:111). In the source, nothing answers a guard dialog from the keyboard. In GPUI, ⌘⇧⌫ answers "Roll back and quit / disconnect" and ⌘⇧↵ answers Commit. Either record this as intended or remove it.
- **P2 – ⌘K/⌘P, ⌘⌥E and ⌘, don't work while a gpui-component dialog has focus.** These are the connection dialog, password prompt, guard dialogs and edit preview. Their layer is drawn by `gpui_component::Root` (app.rs:368), outside the `Workspace` context (root.rs:563). In the source these keys fire before the overlay check (App.tsx:92-112). This is inferred from the code, not tested.
- **P2 – ⌘W.** In Tauri, the default menu's Close Window carries ⌘W. In GPUI, ⌘W can't close the About window. In the main window with no tab open, Tauri probably closes the window, because App.tsx:125 returns without `preventDefault`. Needs a manual check.
- **P2 – extra keys not in the source:** grid PgUp/PgDn (K:240-241, PgUp untested) and statement chooser PgUp/PgDn (K:304-305).
- **P2 – About Enter repeats.** The source ignores held keys (`!e.repeat`, AboutWindow.tsx:53). K:329 most likely fires on key repeat too.
- **P2 – Check for Updates installs nothing yet:** `not_yet("Installing updates")` at models/updates.rs:228 (p8/release-prep in flight).
- **P2 – untested (code-only):**
  - ⌘M, ⌘H, ⌥⌘H, ⌃⌘F, Show All, Zoom (app.rs:159-165).
  - ⇧↵ and ⌘⌥N inside the cell input (K:260, interact.rs:343-377).
  - Enter to submit in the connection dialog (connection_dialog.rs:262) and the password prompt (password_prompt.rs:69).
  - ⌘O doing nothing over the palette or a dialog (root.rs:380-384).
  - Settings Esc with a dropdown open (SettingsView.tsx:136).
  - The ⌥ rule on a real German layout (tests only use a synthetic `key_char`): needs a manual check.
- **P2 – HANDOFF §10 is missing some source keys:** explorer ↑↓→←Enter, guard dialog Enter/Esc, connection dialog and password prompt Enter/Esc, Esc in the value editor, JSON viewer and palette, ⌘C in history, Esc in the scope picker, Ctrl+↵, the Tab/Enter difference in completion, ⌘N with no selection, and the menu accelerators. All of them are bound in GPUI; §10 should list them.

---

## 3. HANDOFF-PARITY §11 checklist: verifications present / missing

### §11 parity checklist audit

Path prefixes: `a:` = `crates/boa-app/src/`, `c:` = `crates/boa-core/src/`, `src:` = the Tauri source repo. Live tests skip without PGHOST. CI sets PGHOST on the macos-15 arm job (`.github/workflows/ci.yml:55-95`), so the live tests do run there.

| # | Item (short) | Needs | T present | S present | M | Missing to tick | Status |
|---|---|---|---|---|---|---|---|
| 1 | Data sources: add / edit / delete, URL, SSL, SSH, test | T,S,M | a:views/connection_dialog/tests.rs `pasting_a_url_fills_the_fields`, `saving_writes_connections_json_with_the_password`, `editing_keeps_the_stored_password`, `delete_asks_with_a_second_click`, `validation_disables_test_and_save`; a:models/connections/tests.rs `saving_adds_or_updates_and_an_open_connection_reconnects`, `deleting_forgets_the_connection`; c:state/conn_url.rs (9); c:ssh.rs (6); c:connections.rs `ssh_tunnel_settings` | phase2 dialog-general / -url-pasted / -ssh / -edit.png | phase2 M#1 (SSH), not run | The "Test connection" button has no test (`a:views/connection_dialog.rs:995`). No test clicks the SSL-mode choice. SSH end to end is manual only. | partial |
| 2 | Password prompt, save toggle | T | connections/tests.rs `no_stored_password_opens_the_prompt_and_its_password_connects`, `a_refused_password_reopens_the_prompt_with_the_error`, `saving_the_password_from_the_prompt_stores_it`, live `a_refused_login_opens_the_prompt`; connection_dialog/tests.rs `the_prompt_connects_with_the_typed_password`, `not_saving_the_password_leaves_it_out` | password-prompt.png | phase2 M#4 (real 28P01) | — | ready to tick |
| 3 | Env tag + colour | S | sidebar/tests.rs `the_production_tag_and_badges_render`; conn_tag.rs `dots_and_colours_follow_the_state_and_tag`; c:grid/conn_tag.rs (3) | phase3 production-tag.png; phase4 integration/production-connection.png, multiple-tabs.png | — | — | ready to tick |
| 4 | Read-only (server-enforced, restored) | T live | connections/tests.rs `a_read_only_connection_is_enforced_by_the_server`; transactions/tests.rs `live_read_only_connections_refuse_writes`; c:pg.rs `live_postgres` (restore at pg.rs:1756); c:sql/guard.rs `read_only_escape_*` | — | — | — | ready to tick |
| 5 | Explorer tree, scope, counts, views, expanded persisted | T,S | a:views/sidebar/tree_tests.rs (11); sidebar/tests.rs `arrow_keys_move_expand_and_collapse_as_the_source` (checks `treeExpanded` in prefs.json), `catalogs_load_the_default_pick_on_connect`, `scope_picker_enter_applies_and_escape_cancels`; scope_picker.rs (4); c:grid/table_meta.rs (6) | explorer-tree, scope-picker, context-menu, production-tag.png | — | Context menu and picker were only captured with render_to_image. The chevron-focus fix is in flight. | ready to tick (P2 open) |
| 6 | DDL view → new console | T | c:pg.rs `live_postgres` (`table_ddl`, pg.rs:1564-1614); c:pg/catalog.rs `table_ddl`, `view_and_matview_ddl`; c:state/ddl.rs `opens_the_ddl_in_a_new_console_on_that_database`, `a_failure_shows_an_error_toast_naming_the_table` | none | — | No boa-app test of explorer "Show DDL" → new console. That path was only checked live, in phase4/integration.md. Toolbar DDL is in flight. | partial |
| 7 | Console: highlight, completion, ⌘↵, chooser | T,S | a:editor/tests.rs (18, e.g. `cmd_enter_arms_then_runs_the_statement_under_the_caret`, `the_statement_chooser_offers_the_subquery`, `ctrl_space_offers_foreign_key_joins_and_stays_open_while_erasing`, `typing_opens_completion_and_tab_replaces_the_whole_word`); editor/highlight.rs (4), completion.rs (7); c:sql/complete.rs (23) | console-results, statement-chooser, completion-popup.png | — | Completion icons are covered by a screenshot only. | ready to tick |
| 8 | Paged results, cancel, progress, Output (⌥J), result tabs | T,S | workspace/tests.rs `live_run_shows_the_result_in_the_database_output`, `live_cancel_stops_a_slow_statement`, `live_paging_fetches_the_next_page`, `live_errors_show_their_position_in_the_output_log`; a:tests.rs `option_b_e_j_toggle_the_panels_from_the_editor`; workspaces/tests.rs `the_output_log_keeps_two_hundred_lines_newest_first` | console-results, run-progress, console-output-log.png | — | The run-progress UI ("Executing… n s", the bar) is covered by a screenshot only. | ready to tick |
| 9 | Grid selection, ⌘C, Copy as | T | a:grid/tests.rs `click_shift_click_and_copy_tsv`, `drag_selects_a_range`, `keyboard_selection_cmd_a_and_esc`, `row_numbers_pick_rows`, `every_copy_as_format_via_its_menu_action`; grid/selection.rs (9); c:grid/copy_as.rs (18) | grid-context-menu.png | — | — | ready to tick |
| 10 | JSON viewer, value editor | T,S | grid/tests.rs `json_viewer_opens_on_enter_and_double_click_and_closes`, `value_editor_saves_with_cmd_enter`; views/json_viewer.rs (3) | json-viewer, value-editor.png | — | — | ready to tick |
| 11 | Table editing, read-only reasons, preview / submit, prompts | T live,S | a:views/table_edits/tests.rs `live_edit_preview_submit_changes_the_row`, `live_add_row_with_defaults_delete_and_set_null`, `live_failed_submit_keeps_the_edits_and_points_at_the_statement`, `pending_edits_ask_before_paging_and_closing`, `read_only_reasons_follow_the_catalog`; c:grid/grid_edits.rs (21), grid/edits.rs (8) | phase5 staged-edits, sql-preview, submit-error.png | — | Fixes for the `<default>` placeholders and the narrow header are in flight (p9/fixes1). | ready after p9/fixes1 |
| 12 | WHERE / ORDER BY, header sort, validation | T live | table_edits/tests.rs `live_filter_bar_applies_where_and_shows_server_errors_in_place`, `live_header_clicks_sort_on_the_server_shift_adds_columns`, `clauses_that_escape_their_place_never_run`; c:grid/table_query.rs (16) | filter-bar, where-error.png | — | The toolbar filter fix is in flight. | ready after p9/fixes1 |
| 13 | Tx Auto / Manual, probe, shortcuts, typed | T live | transactions/tests.rs `live_manual_mode_commit_and_rollback`, `live_aborted_transaction_and_leaving_manual_mode`, `live_typed_begin_commit_and_rollback`, `shortcuts_apply_only_with_a_transaction_open`; c:state/tx.rs (6) | tx-open, tx-menu, end-tx.png | phase6 M#7 | — | ready to tick |
| 14 | Destructive confirmations | T | transactions/tests.rs `destructive_statements_ask_first_unless_turned_off_production_always`, `live_destructive_confirmation`; c:sql/guard.rs (13); c:state/guards.rs (7) | destructive-confirm.png | — | — | ready to tick |
| 15 | Query history | T | a:models/history/tests.rs (9: `writes_the_tauri_apps_jsonl_format`, `retention_drops_old_runs_and_follows_the_setting`, `keeps_at_most_five_thousand_per_connection`, `a_query_run_again_replaces_its_entry`); query_history/tests.rs (9: `arrows_and_pages_move_the_selection` covers search and errors-only, `all_connections_merge_newest_first`); c:state/history.rs (31) | query-history, right-panel-history.png | — | — | ready to tick |
| 16 | CSV export | T live | exports/tests.rs `exports_the_whole_orders_table`, `cancel_leaves_no_file`, `inside_a_transaction_a_savepoint_keeps_it_intact`, `options_are_read_from_and_saved_to_csv_export_json`; csv_export/tests.rs (2); c:pg/export.rs (3) | csv-export-menu / -progress / -done.png | — | The native save panel and Reveal in Finder were never clicked. In flight: export vs console on one session, `.part` on quit, the menu-disable rule. | partial |
| 17 | Session restore + reconnect | T | session/tests.rs (13: `the_session_is_saved_and_restored_through_the_app`, `saves_are_debounced_and_skip_unchanged_sessions`, `reconnects_retrying_while_the_server_is_unreachable`, `the_file_is_what_the_tauri_app_writes`); c:state/session.rs `keeps_a_table_tabs_filter_and_sort` | session-restored.png | — | Retries run about 2 min (`c:state/reconnect.rs:34`), not the "~30 s" in the checklist. The reconnect code follows the source; the checklist wording needs fixing. | ready to tick (wording) |
| 18 | Staying connected | T,M | connections/tests.rs (34: live `a_terminated_session_reconnects`, `tx_lost_and_commit_unknown_messages`, `a_wake_up_retries_a_failed_wanted_connection_right_away`, `a_failed_connect_shows_one_status_toast_per_connection`); c:state/reconnect.rs (20); c:pg.rs `only_rerunnable_statements_are_retried`, `live_dropped_session` | lost-connection-toast.png | phase2 M#2-3, not run | No test stops and starts Postgres. Keepalive detection is untested (`c:pg.rs:107`). | partial |
| 19 | Quit / disconnect guard, double ⌘Q | T,M | transactions/tests.rs `quitting_with_an_open_transaction_asks_and_a_second_cmd_q_quits`, `quit_cancel_stays_roll_back_and_quit_quits`, `live_disconnect_with_an_open_transaction_asks_and_rolls_back`; c:state/guards.rs `a_second_quit_within_3_s_goes_through` | quit-guard.png | phase6 M#1-6, not run | The manual checks have not been run. Dock quit and logout are not held. | partial |
| 20 | Palette, Settings, About | T,S | a:views/command_palette.rs (11: `cmd_k_and_cmd_p_toggle_the_palette_and_esc_closes_it`, `recents_come_first_newest_first_up_to_recent_count`); views/settings.rs (5: `every_switch_and_choice_writes_prefs_json`, `reset_to_defaults_keeps_the_layout_in_prefs_json`); a:tests.rs `enter_in_about_copies_the_version_info` | palette, palette-search, settings-*, about.png | — | — | ready to tick |
| 21 | Updates: download, restart, bridge | M | Check only: a:models/updates.rs (7), c:state/updates.rs (12) | about, status-bar-update.png | none | Install is a stub (`not_yet`, `a:models/updates.rs:228`). The real feed is off by default. The bridge release has not been rehearsed. | not started |
| 22 | Every §10 shortcut | T,M | a:keymap.rs `every_global_shortcut_of_app_tsx_is_bound_in_workspace`, `option_*`; a:tests.rs `edit_menu_actions_reach_the_focused_editor`; workspace/tests.rs `cmd_t_cmd_w_and_option_digits_drive_the_tabs`, `option_arrows_cycle_databases`; grid/tests.rs `each_editing_shortcut_stages_its_change` | — | no combined list | Every key is bound (`a:keymap.rs:161-329`). Missing: a §10 → test table, and a manual list for the rest (native Edit menu, real ⌘Q, ⌥ on a German layout). | partial |
| 23 | Prefs / session / CSV JSON, localStorage import | T | c:state/prefs.rs (31); a:tests.rs `layout_is_saved_to_the_config_dir_and_read_back`; session/tests.rs `the_local_storage_import_runs_once`; c:state/csv_export.rs `options_json_is_what_the_tauri_app_wrote`; c:state/local_storage.rs (9) | — | — | — (the user approved the import, HANDOFF §13) | ready to tick |
| 24 | Look matches boa.png / Tauri | S,M | theme.rs `dark_palette_matches_main_css` | per-phase shots only | none | No side-by-side capture and no user sign-off. Visual deviations are still open. | partial |
| 25 | Migration from a real config dir | T(copy),M | session/tests.rs `real_migration_copy` (#[ignore], run once, counts only); c:state/local_storage.rs `real_local_storage_keys` (#[ignore]) | — | none | The Tauri app was never launched on the migrated files; the files were only parsed with the Rust port of the Tauri rules. Manual check not run. | partial |
| 26 | Release: signed, notarized, archs | M | — | — | none | p8/release-prep | not started |
| 27 | Performance | bench | c:sql/tokenize.rs `tokenizes_a_2000_line_buffer_fast`; grid/tests.rs `paging_down_scrolls_100k_rows` | docs/phase0/numbers.md (skeleton only); phase4/README-grid.md (5k rows at 8.33 ms per frame) | — | Startup and idle memory were not re-measured on the full app. There is no typing-latency number for a 2,000-line console. | partial |

**Totals: 15 ready to tick, 2 ready after p9/fixes1, 8 partial, 2 not started.**

### Open "not verified" / gaps / deviations from the phase docs (re-checked against the code)

**P1**
- [ph6] Dock → Quit and logout are not held. An open transaction is rolled back without a prompt; the source asks first. GPUI has no `applicationShouldTerminate:` hook, so holding them needs `unsafe` and the user's OK (`a:app.rs:204-216`).
- [ph7b] An export shares the database session with consoles, which closes the console's cursor ("cursor __boa does not exist"). In flight: p9/core-conc.
- [ph7c] A scope change reloads the catalog while a table tab fetches, giving "current transaction is aborted". In flight: p9/core-conc.
- [ph7c] Update download, signature check, install and relaunch are a stub (`a:models/updates.rs:228`). In flight: p8.
- [ph2] Manual checks #1-4 (SSH, sleep/wake, network change, real 28P01) have not been run.
- [ph6] Manual checks #1-7 (real ⌘Q, menu quit, close button, reconnect / disconnect guards, ⌘⇧↵ / ⌘⇧⌫) have not been run.
- [ph4-int] Not verified live: native menu bar and Edit menu, drag-reordering connections and tabs, SSH, reconnect after a real network loss or sleep.

**P2**
- [ph4-int / ph3] Preview tabs are not italic: there is no italic Geist Mono in `crates/boa-app/assets/fonts`, so `a:views/title_bar.rs:313` has no effect. In flight.
- [ph4-int] Clicking a chevron in the explorer moves the focus to that row (`a:views/sidebar.rs:765-768`). In flight.
- [ph4-int] With no database selected, the status bar falls back to the first connection in prefs order, not `connections.json` order (`a:views/status_bar.rs:96`). In flight.
- [ph7b] `.part` file left behind on quit (in flight). The save panel has no file-type filter and opens in the last folder or ~/Downloads, where the Tauri dialog let macOS choose (`a:models/exports.rs:209`). The native save panel and Reveal in Finder were never clicked.
- [ph7d] Restore retries for about 2 min; USAGE.md and the checklist say ~30 s. `wanted`, `workspaces` and `seen` are saved sorted. Both are parse-compatible.
- [ph6] In guard dialogs Enter is Cancel and ⌘⇧↵ / ⌘⇧⌫ answer the dialog. A COMMIT / ROLLBACK error does not connect first.
- [ph4-grid] Disabled menu items show no tooltip with the reason. The JSON viewer does not wrap lines. The text sort only approximates `localeCompare`. The modals have no open or close animation. The 100k-row bench was not re-run.
- [ph4] Comments are not italic (the Tauri app ships no italic font either).
- [ph7a] History rows are not text-selectable. PgUp / PgDn page size is computed, not measured.
- [ph7c] Opening a table from the palette without a server moves the focus outside the dispatch tree (only seen in tests).
- [ph0 / ph1 / ph4-int] Not verified: trackpad scrolling, mouse selection in the editor, a real IME, popup placement at window edges, traffic lights side by side with Tauri, window drag and double-click zoom, the ⌥ rule on a real German layout. §13 says Accessibility is now granted, so these can be checked.
- [ph3] The context menu and scope picker were never captured from a real window (render_to_image only).
- [ph0] Memory with huge pages (168 MB for 100k × 7) has not been re-measured.
- [ph4-int] GPUI draws no frames for a fully hidden window, and latency is measured once per connect (as the source does). Neither needs action.

**Since fixed (checked in code):**
- Phase 1: the "+" and "…" title-bar menus (`a:views/title_bar.rs:369,412`) and tab drag / pin (`a:views/title_bar.rs:252-296`).
- Phase 1: the localStorage import (ph7d), and the Settings choice menus, now tested by `every_switch_and_choice_writes_prefs_json`.
- Phase 3: the stubs are wired, and Show DDL works end to end (checked live in integration).
- Phase 4 integration: table tabs are editable (ph5), and the double pager is gone (`a:views/workspace.rs:303`).
- Phase 4 integration: the Tx / palette / history / CSV / session `not_yet` stubs are gone. The only `not_yet` left is the update install.
- Phase 4 integration: CI now runs clippy with `--all-features` (`ci.yml:93`).

---

## 4. Visual comparison (real window vs. the source UI at the same size and scale)

The GPUI captures are real `screencapture -l` windows. The source captures are the
React UI in browser-preview mode at 1280×820 @2×. The reference image is
`dbclient/docs/images/boa.png`.

| What | GPUI | Source |
|---|---|---|
| Main layout: explorer expanded, console with results, Production tag | `docs/audit/main-console.png` (focus in the explorer), `docs/audit/main-console-armed.png` (editor focused, statement armed) | `docs/audit/src/main-console.png`, `boa.png` |
| Table tab with staged edits (update, NULL, delete, new row) | `docs/audit/table-edits.png` | `docs/audit/src/table-edits.png`, `docs/audit/src/table-tab.png` |
| Settings (General) | `docs/audit/settings.png` | `docs/audit/src/settings.png` |
| Command palette | `docs/audit/palette.png` (query "cust") | `docs/audit/src/palette.png` (query "us"), `docs/audit/src/palette-empty.png` |
| New data source dialog | `docs/audit/connection-dialog.png` | `docs/audit/src/connection-dialog.png` |

**These match** (same position, size and colour within a pixel or two at 2×):

- The overall layout and panel sizes.
- Sidebar header and toolbar. Tree row height (26 px), indents, icons, PROD badge, "N of M" badges, row counts.
- Database chip, tabs and the red Production stripe.
- Editor line height (20 px), syntax colours and the current-line band.
- Grid: header with type names, row numbers, number / timestamp / NULL colours, the "N rows" pill.
- Database Output header (Output tab, result tab, `console_1 [Production] · n ms`, export and hide buttons).
- Status bar: breadcrumbs, caret, UTF-8, connection with state dot, PROD and latency.
- Palette: layout, rows, swatch, footer.
- Settings: nav, cards, switches.
- Dialog: fields, swatches, checkboxes.
- Staged-edit tints: the edited-cell blue, the deleted row's red tint with strikethrough, the new row's `+` marker.
- The caret and the statement outline both clear when the editor loses focus, as in the source (`Workspace.tsx:390-394`).

**Concrete differences (all P2 unless noted):**

1. **Editor gutter**
   - The source gutter is ~56 px wide with a 1 px divider line between the numbers and the code.
   - GPUI's gutter is ~29 px narrower and has no divider, so the code starts 29 px further left.
   - Compare `main-console-armed.png` with `src/main-console.png`.
2. **Statement outline / armed fill**
   - The source traces each line's own extent, a stepped outline that hugs the text (`Workspace.tsx:228-234`, `rangePath`).
   - GPUI draws one bounding rectangle around the whole statement, as wide as its longest line.
3. **Console toolbar "Tx: Auto"**
   - The source shows the label, then the chevron, in muted text (`text-quiet`).
   - GPUI puts the chevron *before* the label, and the text is brighter and heavier (`views/tx_controls.rs`).
4. **Default-schema picker (toolbar, right)**
   - The source shows `app_production.public ⌄` in Geist Mono, muted, with the chevron after.
   - GPUI shows `⌄ boa_dev.public`: chevron first, UI font, brighter (`views/workspace.rs:525-550`).
5. **Title-bar "…" button:** GPUI's ellipsis glyph is about half the size of the source's Lucide `Ellipsis` (16 px).
6. **Explorer toolbar "+":** GPUI's plus icon is smaller and thinner than the source's; the other toolbar icons match.
7. **Table toolbar**
   - The source has regular-size "Preview…" and "Submit" buttons (text-sm, h-7, Submit a white primary button).
   - GPUI's are text-xs, with a small white "Submit" pill.
   - The export and open-in-console icons are ~14 px in GPUI vs 16 px in the source.
   - The "Filter loaded rows" box and "DDL" are missing *(p9/fixes1, P1-5)*.
8. **WHERE / ORDER BY bar:** the source draws the `WHERE` / `ORDER BY` labels at the same size as the placeholder (mono text-xs). GPUI's labels are smaller (≈10 px) than the placeholder text.
9. **New-row placeholders:** `<default>` on every column *(p9/fixes1, P1-6)*. Not italic: no italic Geist Mono is bundled, while the source renders italic.
10. **Settings**
    - The page header block is tighter in GPUI. The "General" title sits ~6 px higher, the first section label ~11 px higher and the first card ~13 px higher. Row spacing inside the cards matches.
    - Select triggers ("5", "500", "30 days"): the source uses a light, small-chevron trigger with a faint border. GPUI draws a full bordered box with a larger chevron and a bolder value.
11. **New data source dialog**
    - The source inputs are 36 px tall (`h-9`); GPUI's are ~26 px.
    - The label-to-field gaps are smaller in GPUI, so the dialog is ~93 px shorter (539 px vs 632 px).
    - The footer buttons are smaller too (Test connection / Cancel / Save and connect).
    - The close × is smaller.
12. **Palette:** no differences beyond the data. The search-field circle icon and the green connection swatch are the same in the source.
13. **Traffic lights:** positioned at GPUI (14, 16) and look right in the real window, but couldn't be compared with Tauri (the browser preview has none). Still an M item.

---

## 5. Menus and context menus

### Menus & context menus parity audit

The source has no tab context menu (TitleBar.tsx tabs only close on middle-click). TitleBar.tsx has no other dropdowns, and BottomPanel only hosts the Export menu. The GPUI app has no tab context menu either, which matches.

#### Explorer "+" dropdown (src:Sidebar.tsx:180-197 → views/sidebar.rs:908-925)
| Source item | Condition | GPUI item | Status | Evidence/pointer |
|---|---|---|---|---|
| Query console ⌥N | disabled if nothing connected | same, `.action(NewQueryConsole)` shows ⌥N | works | code-only; sidebar.rs:915 |
| Data source ⌘O | always | same → `connection_dialog::open(None)` | works | code-only (T:cmd_o_opens_the_dialog_in_the_main_window covers ⌘O only) |

#### Explorer context menus (src:Sidebar.tsx:434-685 → sidebar.rs:544-718)
| Source item | Condition | GPUI item | Status | Evidence/pointer |
|---|---|---|---|---|
| **Connection:** Disconnect | state=connected | same | works | code-only; sidebar.rs:555 |
| Connect | not connected; disabled while connecting | same | works | code-only; sidebar.rs:564 |
| Copy connection URL | always | `tree::connection_string(conn, None)`; toast "Copied …" 2.4 s | works | T:connection_url_and_environment (tree_tests.rs:557) |
| — separator, Edit… | always | same → `connection_dialog::open(Some(id))` | works | code-only |
| *(none)* | — | **extra** New query console (disabled if no dbs) | differs | sidebar.rs:582 |
| *(none)* | — | **extra** Refresh (disabled if not connected) | differs | sidebar.rs:597; `refresh_connection` sidebar.rs:1163 |
| *(none)* | — | **extra** Delete… (alert confirm) | differs | T:delete_asks_with_a_second_click (connection_dialog/tests.rs:401) |
| **Database:** New query console | always | same | works | code-only; sidebar.rs:624 |
| Refresh (`loadDatabase force`) | always | `catalogs.refresh(key)` | works | T:refresh_rereads_and_a_plain_load_keeps_the_cache (model only) |
| **Schema / tables·views folder / meta folder / status rows:** no menu | — | no menu (`has_menu` sidebar.rs:828) | works | code-only |
| **Table/view:** Open in new tab (pinned) | always | `open_table(..., preview=false)` | works | S:docs/phase3/context-menu.png |
| New console: select * | always | `select * from {qualified};` | works | S:phase3/context-menu.png |
| Show DDL | always | `show_ddl` → `ddl_action` → new console / toast "No DDL for …" | works | T:opens_the_ddl_in_a_new_console_on_that_database (boa-core state/ddl.rs:72) |
| Export to CSV… | always | `export_table` | works | T:exports_the_whole_orders_table (models/exports/tests.rs, live PG) |
| — separator, Copy qualified name | always | same (`schema.table`, unquoted) | works | S:phase3/context-menu.png |
| *(none)* | — | **extra** Copy name, separator, Refresh | differs | sidebar.rs:691-699 |
| **Column:** Copy name | always | same | works | code-only; sidebar.rs:701 |
| **Key/FK/index/check:** Copy name, Copy definition | always | same | works | code-only; sidebar.rs:703-710 |

#### Grid context menu (src:DataGrid.tsx:711-800 → grid/interact.rs:706-793, render.rs:553)
| Source item | Condition | GPUI item | Status | Evidence/pointer |
|---|---|---|---|---|
| Right-click selects cell unless already in range | — | `body_right_mouse_down` (interact.rs:149) | works | T:right_click_opens_the_menu_and_copy_runs_from_it |
| Copy ⌘C | disabled !shape | same, `!has_shape` | works | T:right_click_opens_the_menu_and_copy_runs_from_it; S:phase4/grid-context-menu.png |
| Copy as › (trigger) | **trigger disabled** !shape | submenu trigger always enabled; only its items are disabled | differs | interact.rs:767 |
| › TSV, TSV with header, CSV, Markdown table, JSON, SQL INSERT, SQL UPDATE, SQL IN list | disabled !shape or `unavailable()`; **tooltip = reason** | same labels and rule; **no reason tooltip** | partial | T:every_copy_as_format_via_its_menu_action; S:phase4/grid-context-menu.png |
| Edit ↵ | editable; disabled no cell or row deleted | `no_cell` | works | T:enter_edits_and_stages_the_change (key path); menu code-only |
| Open in editor ⇧↵ | same | same | works | T:each_editing_shortcut_stages_its_change (keys) |
| Set NULL ⌘⌥N | same | same | works | same |
| Add row ⌘N | editable | `actions::AddRow` | works | keys tested; menu code-only |
| Delete row ⌘⌫ | disabled no row or deleted | `no_cell` (same effect) | works | keys tested; the shown hint may be "⌦" (two bindings, keymap.rs:250-251), not verified |
| Revert cell | disabled unless cell edited | same | works | code-only |
| Revert row ⌘⌥Z | disabled unless row marked | same; **no ⌘⌥Z hint** (menu sends `RevertRow`, key is bound to `Revert`) | partial | interact.rs:789, keymap.rs:249 |
| Preview and submit… ⌘↵ | editable | `SubmitEdits` | works | code-only |
| Read-only content: no edit section | `editing` absent | `editing_is_none` | works | T:read_only_content_blocks_every_edit |

#### TitleBar dropdowns (src:TitleBar.tsx:129-171, 305-324 → views/title_bar.rs:363-460)
| Source item | Condition | GPUI item | Status | Evidence/pointer |
|---|---|---|---|---|
| "+" trigger | disabled !connected | same | works | code-only |
| New query ⌘T | — | same (static hint) | works | code-only |
| Open table… ⌘K | — | dispatches `TogglePalette` | works | code-only |
| "…" Copy connection URL (tab's db), toast 3.2 s | always | URL built from `active.saved` config; empty if no saved config | works | code-only; title_bar.rs:394 (fn tested: title_bar.rs:548) |
| Connection details | always | right tab Info + open panel | works | code-only |
| — separator, Disconnect (destructive) | disabled !connected | same, **not styled destructive** | partial | title_bar.rs:448 |

#### Export menu (src:CsvExport.tsx:30-106 → views/csv_export.rs:72-285)
In-flight: the menu-disable rule and the `.part` file left on quit (p9/fixes1).
| Source item | Condition | GPUI item | Status | Evidence/pointer |
|---|---|---|---|---|
| Trigger "Export" | disabled if `disabled` or no source | same | works | csv_export.rs:131 |
| While exporting: "Exporting…" + Cancel | job for owner | same | works | T:a_running_export_shows_progress_and_cancel |
| Export to CSV… | always | same | works | T:the_export_menu_sets_options_copies_and_exports; S:phase7/csv-export-menu.png |
| Copy page/rows as CSV + count | disabled without rows | `page.is_some()` | works | same test |
| Note "The statement changes data…" | !pageable | same | works | code-only |
| Delimiter: Comma/Tab/Semicolon (radio, menu stays open) | — | same | works | same test ("still open") |
| Header row (checkbox, menu stays open) | — | same | works | same test |

#### Tx mode menu (src:TxControls.tsx:83-96 → views/tx_controls.rs:177-195)
| Source item | Condition | GPUI item | Status | Evidence/pointer |
|---|---|---|---|---|
| Trigger "Tx: Auto/Manual" | disabled !connected | same (adds a "Transaction mode" tooltip) | works | S:phase6/tx-menu.png |
| Auto-commit ✓ | leaving Manual with tx open → `chooseEndTx` | `switch_mode` same flow | works | T:live_aborted_transaction_and_leaving_manual_mode (live PG) |
| Manual ✓ | — | same | works | T:live_manual_mode_commit_and_rollback (live PG) |

#### Console schema picker, status popover, form selects
| Source item | Condition | GPUI item | Status | Evidence/pointer |
|---|---|---|---|---|
| `{db}.{schema}` picker: db label, separator, schemas ✓ (completion only) | — | same (workspace.rs:525-550), `set_default_schema` | works | code-only |
| Status-bar popover: name·state, engine/version/latency, URL, Disconnect / Connect / Connecting… (disabled) / Reconnecting… Retry now | by state | same (status_bar.rs:227,264) | works | T:popover_button_follows_the_state |
| Connection dialog SSL (Disable/Prefer/Require/Verify full), Environment (None + 4), SSH auth (SSH agent/Key file) | — | same lists (connection_dialog.rs:47-66) | works | code-only |
| Settings choice selects | — | `PopupMenuItem.checked` (settings.rs:574) | works | T:every_switch_and_choice_writes_prefs_json, T:choices_match_the_core_lists |
| ScopePicker "N of M" popover | — | sidebar badge popover | works | T:scope_picker_enter_applies_and_escape_cancels; S:phase3/scope-picker.png |

#### Native app menu (src:updater.rs:199-223 + Tauri 2.12.4 `Menu::default` + muda 0.20 → app.rs:296-330, handlers app.rs:138-175)
| Source item | Condition | GPUI item | Status | Evidence/pointer |
|---|---|---|---|---|
| Boa › About Boa | — | same → `about::open` | works | T:app_menu_matches_the_tauri_menu |
| Check for Updates… (open About + check) | — | same; install is a `not_yet` stub (updates.rs:228, p8/release-prep in flight) | partial | T:check_for_updates_reports_in_about |
| *(none)* | — | **extra** Settings… (toggles: a second click closes Settings) | differs | app.rs:302,147 |
| Services · Hide Boa ⌘H · Hide Others ⌥⌘H | — | same | works | keymap.rs:163-164 |
| *(none; Tauri default has no Show All)* | — | **extra** Show All | differs | app.rs:308; the test asserts this extra item |
| Quit Boa ⌘Q (quit guard) | — | same + QuitGate | works | keymap.rs:161 |
| File › Close Window **⌘W** | — | Close Window, **no ⌘W** (⌘W → CloseTab, workspace only) | differs | keymap.rs:173; app.rs:312 |
| Edit › Undo ⌘Z, Redo ⇧⌘Z, Cut, Copy, Paste, Select All | — | `os_action` aliases of the input actions; the grid handles Copy/Select All | works | T:edit_menu_actions_reach_the_focused_editor; render.rs:858 |
| View › Toggle Full Screen ⌃⌘F (AppKit relabels Enter/Exit) | — | static "Enter Full Screen" ⌃⌘F | partial | app.rs:322 |
| Window › Minimize ⌘M, Zoom, separator, Close Window ⌘W | — | same, Close Window has no ⌘W | partial | app.rs:323-328 |
| Help (empty) | — | same | works | the test checks only the menu name |

#### Gaps
- **P1** ⌘W never closes a window. In the source, the Close Window accelerator (⌘W) closes the About window, and closes the main window when no tab is open (App.tsx:122 only prevents the default when a tab exists). In GPUI, `CloseWindow` has no binding and `CloseTab` just propagates, so ⌘W does nothing in About or with no tabs. keymap.rs:173, app.rs:165, root.rs:410.
- **P1** The explorer context-menu wiring has no tests. Nothing calls `Sidebar::build_menu` (sidebar.rs:544), so items, enabled rules and click targets are covered only by a screenshot (S:phase3/context-menu.png). The same goes for the title bar "+" and "…" dropdowns (title_bar.rs:363-460) and the grid menu's edit section (interact.rs:778-791): only the actions behind them are tested.
- **P2** The connection menu has extra items not in the source: New query console, Refresh, Delete… (sidebar.rs:582-619). The table menu adds Copy name and Refresh (sidebar.rs:691-699). These are additive, not parity. Decide whether to keep them and record that in HANDOFF-PARITY §11.
- **P2** The app menu adds Settings… and Show All (app.rs:302,308). Settings… calls `toggle_settings`, so choosing it while Settings is open closes it. `app_menu_matches_the_tauri_menu` asserts the extra items, so the test encodes this difference.
- **P2** The "Copy as" submenu trigger is never disabled (the source disables it when nothing is selected), and disabled formats show no reason tooltip (src:DataGrid.tsx:725 `title={why}`). interact.rs:767-776.
- **P2** "Revert row" shows no ⌘⌥Z hint: the menu sends `RevertRow` but the key is bound to `Revert` (interact.rs:789, keymap.rs:249). The hints for Delete row and Add row may show the second binding ("delete" or "alt-insert") instead of ⌘⌫ / ⌘N; not verified (keymap.rs:250-253).
- **P2** The View item's label is fixed at "Enter Full Screen"; the source's native toggleFullScreen item switches to "Exit Full Screen" (app.rs:322).
- **P2** "Close Window" in File and Window shows no ⌘W, unlike the source (app.rs:312,327).
- **P2** The title bar "Disconnect" is not styled destructive (src:TitleBar.tsx:163 `variant="destructive"`; title_bar.rs:448). Its "Copy connection URL" copies an empty string when `active.saved` is None (title_bar.rs:394-404).
- **P2** The tests for the Tx menu and the Export to CSV… item need a live PG, so they skip silently without a database (models/transactions/tests.rs:637,695; models/exports/tests.rs).
- Docs are stale: phase1/README.md:107 still says "+" and "…" open no menus yet, but they do now (title_bar.rs:363-460).
- In flight elsewhere, not re-audited here: the export menu disable rule, the export `.part` file on quit, preview-tab italics and explorer chevron focus (p9/fixes1); updater install (p8/release-prep).

---

## Appendix: reproducing the captures

GPUI:

```sh
dir=$(mktemp -d); mkdir $dir/cfg
# connections.json: the "Local dev" entry with "environment": "production"
BOA_CONFIG_DIR=$dir/cfg BOA_DEV_CONNECTION=1 BOA_DEV_PANELS=sidebar,bottom \
BOA_DEV_EXPAND='local-dev;local-dev/boa_dev;local-dev/boa_dev/public' \
BOA_DEV_INPUT=$dir/input.txt target/debug/boa-app   # built with --features screenshot
# then append commands to input.txt (click / key / type / wait) and
# screencapture -x -o -l <CGWindowID of the "Boa" window> out.png
```

Source: copy `crates/boa-desktop/ui/{src,index.html,tsconfig.json}` to a scratch dir,
symlink its `node_modules`, then `vite build` with `outDir` / `cacheDir` in the scratch
dir (nothing is written to the source repo). Serve the output, open it in headless
Chrome (`?overlay`), and drive it over the DevTools protocol:
`Input.dispatchKeyEvent` / `dispatchMouseEvent` with a 1280×820 viewport at
`deviceScaleFactor: 2`. ⌥ shortcuts need the composed `key` (e.g. `˜` for ⌥N),
because of `isOptionShortcut`. Typing goes through `Input.insertText` after
focusing the editor's textarea.
