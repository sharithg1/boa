# Phase 4 integration pass (after waves 1–2)

Branch `p4/integrate`. The real app was run end to end (`--features screenshot`,
`BOA_CONFIG_DIR` in a scratch dir, `BOA_DEV_CONNECTION=1`) against the local
Postgres 17 (`boa_dev`, `boa_dev2`), driven through the real keymap and hit
testing, and checked on rendered frames and real-window captures
(`screencapture -l`).

**How it was driven.** The agent's shell has Screen Recording but **not
Accessibility**: `osascript` System Events keystrokes fail ("osascript is not
allowed to send keystrokes", 1002) and posted CGEvents are dropped. So a dev hook,
`views/dev_input.rs` (`BOA_DEV_INPUT`), feeds keys and clicks to
`Window::dispatch_keystroke` / `dispatch_event` from a command file (same keymap,
key contexts, focus and hit testing as real input; only the NSEvent layer is
skipped), and renders frames to PNG without bringing the window forward. The
screenshots below are real `screencapture -l` captures (traffic lights included).
To check the native layer by hand, grant Accessibility to the terminal.

## Screenshots (`docs/phase4/integration/`)

| File | What |
|---|---|
| `connected-tree-table-tab.png` | Connected "Local dev", tree expanded to columns and keys, `customers` open as a preview table tab, right panel Structure, status bar with latency |
| `console-results.png` | A console with a multi-line join, armed outline, results (`orders` int8 / `total` numeric) in Database Output, caret `4:14` |
| `production-connection.png` | The connection set to Production in the dialog: red server icon + `PROD` in the explorer, red database icon in the title chip, red stripe over the editor, `PROD` in the console toolbar and the status bar |
| `two-databases.png` | `boa_dev2` added through the scope picker ("2 of 3"), selected with ⌥↓, its tree and a console on it (`products`) |

Compared with `dbclient/docs/images/boa.png`: same layout, chrome, colours, tags
and stripe; differences are listed under "Still broken / different".

## What works end to end

- **Startup**: an empty config dir shows "No data sources" + "Add PostgreSQL"
  (Sidebar.tsx); saved connections are listed in `connectionOrder` (checked with
  three connections and order `["c","a"]` → Charlie, Alpha, Bravo); nothing
  connects at launch; `session.json` panel flags are honoured; `session.json`,
  `connections.json` and `history-<id>.jsonl` are not rewritten by just running.
- **Connections**: connect by expanding a connection, from its context menu and
  from the status bar popover; disconnect from the context menu and the popover;
  running a statement on a disconnected connection connects first; edit (Edit… →
  Environment: Production → Save) reconnects with the new settings; ⌘O opens the
  new-data-source dialog.
- **Explorer**: database › schema › tables › table › columns / keys / indexes,
  PK / unique key icons, row counts; scope picker on the connection ("N of M",
  adds `boa_dev2`); expanded state persisted and restored (loaded at startup once
  connected).
- **Tables**: click → preview tab (replaces the previous preview), double-click →
  pinned; table tab loads 500 rows with types, `~5,000 total`, Structure panel,
  status bar `~5,000 rows`; Show DDL opens the `CREATE TABLE` in a new console.
- **Consoles**: ⌘T / ⌥N / explorer button open `console_N`; ⌘↵ arms (outline) then
  runs; results in Database Output with column types; `rowLimit` 500 paging over
  `public.orders` (1–500 …, 501–1,000 …); `boa_fx.slow(10)` shows "Executing… n s"
  and the cancel button stops it (`57014`); errors show in the editor's error
  panel and the Output log with the `LINE 1:` excerpt and caret under the
  position; completion lists the tables of the console's own database (from
  Catalogs: `customers` in boa_dev, `products` in boa_dev2).
- **Shortcuts**: ⌥1–9, ⌘T, ⌘W (also from the explorer), ⌥↑ / ⌥↓ across boa_dev and
  boa_dev2, ⌥B / ⌥E / ⌥J, ⌘O, Esc in dialogs.
- **Environment colour**: Production shows on the explorer, title chip, editor
  stripe, console toolbar and status bar (screenshot).
- **Status bar**: breadcrumbs, caret (consoles) / row estimate (tables), UTF-8,
  connection with state badge, tag and `connMeta` (latency, else the state), the
  connection popover.

## What was fixed (with tests)

1. **`spike.rs` removed** (`lib.rs`, `app.rs`, README). Nothing referenced it but
   app start-up; `ActiveDb` (views/mod.rs) already read Connections / Workspaces,
   and the editor's completion schemas already came from Catalogs for the
   console's `DbKey` (verified live, see above). views/mod.rs `connection_tag` /
   `connection_tint` now delegate to `views::conn_tag` (there were three copies).
