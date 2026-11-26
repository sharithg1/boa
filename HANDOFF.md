# Handoff: port Boa from Tauri + React to GPUI

You are porting **Boa**, a macOS PostgreSQL client, from Tauri 2 (Rust backend +
React/WebView frontend) to a pure-Rust app built on **GPUI** (https://gpui.rs/),
the UI framework behind the Zed editor. This directory
(`/Users/sharith/Desktop/repos/boa-gpui/`) is empty and is where the new app lives.

The goal is **feature parity with the current app**, not a redesign. When in
doubt about how something should behave, the existing app is the spec.

---

## 0. First steps (do these before writing code)

1. **Clone Zed as a reference** (outside this project, shallow is enough):

   ```sh
   git clone --depth 1 https://github.com/zed-industries/zed /Users/sharith/Desktop/repos/zed
   ```

   Do not vendor it into this repo. Treat it as read-only reference material.
   GPUI's docs are thin; Zed's source is the real documentation. See §4 for where to look.

2. **Read the source app.** It lives at
   `/Users/sharith/Desktop/repos/agents/dbclient` (read-only for you: do not edit
   it). Read, in this order:
   - `README.md`: what Boa is
   - `docs/USAGE.md`: **the behaviour spec**, feature by feature
   - `docs/DEVELOPMENT.md`: layout of the code
   - `crates/boa-desktop/src/`: the Rust backend you will reuse
   - `crates/boa-desktop/ui/src/`: the React app you will replace

   Record the source commit you port from (`git -C … rev-parse HEAD`) in this
   file's "Progress log" (§10), so later changes to the Tauri app can be forward-ported.

3. `git init` here, add a `.gitignore` (`target/`), and commit in small steps.

4. **Do the Phase 0 spike (§6) and report back to the user before starting
   Phase 1.** It is a go/no-go checkpoint.

---

## 1. The source app at a glance

| Layer | Path (under `crates/boa-desktop/`) | Lines | Fate |
|---|---|---|---|
| Rust backend | `src/` | ~5.0k | **Reuse.** Strip the Tauri layer. |
| Pure TS logic | `ui/src/lib/`, parts of `ui/src/state/` | ~7.2k | **Port to Rust** (straight translation, with tests). |
| TS tests | `ui/src/**/*.test.ts` | ~2.7k | **Port** alongside the logic they cover. |
| React UI | `ui/src/components/` | ~9.4k | **Rewrite** in GPUI. |

### Backend modules (`src/`)
- `pg.rs` (1.7k): sessions per (connection, database), server-side cursors and
  paging, cancel, reconnect / retry-safety rules (`safe_to_retry`, `commits`,
  lost-transaction and commit-unknown errors), read-only enforcement.
- `pg/catalog.rs`: introspection (schemas, tables, columns, indexes,
  constraints, row estimates) and DDL generation.
- `pg/export.rs`: CSV export via `COPY … TO STDOUT`, atomic temp-file rename, progress.
- `tx.rs`: transaction-state probe after every statement (idle / open / aborted).
- `edits.rs`: applying staged grid edits in one transaction or savepoint.
- `connections.rs`: `connections.json` (passwords included, owner-only file).
- `history.rs`: `history-<connection id>.jsonl` per connection.
- `ssh.rs`: SSH tunnels via `/usr/bin/ssh -N -L` in batch mode.
- `error.rs`: `CmdError` / `CmdResult`.
- `updater.rs`: Tauri updater, app menu, About window (**replace**, see §5).
- `lib.rs`: the 26 `#[tauri::command]` wrappers + app setup (**replace**).

The command list in `lib.rs` (`generate_handler![…]`) is the complete backend
API the UI uses; `ui/src/lib/backend.ts` shows the shape of each call and its
events (`export-progress`-style emits, `quit-requested`, update events).

### Pure logic to port (`ui/src/lib/`, `ui/src/state/`)
Each has a `.test.ts`. Port the tests first, then the code, and keep the same cases.
- `sql.ts`: SQL tokenizer (used by highlighting, guards, statement splitting)
- `statements.ts`: split a buffer into statements / find the one under the caret
- `complete.ts` (721): schema-aware completion (tables, columns, aliases, CTEs, types, FK join conditions)
- `guard.ts`: destructive / read-only statement detection
- `copyAs.ts`, `csv.ts`, `values.ts`, `format.ts`: copy-as formats, CSV, value display
- `gridEdits.ts`, `state/edits.ts`: staged edits → SQL
- `tableQuery.ts`, `tableMeta.ts`: a table tab's SELECT with WHERE / ORDER BY, validation
- `history.ts`, `state/history.ts`: "same query" normalization, search, retention
- `connTag.ts`: environment tags / colours
- `state/tx.ts`, `state/guards.ts`, `state/session.ts`, `state/prefs.ts`,
  `state/store.ts` (1.2k, the app state: connections, tabs, results,
  reconnect / retry logic, toasts). This becomes your GPUI model layer.

**Do not port:** `state/mock*.ts` (browser mock), `lib/window-chrome.ts`,
`capabilities/`, `permissions/`, `tauri.conf.json`, `build.rs`, the shadcn
`components/ui/*` wrappers (replace with GPUI equivalents).

---

## 2. Hard constraints

- **Passwords stay in `connections.json`.** Do not use the macOS Keychain. The
  user tried Keychain and rejected it because of the prompts.
- **Keep the on-disk formats and location** so existing users keep their data:
  config dir `~/Library/Application Support/dev.boa.desktop/` (Tauri's
  `app_config_dir` for identifier `dev.boa.desktop`; verify on disk),
  `connections.json`, `history-<id>.jsonl`. Keep the bundle identifier
  `dev.boa.desktop`.
- Prefs, session restore (tabs, SQL, layout) and CSV options currently live in
  WebView `localStorage` (`boa.prefs.v2` etc.). Move them to JSON files in the
  config dir (`prefs.json`, `session.json`) with the same fields as
  `state/prefs.ts` / `state/session.ts`. A one-time import from the WebView's
  storage is nice to have, not required. Ask the user if it looks hard.
- **macOS only** (minimum 11.0), Apple silicon and Intel.
- **License: Boa is MIT.** `gpui` is Apache-2.0, so you can depend on it. Most other
  Zed crates (`editor`, `ui`, `workspace`, `picker`, …) are **GPL/AGPL**:
  read them for patterns but **do not copy their code** into this repo or
  depend on them. Check each crate's `LICENSE-*` file / `Cargo.toml` before reusing anything.
- Keep the workspace lints from the source `Cargo.toml` (`unsafe_code = "deny"`,
  clippy `all` = warn, clippy clean with `-D warnings`), edition 2024, and a
  pinned toolchain (`rust-toolchain.toml`; the source pins 1.99.1. Use whatever
  GPUI needs if that's newer).

---

## 3. Target architecture

```
boa-gpui/
  Cargo.toml                 workspace
  crates/boa-core/           no UI deps: backend (copied from src/, Tauri removed)
                             + the ported TS logic + all their tests
  crates/boa-app/            GPUI app: models, views, actions/keymap, theme, menus
  docs/                      USAGE.md etc. carried over and updated
```

- **boa-core** must build and test without GPUI. Copy `pg.rs`, `pg/`, `tx.rs`,
  `edits.rs`, `connections.rs`, `history.rs`, `ssh.rs`, `error.rs`; replace
  `AppHandle`/`State`/`emit` with plain arguments and callbacks or channels.
  `CmdError`'s serde shape no longer matters, but keep its fields (message,
  detail, kinds like network / tx-lost / commit-unknown) because the UI shows them.
- **Async bridge.** GPUI has its own executor; `tokio-postgres` and the SSH code
  need tokio. Run one multi-thread tokio runtime on a background thread and
  expose a `Backend` handle whose methods return futures the GPUI side can
  `cx.spawn` / await (e.g. `tokio::spawn` + `oneshot`). Progress events (CSV
  export, reconnect state) go over channels into GPUI entities. Look at how Zed
  bridges background work (`cx.background_spawn`, `Task`) and pick one pattern.
- **State.** `store.ts` (zustand) maps onto GPUI `Entity<T>` models with
  `cx.notify()` / `EventEmitter`. Suggested split: `Connections` (saved +
  live state, reconnect loop), `Workspace` (tabs, active tab, panels, MRU),
  `Results`, `History`, `Prefs`, `Toasts`. Views observe them.
- **Widgets.** Evaluate **gpui-component** (https://github.com/longbridge/gpui-component,
  Apache-2.0) in Phase 0. It may provide input / code editor, virtualized
  table, tree / list, dialog, popover, menu, notifications. Using it could save
  weeks. If it doesn't fit, build on raw GPUI and use Zed's (GPL) crates only
  as pattern references.

---

## 4. Where to look in the Zed clone

| Need | Look at |
|---|---|
| GPUI basics, runnable examples | `crates/gpui/examples/`, `crates/gpui/README.md`, `crates/gpui/src/` |
| Text input from scratch (IME, selection, caret) | `crates/gpui/examples/input.rs` (`EntityInputHandler`) |
| Virtualized lists / big tables | `uniform_list`, `list` in `crates/gpui/src/elements/` |
| Actions, keymaps, key contexts | `actions!` usage, `assets/keymaps/default-macos.json`, `crates/gpui/src/keymap/` |
| App menu, quit handling, windows | `crates/zed/src/zed.rs`, `crates/zed/src/main.rs`, `crates/gpui/src/platform/mac/` |
| Titlebar / traffic lights | `crates/title_bar/`, `WindowOptions { titlebar: … }` |
| Docks, panes, resizable panels | `crates/workspace/` (GPL: patterns only) |
| Tree view | `crates/project_panel/` (GPL: patterns only) |
| Command palette / fuzzy picker | `crates/command_palette/`, `crates/picker/` (GPL: patterns only) |
| Code editor, highlighting, completion menu | `crates/editor/` (GPL, very large: patterns only) |
| Theming | `crates/theme/` |
| Notifications / toasts | `crates/notifications/`, `crates/workspace/src/notifications.rs` |
| Auto-update | `crates/auto_update/` |
| File dialogs | `cx.prompt_for_paths` / `prompt_for_new_path` |
| Tests | `#[gpui::test]`, `TestAppContext`, `VisualTestContext` |

Also check whether `gpui` is published on crates.io at a usable version, or
whether you need a git dependency pinned to a Zed commit. **Pin it either way**:
GPUI's API changes often.

---

## 5. Platform pieces Tauri used to provide

- **Window:** 1280×820, min 720×480, background `#151616`, transparent /
  overlay titlebar with hidden title, traffic lights at (14, 22). Tabs live in
  the title bar (`components/TitleBar.tsx`).
- **App menu:** Boa → About Boa, Check for Updates…, Settings, Quit; plus
  Edit / Window menus. See `updater.rs::menu` / `on_menu_event`.
- **Quit guard:** ⌘Q / Quit must not exit while a transaction is open. The UI
  asks first (commit / rollback / cancel). A second ⌘Q within 3 s quits anyway,
  so a stuck UI can't keep the app open. See `lib.rs` `RunEvent::ExitRequested`.
- **About window:** `components/AboutWindow.tsx` (version, Copy version info).
- **Updater:** today `tauri-plugin-updater` reads
  `https://github.com/sharithg1/boa/releases/latest/download/latest.json` and
  verifies a minisign signature (pubkey in `tauri.conf.json`). Options: Sparkle,
  or your own updater that reads the same `latest.json`, verifies minisign, and
  swaps the `.app`. **Important for existing users:** installed Tauri builds
  will update to the first GPUI release through the *old* updater, so that
  release must be published in the format the Tauri updater expects
  (`.app.tar.gz` + `.sig` + `latest.json`). Verify this end to end before
  shipping. Ask the user before touching the release pipeline.
- **Bundling / signing / notarization:** `.github/workflows/release.yml`,
  `scripts/release.sh`, `scripts/check-version.sh`, `docs/RELEASING.md` in the
  source repo. Rebuild them without the Tauri bundler (e.g. `cargo-bundle` or a
  script that builds the `.app` + `create-dmg`). Keep the icons from `icons/`.
- **Fonts:** Geist and Geist Mono (variable). Bundle them as assets.
- **Theme:** dark, colours from `ui/src/assets/main.css` (CSS variables:
  background, chrome, sidebar, field, hover, borders, text shades body / soft /
  quiet / faint / dim / ghost, destructive, environment colours in
  `lib/connTag.ts`). Text sizes 12 / 13 / 14 / 16 / 20 px. Match the look in
  `docs/images/boa.png`.

---

## 6. Phases

Each phase ends with a working, committed app. Run `cargo fmt`, `cargo clippy
--workspace --all-targets -- -D warnings` and `cargo test --workspace` before every commit.

### Phase 0: spike, go/no-go (about 1 week)
Build the two hardest pieces against the real backend, and nothing else:
1. **SQL editor:** multi-line editing, undo/redo, selection, IME, scrolling,
   line gutter, syntax highlighting from the ported tokenizer, a completion
   popup driven by ported `complete.rs`, ⌘↵ runs the statement under the caret.
2. **Result grid:** virtualized, 100k rows scroll smoothly, column resize, cell
   and range selection, ⌘C copies TSV, inline cell edit.

Also: set up the workspace, the tokio bridge, boa-core with `pg.rs` compiling,
and one hard-coded connection. Decide **gpui-component vs. raw GPUI** here.

**Stop and report to the user:** what worked, what was painful, memory and
startup numbers vs. the Tauri app, the widget-library decision, and your revised
estimate. Don't continue without their go-ahead.

### Phase 1: core shell
Window chrome, title-bar tabs, theme, fonts, app menu, keymap plumbing,
resizable sidebar / bottom / right panels (sizes persisted), prefs.json,
toasts, status bar.

### Phase 2: connections
Saved connections, connection dialog (incl. `postgresql://` URL paste, SSL
modes, SSH tunnel tab, environment tag + colour, read-only), password prompt,
test connection, connect / disconnect, live states (connected / connecting /
reconnecting / failed) and the **reconnect behaviour in USAGE.md "Staying
connected"**: backoff for about 2 min, wake / network-change retry, one status toast
per connection (not one per retry round; see the source's latest commits).

### Phase 3: explorer
Database Explorer tree (connection › database › schema › tables › columns,
plus keys / foreign keys / indexes / checks folders), "N of M" scope picker,
row counts, PK / FK icons, context menus (copy name / definition, Show DDL,
Export to CSV, open table), expanded state persisted.

### Phase 4: consoles and results
Full console tabs on the Phase 0 editor; statement chooser; run / cancel;
run progress; Database Output panel with result tabs and the Output log;
paging (`Pager`); breadcrumbs; JSON viewer; value editor (⇧↵).

### Phase 5: table data editing
Table tabs, WHERE / ORDER BY filter bar with validation, header-click sort
(⇧-click multi-sort), staged edits tinted per kind, add / delete / set NULL /
revert, SQL preview (⌘↵), submit in one transaction, the "pending changes" confirmations.

### Phase 6: transactions and guards
Tx Auto / Manual per session, commit / rollback (⌘⇧↵ / ⌘⇧⌫), real
transaction state in the toolbar and status bar, destructive-statement
confirmations (always on for Production), read-only, disconnect / quit guards.

### Phase 7: the rest
Query history (⌘⌥E, right panel History tab, search, errors-only, all
connections, insert / open in new console / copy / delete), Copy as…, CSV
export with progress / cancel, command palette (⌘K) with recents, Settings
view, session restore with reconnect at launch, right panel (structure / info / history).

### Phase 8: ship
Updater (see §5), bundling, signing, notarization, CI (`ci.yml` equivalent),
README / docs updated (`docs/USAGE.md` should still be true), migration test
with a real config dir from the Tauri app.

---

## 7. Parity checklist

Tick these off against `docs/USAGE.md`. That file has the precise behaviour for each.

- [ ] Data sources: add / edit / delete, URL paste, SSL modes, SSH tunnel (agent or key file), test
- [ ] Environment tag + colour shown on explorer, editor, console toolbar, status bar
- [ ] Read-only connections (server-enforced, restored if turned off)
- [ ] Explorer tree with keys / FKs / indexes / checks; scope picker; row counts; views toggle
- [ ] DDL view (`pg_get_*def`), opens in a new console
- [ ] Console: highlighting, completion (incl. FK join conditions), ⌘↵ statement under caret
- [ ] Paged results with column types; cancel; Output log (⌥J)
- [ ] Grid selection (cell, range, rows, ⌘A, Esc), ⌘C TSV, Copy as (TSV ± header, CSV, Markdown, JSON, INSERT, UPDATE, IN list)
- [ ] Table data editing for PK tables; read-only reasons for others; preview + transactional submit
- [ ] WHERE / ORDER BY bar, header sort, one-clause validation, server errors in place
- [ ] Tx Auto / Manual, state probe, commit / rollback shortcuts, typed `begin` / `commit` / `rollback`
- [ ] Destructive-statement confirmations; setting; Production always confirms
- [ ] Query history: per connection on disk, retention setting, max 5,000, "same query" dedupe, search
- [ ] CSV export: COPY streaming, temp + rename, progress / cancel, savepoint rules, delimiter / header options
- [ ] Session restore: tabs, SQL, filters / sorts, active tab, layout; reconnect at launch (~30 s retry)
- [ ] Staying connected: keepalive detection, safe auto-retry only, tx-lost and commit-unknown messages, backoff, wake retry
- [ ] Quit / disconnect guard with open transactions; double ⌘Q escape hatch
- [ ] Command palette ⌘K; Settings; About; updates ("Restart to update")

Keyboard shortcuts in USAGE.md: ⌘O, ⌘K, ⌘↵, ⌘⇧↵, ⌘⇧⌫, ⌥J, ⌘⌥E, ⇧↵,
⌘⌥N, ⌘N, ⌘⌫, ⌘⌥Z, ⌘A, ⌘C, Esc. Grep the source components for others
(`onKeyDown`, `metaKey`) before declaring parity.

---

## 8. Testing

- All ported TS tests become Rust unit tests in boa-core, same cases.
- Keep the backend's existing Rust tests, including the live Postgres test
  (runs when `PGHOST`/`PGPORT`/`PGUSER`/`PGPASSWORD`/`PGDATABASE` are set;
  it creates and drops a `boa_test` schema). Run a local Postgres for it.
- UI: `#[gpui::test]` for model logic and key flows (run statement, stage and
  submit edits, quit guard). The old headless-Chrome driver doesn't apply anymore.
- Before declaring a phase done, run the app (`cargo run -p boa-app`) against a
  real database and exercise the phase's features by hand, or take screenshots
  if you can. Report anything you couldn't verify.

---

## 9. Working agreements

- Don't modify the source repo. Don't push anywhere or create remotes without asking.
- Ask the user before decisions that are hard to undo: dropping a feature,
  changing on-disk formats, release / updater changes, adding a GPL dependency (don't).
- Prefer small, reviewable commits with clear messages, one feature at a time.
- When GPUI fights you, look for how Zed solves the same thing before inventing a pattern.
- Keep this file current: update §10 at the end of every session.

---

## 10. Progress log

Phase 0 is done and approved. **Phases 1–8 continue in [HANDOFF-PARITY.md](HANDOFF-PARITY.md)**,
which supersedes this file's phase plan and keeps its own progress log.

| Date | Source commit ported from | Phase | Notes |
|---|---|---|---|
| 2026-11-26 | `61984bb126ea7631fcc4e1ae5e288314886eb431` | 0 (done, awaiting go/no-go) | Summary in `docs/phase0/README.md`. gpui-component 0.11.0 + gpui-pre =0.3.14; own grid. Phase 0 shortcuts to undo later: `boa-app/src/spike.rs` (hard-coded connection, temp config dir), grid page size 100k (`BOA_GRID_PAGE_SIZE`), every result editable, dev env hooks in `grid/dev.rs` / editor (`BOA_EDITOR_*`). |
| | | | Setup notes: | Zed reference clone at `a3ce8d5`. Local Postgres 17 (Homebrew, `pg_ctl`, not a service): role `boa`/`boa`, db `boa_dev` with seeded `customers` (5k) / `orders` (100k). No full Xcode on the dev machine → gpui `runtime_shaders`. Work split across worktree branches `p0/{core,sql,gridlogic,app}`, merged into `main`. |
