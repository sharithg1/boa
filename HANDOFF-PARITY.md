# Handoff: build Boa on GPUI to full parity (Phases 1–8)

You are taking over the port of **Boa**, a macOS PostgreSQL client, from Tauri 2 +
React to a pure-Rust **GPUI** app. Phase 0 (the go/no-go spike) is done and the
user has approved going ahead. Your job is to finish the app, end to end, until it
does everything the Tauri app does, and is ready to ship to its existing users.

- Repo (this one): `/Users/sharith/Desktop/repos/boa-gpui`, branch `main`.
- Source app, **the spec** (read-only, never edit): `/Users/sharith/Desktop/repos/agents/dbclient`
  at commit `61984bb126ea7631fcc4e1ae5e288314886eb431`. If it has moved on since,
  `git -C … log 61984bb..HEAD` and forward-port those changes too (record it in §13).
- The original plan: `HANDOFF.md` (still valid for constraints and background; this file
  supersedes its phase plan and adds everything learned in Phase 0).

**Definition of done:** every item of the parity checklist (§11) is ticked with the
verification it names; every behaviour in the source's `docs/USAGE.md` is true of
the GPUI app; all keyboard shortcuts in §10 work; existing users' data (connections,
passwords, history, prefs, session) carries over; the app is bundled, signed,
notarized and can be delivered through the old Tauri updater (§9 Phase 8 —
**ask the user before touching anything release-related**).

---

## 1. Read first, in this order

1. `HANDOFF.md` §2 (hard constraints), §5 (platform pieces Tauri provided), §9 (working agreements).
2. `docs/phase0/README.md` then `widgets.md`, `editor.md`, `grid.md`, `numbers.md`
   (decisions, numbers, known gaps and estimates per pane).
3. `crates/boa-core/README.md` (the Backend API, events, module layout).
4. Source: `README.md`, `docs/USAGE.md` (**the behaviour spec**), `docs/DEVELOPMENT.md`,
   `docs/RELEASING.md`, then `crates/boa-desktop/ui/src/state/store.ts` (1.2k lines:
   the app's whole state machine) and the component for whatever you work on.
5. Reference code: Zed at `/Users/sharith/Desktop/repos/zed` (GPUI patterns; GPL outside
   `crates/gpui*` — read, never copy) and gpui-component at
   `/Users/sharith/Desktop/repos/gpui-component` (Apache-2.0; examples in `crates/story`).
   The exact versions we build against are in `~/.cargo/registry/src/*/{gpui-pre-0.3.14,gpui-component-0.11.0,gpui-base-0.11.0}`
   — read those, not the clone's HEAD, when the API matters.

---

## 2. What exists on `main`

```
Cargo.toml                 workspace (members crates/*), lints, pins, release profile
rust-toolchain.toml        1.99.1 (rust-version = 1.88: let-chains are used)
crates/boa-core/           no UI deps. 189 tests (incl. live Postgres)
  src/backend.rs           Backend: own tokio runtime, one async method per command,
                           BackendTask<T> futures any executor can await; events over
                           async-channel (CSV export progress, LinkEvent::{Lost,TxLost})
  src/model.rs             the one set of catalog/result types (Schema, Table, Column,
                           Index, Constraint, ForeignRef, QueryResult, ResultColumn, Value)
  src/pg.rs, pg/*, tx.rs, edits.rs, connections.rs, history.rs, ssh.rs, error.rs, config.rs
                           the Tauri backend, Tauri layer removed, behaviour + formats unchanged
  src/sql/                 ported sql.ts, statements.ts, complete.ts, guard.ts (UTF-8 byte offsets)
  src/grid/                ported values, format, csv, copyAs, gridEdits, state/edits,
                           tableQuery, tableMeta, connTag
crates/boa-app/            GPUI app. 65 tests
  src/app.rs               app init, window options, menu (About, Quit only), About window,
                           screenshot hook (feature "screenshot")
  src/theme.rs             Palette from main.css, fonts, text sizes; pushed into gpui-component's Theme
  src/assets.rs            Geist / Geist Mono TTFs (OFL) + gpui-component icons
  src/workspace.rs         title-bar strip + editor/results split (placeholder chrome)
  src/editor/              SqlEditor on gpui-component's Editor: highlighting, completion,
                           ⌘↵ → RunStatement, statement outline
  src/grid/                ResultGrid on uniform_list: 100k rows @120 Hz, selection, ⌘C TSV,
                           column resize, inline edit → Pending, pager, error caret
  src/spike.rs             PHASE 0 SHORTCUT: one hard-coded connection (see §8)
docs/phase0/               spike reports and screenshots
```

Dependency pins (root `Cargo.toml`): `gpui = { package = "gpui-pre", version = "=0.3.14" }`,
`gpui_platform` (gpui-pre-platform, features `font-kit`, `runtime_shaders`),
`gpui-component =0.11.0`, `gpui-component-assets` (gpui-kit-assets). **Bump all four
together and only on purpose**, to the gpui-pre version that gpui-component release pins.

---

## 3. Environment

- Rust is not on PATH in non-interactive shells: `export PATH="$HOME/.cargo/bin:$PATH"`.
- Only the Xcode Command Line Tools are installed (no `metal` compiler): keep
  `runtime_shaders`. If release builds should use precompiled shaders, that needs full
  Xcode on the build machine (CI) — ask the user.
- Local Postgres 17 (Homebrew, **not** a launch service):
  `/opt/homebrew/opt/postgresql@17/bin/pg_ctl -D /opt/homebrew/var/postgresql@17 -l /opt/homebrew/var/log/postgresql@17.log start`
  (`stop` likewise). Role `boa`/`boa` (superuser), database `boa_dev` with
  `public.customers` (5k) and `public.orders` (100k, FK, jsonb). Don't drop them; add your
  own fixtures in other schemas/databases (views, checks, multi-column FKs, a table
  without a PK, a second database, a read-only role, non-ASCII names, ~50 tables
  for the explorer, a slow function for cancel tests).
- Tests: `PGHOST=localhost PGPORT=5432 PGUSER=boa PGPASSWORD=boa PGDATABASE=boa_dev cargo test --workspace`.
  Without `PGHOST` the live tests skip; **always run with it** before merging.
- SSH tunnel testing: `/usr/bin/ssh` is used in batch mode. For an end-to-end test you
  need an sshd; Remote Login is off unless the user turns it on — ask, or test via
  the existing ssh.rs unit tests plus a manual check by the user.
- **Seeing the app.** Agents in Phase 0 had no Screen Recording / Accessibility
  permission, so:
  - `cargo run -p boa-app --features screenshot` with `BOA_SCREENSHOT=/abs/out.png`
    (and `BOA_SCREENSHOT_DELAY_MS`) renders the main window to a PNG via
    `render_to_image` and quits. Native parts (traffic lights, menus, dialogs that are
    separate windows) are not in the image. Extend the env-var hooks (`grid/dev.rs`,
    `BOA_EDITOR_TEXT`/`BOA_EDITOR_TYPE`) to script state before a capture.
  - Interactions: `#[gpui::test]` + `VisualTestContext` (`simulate_keystrokes`,
    `simulate_input`, mouse events). These go through the real keymap/dispatch tree.
  - **Ask the user early** whether they can grant Screen Recording + Accessibility to
    the terminal running you; then `screencapture -l <windowid>` and real input
    (e.g. `osascript` System Events) become available and native chrome can be checked.
- Never run broad `pkill boa-app`: kill only PIDs you started (the user may be running
  the app, and parallel agents run their own copies). `BOA_GRID_QUIT_AFTER_MS` makes the
  app quit itself.
- The user's real Tauri install is `/Applications/Boa.app` and their real data is in
  `~/Library/Application Support/dev.boa.desktop/`. **Never write there from tests or
  dev runs.** Develop against a separate config dir (see §8, `BOA_CONFIG_DIR`). For
  migration tests copy the dir to a scratch location first.

---

## 4. Hard constraints (unchanged from HANDOFF.md §2, plus Phase 0 decisions)

- Passwords stay in `connections.json` (owner-only). **No Keychain.**
- Same config dir, same `connections.json` and `history-<id>.jsonl` formats, bundle id
  `dev.boa.desktop`. Prefs / session / CSV options move from WebView localStorage to
  `prefs.json` / `session.json` / `csv-export.json` in the config dir with the **same
  fields** as `state/prefs.ts` (`boa.prefs.v2`), `state/session.ts` (`boa.session.v1`) and
  `state/csvExport.ts` (`boa.csvExport`). Changing any on-disk format needs the user's OK.
- One-time import of the old localStorage is feasible: WebKit keeps it at
  `~/Library/WebKit/dev.boa.desktop/WebsiteData/Default/<salt>/<salt>/LocalStorage/localstorage.sqlite3`
  (SQLite, `ItemTable(key, value)`; values are UTF-16LE blobs — verify). Read it
  read-only (copy the db + wal first), import only when `prefs.json` / `session.json` don't
  exist yet, and never fail startup because of it. Confirm with the user before relying on it.
- macOS 11.0+, Apple silicon and Intel (CI must build both, or a universal binary).
- MIT. Dependencies Apache/MIT/BSD/ISC/OFL/Unicode/MPL-unmodified only. **No GPL/AGPL** (no Zed
  crates other than gpui*). Add `cargo deny` (licences) in CI.
- Lints: `unsafe_code = "deny"` (if a platform call truly needs unsafe — e.g. sleep/wake
  notifications — isolate it in one small module with a scoped `#[allow]` and ask the user
  first), clippy clean with `-D warnings`, edition 2024, toolchain pinned.

---

## 5. Lessons from Phase 0 (read before writing UI code)

GPUI / gpui-component:
- Every window's root view must be gpui-component's `Root`, or dialogs, sheets,
  notifications and popovers silently don't work (`push_notification` *panics* without it —
  wrap test windows in `Root` too).
