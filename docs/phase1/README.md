# Phase 1: core shell

Branch `p1/shell`. Spec: source `docs/USAGE.md` "Layout", App.tsx (`useShortcuts`),
TitleBar / Sidebar / RightPanel / BottomPanel / StatusBar / PanelResizeHandle /
Toasts / SettingsView / AboutWindow / chrome.tsx, `state/prefs.ts`, main.css,
updater.rs (menu), lib.rs (window). Architecture contract: `crates/boa-app/README.md`.

## Built

- **boa-core `state::prefs`**: port of state/prefs.ts. Same JSON as `boa.prefs.v2`
  (camelCase, field order, `'*'` scopes, `Record<string, true>`), defaults, clamps
  (sidebar 200–480, right 280–800, bottom 120–min(900, window − 200)), `applyOrder`,
  reset keeping the layout, lenient load like the TS (wrong-typed field → default,
  bad `rightTab` / negative `historyDays` → default, unknown fields kept),
  `load(dir)` / `save(dir)` of `prefs.json` (temp + rename; missing/corrupt → defaults).
- **Config dir**: `BOA_CONFIG_DIR` overrides it for the whole app (`app_state::config_dir` →
  `Backend::with_config_dir` + models).
- **Models**: `Prefs` (debounced save, `PrefsChanged` events, flush on quit), `Layout`
  (panels + Settings; panel flags persisted in session.json), `Toasts` (keyed
  replace keeping the id, TTL 6 s, max 5, action button, kinds); documented stubs for
  Connections, Catalogs, Workspaces, Console, TableTab, Transactions, History, Exports,
  Session, Updates; `AppState` global.
- **Actions / keymap**: every §10 action declared (`src/actions.rs`), all bindings in
  `src/keymap.rs` with per-view contexts and an audit of gpui-component's bindings;
  the ⌥ rule (see below). Stubs toast "… isn't available yet".
- **Views**: root (App.tsx layout), title bar (database selector, placeholder tab
  strip with close / middle-click close / overflow scroll / "+", "…", panel toggles),
  sidebar frame (header, toolbar, placeholder tree, Settings footer), right panel
  frame (Structure / Info / History pills, `rightTab` persisted, source empty states),
  bottom panel frame (Database Output header, Output tab + one result tab holding the
  Phase 0 grid, hide), status bar (⌥J toggle, breadcrumbs, UTF-8, connection),
  resize handles (drag with window-level listeners, double-click resets), toasts,
  Settings (all General prefs wired: switches, choice menus, search, ⌘F, Esc, Saved,
  Reset; Connections; Keyboard), About window (copy version info on Enter).
- **Window chrome**: transparent title bar, traffic lights at (14, 16), the app moves
  the window from its own title rows (`app_owns_titlebar_drag` + `WindowDrag`);
  double click zooms.
- **Menu bar**: Boa (About Boa, Check for Updates… [stub], Settings… ⌘,, Services,
  Hide Boa ⌘H, Hide Others ⌥⌘H, Show All, Quit Boa ⌘Q), File (Close Window), Edit
  (Undo, Redo, Cut, Copy, Paste, Select All → gpui-component input actions, so they
  reach the focused Input / Editor; Copy / Select All also reach the grid), View
  (Enter Full Screen ⌃⌘F), Window (Minimize ⌘M, Zoom, Close Window), Help.
- **Dev hooks** behind feature `dev-hooks` (implied by `screenshot`): grid/dev.rs,
  `BOA_GRID_PAGE_SIZE`, `BOA_EDITOR_*`, and new `BOA_DEV_PANELS / _SETTINGS / _TOAST / _ABOUT`.

### The ⌥ rule