2. **The `dev-hooks` / `screenshot` build was broken** by the wave-2 merges
   (`Catalog` gained `names_loaded` / `loaded` / `force_pending`, `Tab` gained
   `content`): `views/workspace/dev.rs`, `views/sidebar/dev.rs`. CI only builds
   default features, so nobody noticed. *Test:* `cargo clippy -p boa-app
   --all-targets --features screenshot -- -D warnings` (now clean) — **the lead
   should add this to CI**.
3. **Global shortcuts died when the focused view went away** (views/root.rs). A
   closed dialog opened from a context menu gives the focus back to the menu,
   which no longer exists; hiding the explorer with ⌥B while it had the focus did
   the same. With nothing focused no key reaches the `Workspace` context: ⌘T,
   ⌘W, ⌥1…, even ⌥B to bring the explorer back, did nothing. Found live (Edit… →
   Esc → ⌘T ignored). RootView now refocuses the active tab (or Settings) on
   `on_focus_lost`. *Test:* `views::root::tests::the_shortcuts_survive_the_focused_view_going_away`
   (fails without the fix).
4. **Deleting a connection left its tabs open** and the title bar on its database:
   `ConnectionsEvent::Deleted` had no subscriber. `Workspaces::forget_connection`
   (store.ts `deleteConnection`) is wired in `AppState::init`. *Test:*
   `models::workspaces::tests::deleting_a_connection_closes_its_tabs` (real backend,
   fails without the wiring).
5. **Status bar connection** (views/status_bar.rs) was a placeholder: now the
   `ConnectionIcon` with state badge, name, tag, `connMeta` ("8 ms" / "offline" /
   "connecting" …) and StatusBar.tsx's popover (`name · state`, `PostgreSQL ·
   version · latency`, URL, Connect / Disconnect / "Reconnecting… Retry now").
   Removed the console "last run" time the source doesn't show there. *Tests:*
   `conn_meta_is_the_latency_or_the_state`, `popover_button_follows_the_state`.

## Still broken / different (drives the next waves)

Owned by agents running now (not touched here):

- **Table tabs aren't editable yet** (p5/table-edits): double-click / Enter / typing
  on a `customers` cell only selects it; no WHERE / ORDER BY bar; no read-only
  reason shown for PK-less tables. `models/table_tab.rs`, `views/workspace.rs`
  table area.
- **Table tabs show two pagers**: the tab toolbar's (`views/workspace.rs:597`) and
  the grid's own Phase 0 status line "Results · n ms · pager"
  (`grid/render.rs:573`, drawn for table grids). The source has only the toolbar.
- **Tx: Auto / Manual, commit / rollback** (p6): the toolbar chip is static;
  ⌘⇧↵ / ⌘⇧⌫, Add row, Submit edits are `not_yet` stubs (`views/root.rs:329-332`).
- **Command palette** ⌘K / ⌘P, the title-bar database chip and "Open table…"
  (`views/root.rs:272`), **query history** ⌘⌥E (`views/root.rs:276`) and the
  toolbar history button, **CSV export** (`views/sidebar.rs:687`) are `not_yet`
  toasts (Phase 7). Session restore (tabs, `wanted` connections) is Phase 7.

Gaps found here, not fixed:

- **Preview tabs aren't italic**: `views/title_bar.rs:285` sets `.italic()` but no
  italic Geist Mono is bundled (`assets/fonts`) and GPUI doesn't synthesize
  oblique, so a preview tab looks pinned. Needs GeistMono-Italic TTFs (OFL) or a
  different preview cue (ask the user).
- **Explorer chevron click focuses the row** (`views/sidebar.rs:762`); in the
  source the chevron only toggles (the row's focus highlight stays where it was).
  Cosmetic.
- **Status bar fallback connection** (no database selected) is the first in the
  prefs-sorted list (`views/status_bar.rs:91`, `Connections::sort_saved`); the
  source takes the first in `connections.json` order. Cosmetic.
- **Latency is measured once per connect** (as in the source); it doesn't refresh.
- **Background windows draw nothing**: GPUI draws no frames while the window is
  fully occluded (the first table paint was logged 8.7 s "after the content
  arrived" while the window was hidden). Not visible to users; it is why
  `BOA_DEV_INPUT` draws before each command.
- **Not verified live**: the native menu bar / Edit menu, traffic-light position
  side by side with Tauri, trackpad scrolling, drag-reordering connections and
  tabs, the password prompt (covered by Phase 2 tests), SSH tunnels, reconnect
  after a real network loss / sleep (Phase 2 tests only).