- Editor colours come only from the global `Theme` (`highlight_theme`); per-editor style
  overrides are reset on render. The Editor forces 1.5 line height; we override to 20 px.
- The completion menu is closed: label + detail rows only, private selected index, Enter
  accepts, **Tab does not** (we bind Tab in `SqlEditor > Input` to dispatch Enter when the menu is
  open). For parity (per-kind icons, Tab replaces the whole word, PageUp/Down, sticky explicit mode)
  draw our own popup from the editor's caret geometry (planned in Phase 4).
- We rely on `#[doc(hidden)]` APIs: `soft_wrap`, `folding`, `completion_menu_state()`. Keep
  them behind our wrapper types so an upgrade breaks in one place.
- gpui-component installs bindings: ⌘⌥I inspector (debug), Root binds tab / shift-tab / ⌘C,
  `Input` has ~86 bindings (⌘F, ⌘., ⌘⌥↑/↓ …). Our bindings must live in narrower key
  contexts or be bound later at the same depth to win. Audit against §10.
- Mouse drags that leave an element need a `canvas` registering window-level
  `MouseMove`/`MouseUp` while dragging (Zed's editor pattern).
- A `track_focus` parent re-focuses itself on mouse-down *after* the child handler; call
  `window.prevent_default()` when a child moves focus (e.g. into an inline input).
- Typing a character that opens an input must stop propagation, or the key is lost/duplicated.
- `Input` with `appearance(false)` keeps padding; override `px_0/py_0/h_full/text_size`.
- GPUI can't load WOFF2; fonts are TTF. No italic Geist Mono is bundled (comments aren't
  italic) — add the italic TTFs (OFL) if the Tauri look has them.
- Traffic lights: GPUI's `traffic_light_position` y means the button top; Tauri's (14, 22)
  maps to (14, 16). Not yet verified on screen side by side.
- Tests: focus events and overlays need a drawn frame first; `simulate_keystrokes("enter")` also
  types "\n" when an action propagates — send Enter as a bare key-down where that matters.