GPUI matches `alt-n` on the key and reports the composed character in
`Keystroke::key_char`. A keystroke interceptor remembers the keystroke in flight; ⌥
handlers call `keymap::option_shortcut_applies(cx)` and, when ⌥+key composed a
printable ASCII character (German ⌥N = "~"), call `cx.propagate()` — the key is then
unhandled and the mac backend gives it to the input system, which types the
character. Tested with synthetic keystrokes (`option_keys_that_compose_text_are_left_to_the_editor`);
not tried with a real German layout.

## Screenshots

Rendered with `--features screenshot` on a temp `BOA_CONFIG_DIR` (no native parts),
plus two `screencapture -l` shots of real windows (Screen Recording works now).

| File | What |
|---|---|
| `main-default.png` | default layout at 1280×820 (all panels open, as store.ts opens the right panel from 1100 px) |
| `main-result.png` | a result in the Database Output panel |
| `main-panels-closed.png` | ⌥B / ⌥E / ⌥J closed: traffic-light inset + toggles move into the title bar |
| `toasts.png` | success, error with action, info toasts |
| `settings-general.png`, `settings-keyboard.png` | Settings |
| `native-window.png` | the real window (traffic lights, title row) |
| `about-window.png` | the real About window |

### Compared with the source's `docs/images/boa.png`

Same structure, colours, type scale and row heights (sidebar header 40, toolbar 32,
title bar 40, Database Output header 36, status bar 32). Differences, all
later-phase content rather than chrome:

- The explorer tree is a placeholder row (Phase 3); no env tag / "1 of 2" chips.
- The console toolbar is Phase 0's ("Run ⌘↵", catalog counts) instead of run / stop /
  history, "Tx: Auto", the env tag and the schema picker (Phase 4 / 6).
- Title-bar tabs are a placeholder list; the database selector shows the spike's database.
- The result grid keeps its own Phase 0 status strip ("Results · n ms" and a row-count
  pill) under the Database Output header; the source puts source / timing / export /
  pager in the header and shows "200 rows" as a toast (Phase 4).
- Status bar: no caret position, env tag or latency yet; connection name + state dot only.
- boa.png has the right panel closed (that capture's window was under 1100 px); ours
  opens it at 1280 px, as the source does.

## Tests

- boa-core: 31 prefs tests (`state::prefs::tests`).
- boa-app (`src/tests.rs` and module tests, 20 new): ⌥B/⌥E/⌥J from the editor, ⌥ keys
  that compose text go to the editor, handle drags clamp (sidebar / right / bottom,
  double-click reset), prefs.json + session.json written to a temp config dir and read
  by a fresh app, session.json keeps its other keys, toast replace-by-key / TTL / cap,
  toasts + "not yet" stubs in the window, ⌘, toggles Settings, ⌘F + Esc in search,
  Esc closes and refocuses the editor, reset keeps the layout, Enter in About copies
  the version info, Edit menu actions reach the editor, menu structure, keymap
  coverage of App.tsx shortcuts, ⌥ rule unit cases, About helpers, icon assets.

## Gaps / not verified

*Later status (2026-11-26):* the "+" / "…" menus, tab drag-reorder and pin, the
localStorage import (p7/session) and the Settings choice menus
(`every_switch_and_choice_writes_prefs_json`) were done in later phases; see
`docs/parity-audit.md` ("Since fixed"). The menu bar and window drag remain
manual checks (`docs/MANUAL-CHECKS.md` M13, M17).

- The menu bar was not seen on screen (the app wasn't frontmost; I didn't take focus
  from the user's session). Menu structure is unit-tested; Edit actions tested via dispatch.
- Window dragging from the title rows and double-click zoom: implemented, not tried by
  hand (the test platform can't move windows).
- Tab drag-reorder and pin (double click) are not in the placeholder strip (Phase 4).
- "+" and "…" open no menus yet ("+" adds a placeholder console; "…" toasts "not yet").
- One-time localStorage import: not done (needs the user's OK; `state/local_storage.rs`
  belongs to another agent). `Prefs::from_json` is ready for it.
- Settings choice menus (gpui-component dropdown) not exercised in tests.