Backend:
- `BackendTask`s start when called; dropping one does not cancel — use `cancel_query`.
- The reconnect loop is **not** in boa-core: it must be built in the app's `Connections` model
  on `link_events()` + `ping` (port `store.ts` reconnect logic + `state/reconnect.test.ts`).
- CSV export sends progress only every 250 ms; small exports send none. Don't wait for one.
- `Backend::apply_edits` takes `Vec<edits::EditStatement>`: `stmts.iter().map(Into::into).collect()`.
- Grid memory: rows are `Vec<Vec<Option<String>>>` (~60–80 B/cell). Fine at the real page
  sizes (500 default, 5,000 max); revisit only if pages grow.

Process:
- A pipeline like `cargo test | grep` hides failures. Check each command's exit code.
- Raising `rust-version` enables new clippy suggestions (let-chains); re-run clippy.
- Concurrent heavy builds make timing-sensitive tests flaky (`ssh::tests::unreachable_ssh_server_fails_cleanly`);
  re-run alone before calling it a failure.

---

## 6. How to work (subagents)

Phase 0 was built by subagents in parallel git worktrees, merged by the lead. It worked
well (≈75 min wall-clock for Phase 0). Keep doing it:

1. **Contract first.** Before fanning out, the lead lands on `main` the shared types, entity
   skeletons, events and module stubs the parallel agents build against (as was done
   with `RunStatement` → `ResultGrid::run_sql`). Each agent then owns a disjoint set of files.
2. **One worktree per agent:** `git worktree add .worktrees/<name> -b p<N>/<name>`
   (`.worktrees/` is gitignored). Agents commit to their branch only; never push.
3. **Agent prompt template:** the environment block of §3, the rules (fmt/clippy/test must pass,
   commit trailer, no system installs, don't touch other worktrees or the source repo), the
   contract, the exact source files that are the spec for its slice, required tests,
   required screenshots, and a ≤400-word report (built / API / tests / not verified / gotchas).
4. **Merge protocol (lead):** `git merge --no-ff --no-commit`, resolve (`Cargo.lock`: take
   one side and let cargo re-resolve), then all three checks with exit codes checked, then
   commit. Never commit a red `main`.
5. **Review, don't rubber-stamp:** read each report's "not verified" and "deviations" lists;
   either fix, schedule, or record them in §13.
6. Keep 2–4 agents per wave; more makes merges and the machine (18 cores, 24 GB) the bottleneck.
7. End every commit message with the attribution line the system asks for (currently
   `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`).

---

## 7. Target app architecture

Replace `store.ts` (zustand) with GPUI entities. Port the remaining pure TS logic into
boa-core **with its tests first** (same cases), then build the entities on it.

Still to port into boa-core (each has a `.test.ts` or is covered by store tests):
`lib/history.ts` + `state/history.ts` (same-query normalisation, search, retention, 5,000 cap),
`state/tx.ts` (tx mode/state per session), `state/guards.ts` (when to confirm),
`state/session.ts` (parse / prune / `nextTabNumber`), `state/prefs.ts` (defaults, clamps,
`applyOrder`, reset keeping layout), `state/csvExport.ts` (export source rules, savepoint
rules), `state/ddl.ts`, `state/links.ts` (already in Backend; check), `lib/updates.ts`,
`state/reconnect.test.ts` / `scope.test.ts` / `session.test.ts` (store behaviour: port as model tests).
`components/ConnectionDialog.test.ts` (URL paste parsing) → boa-core too.
Do **not** port `state/mock*`, `lib/window-chrome.ts`, `components/ui/*`.

Entities in boa-app (suggested; one module each, observed by views):

| Entity | Owns | From |
|---|---|---|
| `Prefs` | prefs.json, defaults, clamps, change events | state/prefs.ts |
| `Connections` | saved list, live state per connection (connected / connecting / reconnecting / failed), password prompts, reconnect loop + backoff, wake/network retry, one status toast per connection | store.ts, links.ts, reconnect.test.ts |
| `Catalogs` | per (connection, database): schemas in scope, introspection cache, row estimates, refresh | store.ts, ScopePicker, Sidebar |
| `Workspaces` | per database: tabs (console / table), active tab, MRU, `seen`, active database | store.ts, session.ts |
| `Console` / `TableTab` | SQL, results (result tabs + Output log), run/cancel, paging, filter/sort, staged edits (`EditsState`) | Workspace.tsx, BottomPanel.tsx, TableEdits.tsx |
| `Transactions` | tx mode + probed state per session, commit/rollback | state/tx.ts, TxControls.tsx |
| `History` | per-connection history via Backend, search, retention | state/history.ts |
| `Exports` | running CSV exports, progress, cancel | state/csvExport.ts |
| `Toasts` | keyed toasts (replace same key), actions | store.ts pushToast |
| `Session` | session.json, debounced save (250 ms) on change, restore at launch | state/session.ts |
| `Updates` | check / download / "Restart to update" | lib/updates.ts, updater.rs |

Views mirror the React components (one module per component): TitleBar (tabs live in it),
Sidebar (explorer), Workspace (editor + toolbar), BottomPanel (Database Output), DataGrid,
RightPanel (Structure / Info / History), StatusBar, CommandPalette, ConnectionDialog,
SettingsView, QueryHistory, GuardDialogs, TxControls, TableFilterBar, TableEdits, Pager,
RunProgress, JsonViewer, ValueEditor, CsvExport, ScopePicker, StatementChooser,
CompletionPopup, Toasts, AboutWindow. Actions + keymap in one place (`actions.rs`/`keymap.rs`),
with key contexts per view.

---

## 8. Remove the Phase 0 shortcuts (do this in Phase 1/2)

- `src/spike.rs` (hard-coded connection in a temp config dir) → the real `Connections`
  model. Keep a **dev override**: `BOA_CONFIG_DIR=<dir>` makes the app use another config dir
  (default: the real one) so development and screenshots never touch the user's data.
- Grid page size 100,000 / `BOA_GRID_PAGE_SIZE` → `prefs.rowLimit` (default 500, Settings caps 5,000).
- Every result editable → `grid_edits::editability` (table tabs of tables with a PK only;
  read-only reasons shown otherwise).
- ⌘↵ runs immediately → arm-then-run + statement chooser (USAGE.md, Workspace.tsx).
- "Copied …" in the row-count pill → real toasts.
- Env dev hooks (`grid/dev.rs`, `BOA_EDITOR_*`): keep, but compile them only with the
  `screenshot` / a `dev-hooks` feature, never in release builds.

---

## 9. Phases

Each phase ends with a working, committed `main` (fmt, clippy `-D warnings`, tests with
`PGHOST` set), the relevant §11 items ticked, screenshots in `docs/phaseN/` for visual
pieces, and §13 updated. Spec per phase = the listed USAGE.md sections + components.
Rough effort (one experienced dev): ~12–14 weeks total; with parallel agents much less
calendar time, bounded by verification.

### Phase 1 — core shell (~1.5–2 wk)
Spec: USAGE "Layout"; TitleBar.tsx, Sidebar.tsx (frame only), RightPanel.tsx (frame),
BottomPanel.tsx (frame), StatusBar.tsx, PanelResizeHandle.tsx, Toasts.tsx, SettingsView.tsx
(shell), AboutWindow.tsx, App.tsx (`useShortcuts`), updater.rs `menu`/`on_menu_event`, main.css.
- Window chrome per HANDOFF §5; tabs in the title bar (overflow, close, reorder if the source does).
- Resizable sidebar / right / bottom panels with the prefs clamps (sidebar 200–480, right
  280–800, bottom 120–min(900, window−200)), sizes and open state persisted; ⌥B / ⌥E / ⌥J.
- `Prefs` + prefs.json (+ optional localStorage import, ask), `Toasts`, status bar.
- App menu: Boa → About Boa, Check for Updates…, Settings… (⌘,), Quit (⌘Q); Edit (undo,
  redo, cut, copy, paste, select all — must reach the focused input) and Window menus.
- About window incl. "Copy version info" (Enter copies).
- Keymap plumbing: all global shortcuts of §10 dispatch actions (stubs allowed for later phases);
  the ⌥-shortcut rule from `isOptionShortcut` (don't steal ⌥-composed characters on non-US layouts).
- `BOA_CONFIG_DIR` override.

### Phase 2 — connections (~1.5 wk)
Spec: USAGE "Data sources", "SSH tunnel", "Staying connected"; ConnectionDialog.tsx (+ its
test), ConnectionTag.tsx, StatusDot.tsx, store.ts connection + reconnect parts, links.ts,
reconnect.test.ts, connTag.ts; source commit 61984bb (one status toast per connection).
- Connection dialog: general fields, `postgresql://` URL paste, SSL modes, SSH tab (agent or
  key file, file picker), environment tag + colour, read-only, test connection, save password
  or not; edit / delete (with confirm); sidebar order drag (`connectionOrder`).
- Password prompt when none is stored / auth fails (`password_required`, 28P01, 28000).
- Connect / disconnect; live states; reconnect: backoff ~2 min, retry on wake and network
  change (macOS notifications — NSWorkspace did-wake / network path monitor; find a safe crate,
  or ask before any `unsafe`), only retry-safe statements auto-retry, tx-lost and
  commit-unknown messages, **one** keyed status toast per connection.
- Delete `spike.rs`.

### Phase 3 — explorer (~1–1.5 wk)
Spec: USAGE "Layout" (explorer parts); Sidebar.tsx, ScopePicker.tsx, tableMeta.ts, ddl.ts.
- Tree: connection › database › schema › tables (+ views toggle) › columns, plus keys / foreign
  keys / indexes / checks folders; PK / FK icons; row counts (pref); "N of M" scope picker
  (`scopes` pref, `'*'` semantics); expanded state persisted (`treeExpanded` keys exactly as
  Sidebar `nodeId`, so imported prefs keep working); virtualized (gpui-component Tree or
  `uniform_list`); keyboard navigation.
- Context menus: copy name / qualified name / definition, Show DDL (opens in a new console),
  Export to CSV, open table, refresh, new console; connection menu (connect, edit, disconnect…).
- Environment colour on explorer, editor, console toolbar, status bar.

### Phase 4 — consoles and results (~2.5–3 wk)
Spec: USAGE "Selecting and copying rows" + console parts; Workspace.tsx, StatementChooser.tsx,
CompletionPopup.tsx, BottomPanel.tsx, RunProgress.tsx, Pager.tsx, JsonViewer.tsx,
ValueEditor.tsx, DataGrid.tsx; docs/phase0/editor.md + grid.md gap lists.
- Console tabs (⌘T / ⌥N new, ⌘W close, ⌥1–9), per-database workspaces, ⌥↑/↓ cycle database.
- Editor parity: arm-then-run ⌘↵, statement chooser (subqueries), own completion popup
  (icons, Tab replaces word, PageUp/Down, sticky explicit, `count(|)` caret, schema re-open),
  default-schema picker, caret position in status bar, italic comments.
- Run / cancel, run progress, Database Output panel: result tabs + Output log (⌥J), errors with
  position, "N rows affected", row-limit reached.
- Grid parity: header click sort (local cycle), context menu with Copy / **Copy as ›** (TSV ±
  header, CSV, Markdown, JSON, INSERT, UPDATE, IN list), JSON viewer (Enter / double-click /
  hover button), value editor (⇧↵, auto for multi-line / >200 chars / JSON; ⌘↵ saves), breadcrumbs.
- Paging with `rowLimit` (Pager), "pending changes" prompts before paging / re-running.

### Phase 5 — table data editing (~1.5 wk)
Spec: USAGE "Editing table data", "Filtering and sorting table data"; TableEdits.tsx,
TableFilterBar.tsx, gridEdits.ts, state/edits.ts, tableQuery.ts.
- Table tabs (preview tabs if the source has them), WHERE / ORDER BY bar with one-clause
  validation and server errors in place, header sort → server ORDER BY with ⇧-click multi-sort.
- Staged edits tinted per kind; add row (⌘N, Alt+Insert, `<default>` placeholders), delete
  (⌘⌫ / Delete), set NULL (⌘⌥N), revert (⌘⌥Z); editability + read-only reasons.
- SQL preview (⌘↵) and submit in one transaction / savepoint (`apply_edits`), history entry,
  toasts; pending-changes confirmations on close / navigate.

### Phase 6 — transactions and guards (~1 wk)
Spec: USAGE "Transactions and guard rails"; TxControls.tsx, GuardDialogs.tsx, state/tx.ts,
state/guards.ts, guard.ts, lib.rs `RunEvent::ExitRequested`.
- Tx Auto / Manual per session, probed state (idle / open / aborted) in toolbar + status bar,
  commit ⌘⇧↵ / rollback ⌘⇧⌫ (also in the guard dialogs), typed `begin`/`commit`/`rollback`.
- Destructive-statement confirmations (pref; Production always), read-only enforcement +
  `read_only_escape` messages.
- Disconnect guard and **quit guard** (⌘Q / menu / last window close / system logout): ask
  commit / rollback / cancel when a transaction is open; a second ⌘Q within 3 s quits anyway.
  Use GPUI's `on_app_quit` / should-quit hooks (see Zed's `zed.rs` quit handling).

### Phase 7 — the rest (~2 wk)
Spec: USAGE "Query history", "CSV export", "Session restore"; QueryHistory.tsx, CsvExport.tsx,
CommandPalette.tsx, SettingsView.tsx, RightPanel.tsx, session.ts, history.ts, csvExport.ts.
- Query history: ⌘⌥E, right panel History tab, search, errors-only, all connections, insert /
  open in new console / copy / delete (⌘⌫), retention setting, max 5,000, same-query dedupe.
- CSV export: COPY streaming via Backend, save panel (`cx.prompt_for_new_path`), progress +
  cancel in status bar and result toolbar, savepoint rules, delimiter / header options persisted,
  reveal in Finder.
- Command palette ⌘K / ⌘P with recents (`recentCount`), fuzzy match (`format::fuzzy`).
- Settings view (⌘,; ⌘F search, Esc) with every pref and "Reset to defaults" (keeps layout).
- Session restore: tabs, SQL, filters / sorts, active tab, MRU, panel open state, reconnect
  open connections at launch (~30 s retry while unreachable); results are not re-run.
- Right panel: Structure / Info / History tabs (`rightTab` persisted).

### Phase 8 — ship (~1–1.5 wk) — **ask the user before each release-related step**
Spec: source `docs/RELEASING.md`, `scripts/release.sh`, `scripts/check-version.sh`,
`.github/workflows/{ci,release,cut-release}.yml`, `tauri.conf.json` (updater pubkey + endpoint),
`crates/boa-desktop/icons/`.
- Bundle the `.app` (Info.plist: id `dev.boa.desktop`, version, min macOS 11.0, category
  DeveloperTool, icon.icns), universal or per-arch builds, DMG, codesign + notarize + staple.
- Updater: read the same `latest.json`, verify the minisign signature with the existing
  pubkey, download, swap the `.app`, "Restart to update" pill. Or Sparkle — the user decides.
- **The bridge release:** installed Tauri builds update via the *old* updater, so the first GPUI
  release must be published as `.app.tar.gz` + `.sig` + `latest.json` exactly as
  tauri-plugin-updater expects. Rehearse end to end with a Tauri build pointed at a local/staging
  endpoint before any real publish.
- CI: fmt, clippy, tests with a Postgres service, cargo deny, both architectures.
- Migration test: copy a real config dir (+ the WebKit localStorage db) to scratch, run the
  GPUI app on it via `BOA_CONFIG_DIR`, confirm connections (with passwords), history, prefs,
  session all appear; confirm nothing is rewritten in an incompatible way (the Tauri app must
  still read the files — users may downgrade).
- README, `docs/USAGE.md` (still true), `DEVELOPMENT.md`, `RELEASING.md` updated.

---

## 10. Keyboard shortcuts (all must work; grep the source again before declaring parity)

| Keys | Action | Where (source) |
|---|---|---|
| ⌘K, ⌘P | toggle command palette | App.tsx |
| ⌘⌥E | toggle query history | App.tsx |
| ⌘O | new connection (not over a dialog) | App.tsx |
| ⌘, | toggle Settings | App.tsx |
| ⌘T, ⌥N | new console | App.tsx |
| ⌘W | close tab | App.tsx |
| ⌥1–⌥9 | activate tab n | App.tsx |
| ⌥↑ / ⌥↓ | previous / next database | App.tsx |
| ⌥B / ⌥E / ⌥J | sidebar / right panel / Database Output | App.tsx |
| ⌘Q (twice in 3 s) | quit (guarded) | lib.rs, GuardDialogs |
| ⌘↵ | arm / run statement (editor); submit edits (grid) | Workspace.tsx, DataGrid.tsx |
| Ctrl+Space | explicit completion | Workspace.tsx |
| ↑ ↓ PgUp PgDn Enter Tab Esc | completion / statement chooser | Workspace.tsx |
| ⌘⇧↵ / ⌘⇧⌫ | commit / rollback | GuardDialogs.tsx, TxControls |
| ⌘C, ⌘A, Esc | copy TSV, select all, clear selection | DataGrid.tsx |
| click, ⇧-click, ⌘-click, drag | cell / range / row selection | DataGrid.tsx |
| Enter, F2, typing | edit cell (Enter on JSON opens viewer) | DataGrid.tsx |
| ⇧↵ | value editor | DataGrid.tsx |
| Tab / ⇧Tab, Enter, Esc (in cell) | commit + move / commit / cancel | DataGrid.tsx |
| ⌘N, Alt+Insert | add row | DataGrid.tsx |
| ⌘⌫, Delete | delete row | DataGrid.tsx |
| ⌘⌥N / ⌘⌥Z | set NULL / revert | DataGrid.tsx |
| header click / ⇧-click | sort / multi-sort | DataGrid.tsx |
| ⌘↵ (value editor) | save | ValueEditor.tsx |
| Enter / Esc | apply / clear filter bar | TableFilterBar.tsx |
| ↑ ↓ PgUp PgDn Enter ⌘⌫ | history list navigate / open / delete | QueryHistory.tsx |
| ↑ ↓ Tab ⇧Tab Enter | palette | CommandPalette.tsx |
| ⌘F, Esc | Settings search | SettingsView.tsx |
| Enter | scope picker apply | ScopePicker.tsx |
| Esc | clear right-panel search | RightPanel.tsx |
| Enter | About: copy version info | AboutWindow.tsx |
| ⌘Z ⌘⇧Z ⌘X ⌘C ⌘V ⌘A | editing in every input (Edit menu too) | system |

---

## 11. Parity checklist (tick with the verification named)

Verification: **T** = automated test (boa-core unit / live, or `#[gpui::test]`), **S** = screenshot
in `docs/phaseN/`, **M** = manual check by the user (list these for them at the end of each phase).

Ticked 2026-11-26 (p10/docs-migration) on `main` at `087179f`, only where every named
verification exists; test names are fn names (`a:` = `crates/boa-app/src/`, `c:` =
`crates/boa-core/src/`); details per item in `docs/parity-audit.md` §3. All **M** items are
collected in [docs/MANUAL-CHECKS.md](docs/MANUAL-CHECKS.md) (none run yet).

- [ ] Data sources: add / edit / delete, URL paste, SSL modes, SSH tunnel (agent or key file), test — T, S, M (SSH)
  — *partial:* T `pasting_a_url_fills_the_fields`, `saving_writes_connections_json_with_the_password`, `editing_keeps_the_stored_password`, `delete_asks_with_a_second_click`, `validation_disables_test_and_save` (a:views/connection_dialog/tests.rs), c:ssh.rs, c:state/conn_url.rs; S docs/phase2/dialog-*.png. Missing: a test of the Test connection button and of the SSL choice; M1 (SSH end to end).
- [x] Password prompt; save-password toggle; passwords stay in connections.json — T
  — `no_stored_password_opens_the_prompt_and_its_password_connects`, `saving_the_password_from_the_prompt_stores_it`, `not_saving_the_password_leaves_it_out`; tests/migration.rs (passwords survive the migration and every save). M5 (real 28P01) is extra.
- [x] Environment tag + colour on explorer, editor, console toolbar, status bar — S
  — docs/phase3/production-tag.png, docs/phase4/integration/production-connection.png, docs/phase4/multiple-tabs.png (+ T `the_production_tag_and_badges_render`).
- [x] Read-only connections (server-enforced, restored if turned off) — T (live)
  — `a_read_only_connection_is_enforced_by_the_server`, `live_read_only_connections_refuse_writes`, c:pg.rs `live_postgres`.
- [x] Explorer tree with keys / FKs / indexes / checks; scope picker; row counts; views toggle; expanded state persisted — T, S
  — a:views/sidebar/tree_tests.rs, `arrow_keys_move_expand_and_collapse_as_the_source` (treeExpanded in prefs.json), `scope_picker_enter_applies_and_escape_cancels`; S docs/phase3/explorer-tree.png, scope-picker.png, context-menu.png. Open: context menus have no tests (audit P1-9).
- [x] DDL view (`pg_get_*def`), opens in a new console — T
  — c:pg/catalog.rs `table_ddl`, `view_and_matview_ddl`; c:state/ddl.rs `opens_the_ddl_in_a_new_console_on_that_database`; a:views/table_edits/tests.rs `the_ddl_button_shows_for_known_tables_and_opens_their_ddl` (toolbar → new console). The explorer's "Show DDL" shares `ddl_action` but has no view test.
- [x] Console: highlighting, completion (incl. FK join conditions, icons, Tab), ⌘↵ arm/run, statement chooser — T, S
  — a:editor/tests.rs (`cmd_enter_arms_then_runs_the_statement_under_the_caret`, `the_statement_chooser_offers_the_subquery`, `typing_opens_completion_and_tab_replaces_the_whole_word`, `ctrl_space_offers_foreign_key_joins_and_stays_open_while_erasing`); S docs/phase4/console-results.png, statement-chooser.png, completion-popup.png (icons: S only).
- [x] Paged results with column types; cancel; run progress; Output log (⌥J); result tabs — T, S
  — `live_run_shows_the_result_in_the_database_output`, `live_cancel_stops_a_slow_statement`, `live_paging_fetches_the_next_page`, `option_b_e_j_toggle_the_panels_from_the_editor`; S docs/phase4/run-progress.png, console-output-log.png.
- [x] Grid selection (cell, range, rows, ⌘A, Esc), ⌘C TSV, Copy as (TSV ± header, CSV, Markdown, JSON, INSERT, UPDATE, IN list) — T
  — a:grid/tests.rs `click_shift_click_and_copy_tsv`, `keyboard_selection_cmd_a_and_esc`, `every_copy_as_format_via_its_menu_action`; c:grid/copy_as.rs.
- [x] JSON viewer; value editor (⇧↵) — T, S
  — `json_viewer_opens_on_enter_and_double_click_and_closes`, `value_editor_saves_with_cmd_enter`; S docs/phase4/json-viewer.png, value-editor.png.
- [x] Table data editing for PK tables; read-only reasons for others; preview + transactional submit; pending-changes prompts — T (live), S
  — a:views/table_edits/tests.rs `live_edit_preview_submit_changes_the_row`, `live_add_row_with_defaults_delete_and_set_null`, `pending_edits_ask_before_paging_and_closing`, `read_only_reasons_follow_the_catalog`, `new_rows_show_default_only_in_columns_with_a_default` (p9/fixes1); S docs/phase5/staged-edits.png, sql-preview.png, submit-error.png.
- [x] WHERE / ORDER BY bar, header sort (⇧ multi), one-clause validation, server errors in place — T (live)
  — `live_filter_bar_applies_where_and_shows_server_errors_in_place`, `live_header_clicks_sort_on_the_server_shift_adds_columns`, `clauses_that_escape_their_place_never_run`; c:grid/table_query.rs.
- [x] Tx Auto / Manual, state probe, commit / rollback shortcuts, typed begin / commit / rollback — T (live)
  — a:models/transactions/tests.rs `live_manual_mode_commit_and_rollback`, `live_typed_begin_commit_and_rollback`, `shortcuts_apply_only_with_a_transaction_open`. M11 (real keyboard) is extra.
- [x] Destructive-statement confirmations; setting; Production always confirms — T
  — `destructive_statements_ask_first_unless_turned_off_production_always`, `live_destructive_confirmation`; c:sql/guard.rs, c:state/guards.rs.
- [x] Query history: per connection on disk, retention, max 5,000, same-query dedupe, search, errors-only, all connections — T
  — a:models/history/tests.rs `writes_the_tauri_apps_jsonl_format`, `retention_drops_old_runs_and_follows_the_setting`, `keeps_at_most_five_thousand_per_connection`, `a_query_run_again_replaces_its_entry`; a:views/query_history/tests.rs `all_connections_merge_newest_first`; c:state/history.rs.
- [x] CSV export: COPY streaming, temp + rename, progress / cancel, savepoint rules, delimiter / header options — T (live)
  — a:models/exports/tests.rs `exports_the_whole_orders_table`, `cancel_leaves_no_file`, `inside_a_transaction_a_savepoint_keeps_it_intact`, `options_are_read_from_and_saved_to_csv_export_json`; export vs console on one session fixed and tested in c:pg/concurrency_tests.rs (p9/core-conc). The native save panel and Reveal in Finder: M19.
- [x] Session restore: tabs, SQL, filters / sorts, active tab, layout; reconnect at launch (retries ~2 min, `RETRY_DELAYS_MS`; this line said ~30 s, the source code also retries ~2 min) — T
  — a:models/session/tests.rs `the_session_is_saved_and_restored_through_the_app`, `reconnects_retrying_while_the_server_is_unreachable`, `gives_up_after_the_retries_and_keeps_the_connection_wanted`; a:tests/migration.rs (a Tauri session restored).
- [ ] Staying connected: keepalive detection, safe auto-retry only, tx-lost and commit-unknown messages, backoff, wake retry, one toast per connection — T (kill backend sessions with `pg_terminate_backend`; stop/start Postgres), M (sleep/wake)
  — *partial:* T `a_terminated_session_reconnects`, `tx_lost_and_commit_unknown_messages`, `a_wake_up_retries_a_failed_wanted_connection_right_away`, `a_failed_connect_shows_one_status_toast_per_connection`, c:pg.rs `only_rerunnable_statements_are_retried`, `live_dropped_session`. Missing: a stop/start-Postgres test, keepalive detection; M3, M4.
- [ ] Quit / disconnect guard with open transactions; double ⌘Q escape hatch — T, M
  — *partial:* T `quitting_with_an_open_transaction_asks_and_a_second_cmd_q_quits`, `live_disconnect_with_an_open_transaction_asks_and_rolls_back`; S docs/phase6/quit-guard.png. Missing: M6–M10; Dock quit / logout not held (audit P0-2).
- [x] Command palette ⌘K with recents; Settings (all prefs, search, reset); About (copy version info) — T, S
  — `cmd_k_and_cmd_p_toggle_the_palette_and_esc_closes_it`, `recents_come_first_newest_first_up_to_recent_count`, `every_switch_and_choice_writes_prefs_json`, `reset_to_defaults_keeps_the_layout_in_prefs_json`, `search_filters_the_rows_and_shows_general`, `enter_in_about_copies_the_version_info`; S docs/phase7/palette*.png, settings-*.png, about.png.
- [ ] Updates: check, download, "Restart to update"; bridge release from the Tauri updater rehearsed — M (with the user)
  — *partial:* updater module + local GPUI→GPUI and Tauri→GPUI rehearsals with a throwaway key (docs/RELEASING.md "Rehearsals"); "Restart to update" not wired yet (p10/updates-wire); M21, M22.
- [ ] Every shortcut in §10 — T where possible, M list for the rest
  — *partial:* T `every_global_shortcut_of_app_tsx_is_bound_in_workspace`, `edit_menu_actions_reach_the_focused_editor`, `cmd_t_cmd_w_and_option_digits_drive_the_tabs`, `each_editing_shortcut_stages_its_change`; overlay gating (P1-1), ⌘W window close (P1-3) and Ctrl grid keys in p10/keys; M list: M11–M14.
- [x] Prefs / session / CSV options in JSON files; localStorage import (if approved) — T
  — c:state/prefs.rs, c:state/local_storage.rs, `the_local_storage_import_runs_once`, `options_json_is_what_the_tauri_app_wrote`, `layout_is_saved_to_the_config_dir_and_read_back`; a:tests/migration.rs `a_tauri_install_carries_over_and_its_files_stay_readable_by_the_tauri_app` (UTF-16LE localStorage → the three files, byte for byte).
- [ ] Look matches `docs/images/boa.png` and the Tauri app side by side — S, M
  — *partial:* side-by-side captures docs/audit/ vs docs/audit/src/ with the difference list in parity-audit.md §4 (p10/visual working on it); M17, M18.
- [ ] Migration from a real Tauri config dir; Tauri app still reads the files after — T (on a copy), M
  — *T done:* a:tests/migration.rs `a_tauri_install_carries_over_and_its_files_stay_readable_by_the_tauri_app` (synthetic Tauri install → app init → every file the app writes checked with the Tauri rules; with `BOA_TAURI_UI_SRC` the source's own prefs.ts / session.ts / csvExport.ts parse them losslessly, run 2026-11-26) and `real_install_copy` (#[ignore]) run on a copy of the user's install (§13). Missing: M20 (the Tauri app opened on GPUI-written files).
- [ ] Release: signed, notarized, universal/both archs, macOS 11 — M (with the user)
  — not started: needs the Developer ID and the updater key (RELEASING.md); M23.
- [ ] Performance: startup ≤ Tauri, idle memory ≤ Tauri, 5,000-row page scroll at display rate, typing latency in a 2,000-line console — T/bench, numbers in docs
  — *partial:* docs/perf.md: idle memory 65 vs 83 MB ✓, 5,000-row scroll at 120 Hz ✓, typing ~1.3 ms + ~1.8 ms frame ✓; startup ~207 ms window-on-screen vs Tauri ~165 ms ✗ (a painted-UI comparison not done).

---

## 12. Ask the user (don't decide alone)

- Granting Screen Recording / Accessibility to the agent's terminal (unblocks real UI checks).
- localStorage → JSON import: wanted? (feasible, see §4).
- Updater: own minisign-based updater reading `latest.json` vs. Sparkle; any change to the
  release pipeline, signing identity, notarization credentials, GitHub releases.
- Any `unsafe` (sleep/wake or network-change notifications) or a new platform dependency.
- Dropping or changing any feature, or any on-disk format change.
- Bumping gpui / gpui-component.

---

## 13. Progress log

| Date | Source commit | Phase | Notes |
|---|---|---|---|
| 2026-11-26 | 61984bb | 0 done | Go approved by the user. See docs/phase0/. |
| 2026-11-26 | 61984bb | §12 answers | Screen Recording + Accessibility granted; localStorage import: yes; isolated unsafe for wake/network: yes; Phase 8: prep only, ask before signing / notarizing / publishing. |
| 2026-11-26 | 61984bb | 1 done | Shell (prefs, layout, toasts, actions/keymap, panels, menus, About), boa-core state ports (session, csv export, tx, guards, localStorage import, history, ddl, updates, conn URL, reconnect), CI + cargo-deny + fixtures. docs/phase1/. |
| 2026-11-26 | 61984bb | 2–4 merged | Connections (dialog, password prompt, reconnect loop, wake/network monitor), explorer (Catalogs, tree, scope picker, context menus, right panel Structure/Info), consoles (workspaces, tabs, arm-then-run, chooser, own completion popup, Output log), grid as pure view (copy-as, JSON viewer, value editor, edit mechanics). docs/phase2–4/. Integration pass running. |
| 2026-11-26 | 61984bb | 5–7 merged | p5/table-edits, p6/tx-guards, p7/history, p7/csv-export, p7/session (Session model, debounced save, reconnect at launch, one-time localStorage import), p7/palette-settings, p4/integrate (spike.rs removed, live status bar, BOA_DEV_INPUT). docs/phase5–7/. |
| 2026-11-26 | 61984bb | 8 prep + p9 merged | p9/fixes1 (new-row defaults, table toolbar row filter + DDL, italic Geist, chevron focus, status-bar fallback, export cleanup on quit); p8/release-prep (scripts/bundle.sh, Info.plist + icon, updater module with minisign verify / swap / relaunch, local update + bridge rehearsals with a throwaway key, manual release.yml, docs/RELEASING.md; nothing signed or published); p9/core-conc (pg/ops.rs: one operation at a time per session, per-operation CancelHandle; fixes the export-vs-console cursor race, also present in the Tauri app; docs/perf.md); p9/audit (docs/parity-audit.md, docs/audit/). |
| 2026-11-26 | 61984bb | p10 wave | Four agents: p10/keys (overlay shortcut gating, ⌘W, tunnels at quit, Ctrl grid keys), p10/updates-wire (Restart to update), p10/visual (parity-audit §4), p10/docs-migration (below). |
| 2026-11-26 | 61984bb | p10/docs-migration | Migration test a:tests/migration.rs: synthetic Tauri install (connections.json with passwords + SSH, history jsonl, WebKit localStorage UTF-16LE) → import + AppState::init + main window; everything appears; every file the app writes keeps the Tauri format (unchanged connection save byte-identical, Tauri history lines untouched, prefs/session/csv follow the TS rules; with BOA_TAURI_UI_SRC the source's own TS parsers read them losslessly via node). Rehearsal on a copy of the user's real install (originals checksummed unchanged; counts only): 3 connections, 3 with passwords, 0 SSH; history 38 lines, 0 unreadable, 38 shown; localStorage import wrote prefs.json + session.json (no boa.csvExport key: defaults); session 1 database, 1 tab (console with SQL), 2 connections to reconnect; unchanged saves of every connection leave connections.json byte-identical; prefs/session/csv pass the TS parsers losslessly; the app (hosts neutralised) restores explorer, panels, History tab and database chip (screenshot kept out of the repo: real data). Found: the user's real config dir already has GPUI-written prefs.json / session.json / .localstorage-imported (12:42 today), so a GPUI build ran on it; prefs.json is written 0644 (`Prefs::save`, c:state/prefs.rs), unlike the other config files (0600). Docs: README.md, docs/USAGE.md (ported, ~2 min retry, differences + known gaps), docs/MANUAL-CHECKS.md (M1–M23), boa-core README (state, pg::ops, CancelHandle), DEVELOPMENT.md; §11 ticked (18 of 27). |
| 2026-11-26 | 61984bb | p10 + p11 merged | All of wave 4 merged (keys, updater wiring with guarded Restart to update, visual polish incl. per-line statement outline, migration + docs); prefs.json now written 0600. p11/behaviour (⌘K/⌘⌥E/⌘, over dialogs, Copy as disable rules + reasons, menu key hints, full-screen label, About Enter once) and p11/rem-polish (explicit px in the remaining views, shared dialog surface, 16 px checkboxes). User commits on main kept: title bar "+" sizing, chrome::menu_row (d311052), BOA_DEV_BACKGROUND + CLAUDE.md (0a9e484), DataGrip outline commits (superseded by the source-shape outline at the user's choice). 9 §11 items remain open: each needs a manual check (docs/MANUAL-CHECKS.md), a user decision (Dock/logout quit guard needs an isolated unsafe NSApplication delegate; release signing / notarizing / version 0.7.0), or misses its target (startup ~207 ms vs Tauri ~165 ms, docs/perf.md). |
| | | | |
