# boa-app

Boa's GPUI app: the main window, its models and views, the menu bar and the
keymap. The backend and all pure logic live in `boa-core`; this crate only
holds state as GPUI entities and draws it.

## Layout

```
src/
  main.rs          calls app::run()
  app.rs           app init, app-wide actions (menu bar), main window options, screenshot hook
  app_state.rs     AppState global (backend + app-wide models), config dir (BOA_CONFIG_DIR)
  actions.rs       EVERY action (HANDOFF-PARITY.md §10), documented with its keys and handler
  keymap.rs        EVERY key binding, key contexts, the ⌥ rule
  models/          GPUI entities, no views (see the table in models/mod.rs)
  views/           one module per React component (App.tsx → root.rs, TitleBar.tsx → title_bar.rs, …)
  editor/, grid/   SQL editor and result grid (pure views; live state comes from the models)
  theme.rs         main.css palette + type scale, pushed into gpui-component's Theme
  assets.rs        Geist TTFs, app icon, Lucide icons gpui-component doesn't bundle
  tests.rs         #[gpui::test]s of the shell (open_main(), TempDir helpers)
```

## The contract later phases build on

### State: models + `AppState`

- App-wide state is a set of entities in [`AppState`](src/app_state.rs), a GPUI
  global created once at startup (`AppState::init(config_dir, window_width, cx)`):
  `backend` (boa-core `Backend` on the config dir), `prefs`, `layout`, `toasts`,
  `connections`, `catalogs`, `workspaces`, `transactions`, `history`, `exports`,
  `session`, `updates`. Views get them with `AppState::global(cx).prefs.clone()` and
  `cx.observe(&entity, |_, _, cx| cx.notify())`.
- Per-tab state (`Console`, `TableTab`) will be entities owned by `Workspaces`.
- Models never hold views. A model that persists writes its own file in
  `AppState::config_dir` with a debounced save (250 ms) and flushes in
  `cx.on_app_quit` (see `models/prefs.rs`, `models/layout.rs`).
- Phase 1 models: **Prefs** (prefs.json, `PrefsChanged { prev }` event, `update(cx, |p| …)`,
  `reset(cx)`), **Layout** (sidebar / right / bottom open + Settings page;
  `LayoutEvent`; persists the three panel flags into session.json), **Toasts**
  (`push(NewToast::error("…").key("conn:1").body("…").action("Retry", |w, cx| …), cx)`,
  `push_for(.., ttl, cx)`, `dismiss`, `dismiss_key`, `report_error`). The others
  are documented stubs with the intended types and a few method signatures.

### Files in the config dir

| File | Owner | Format |
|---|---|---|
| `connections.json`, `history-<id>.jsonl` | boa-core | unchanged from the Tauri app |
| `prefs.json` | `models::prefs::Prefs` via `boa_core::state::prefs` | exactly `boa.prefs.v2` (camelCase, same fields; unknown fields kept) |
| `session.json` | `models::session::Session` (the only writer; Layout's panel flags included) | exactly `boa.session.v1` (`JSON.stringify(pick(state))`: same keys, same order, no others) |
| `.localstorage-imported` | `models::session::import_local_storage` at startup | marker: the one-time localStorage import ran |

`BOA_CONFIG_DIR=<dir>` replaces the config dir for the whole app (backend and
models). **Every dev run, screenshot and test must use it**; the default is the
user's real `~/Library/Application Support/dev.boa.desktop/`.

### Actions and keys

- Add an action to [`actions.rs`](src/actions.rs) (namespace `boa`), its binding to
  [`keymap.rs`](src/keymap.rs) in the right context, and its handler on the view
  that owns the context (`.on_action(cx.listener(…))`). App-wide ones (menu bar)
  are `cx.on_action` in `app::register_app_actions`. Stubs call
  `views::not_yet("What", cx)` (a keyed info toast).
- Key contexts: `Workspace` (main window root; the global shortcuts), `SqlEditor`,
  `ResultGrid` / `GridCellEditor`, `ValueEditor`, `FilterBar`, `Sidebar`,
  `RightPanel`, `Palette`, `QueryHistory`, `StatementChooser`, `ScopePicker`,
  `Settings`, `About`. To override a key gpui-component's `Input` binds, bind it
  in `<YourContext> > Input` (same depth, bound later, wins). The audit of
  gpui-component's own bindings is in keymap.rs's docs.
- **⌥ rule** (App.tsx `isOptionShortcut`): a ⌥ handler must start with
  `if !keymap::option_shortcut_applies(cx) { cx.propagate(); return; }` (the
  root view's `pass_through` does it). When ⌥+key composed a printable ASCII
  character (German ⌥L = "@"), the key then reaches the focused input as text.
- While Settings is open the workspace shortcuts pass through (App.tsx returns
  early); ⌘K / ⌘⌥E close Settings first; ⌘, toggles it; ⌘O works over it.
- Buttons call models directly (or `window.dispatch_action` for actions that
  have no ⌥ form); don't route mouse clicks through ⌥ actions.

### Views and window chrome

- Every window's root is gpui-component's `Root` (dialogs, popovers, menus and
  notifications need it, tests too). The main window is `Root(RootView)`
  (`app::open_main_window`), the About window `Root(AboutView)`.
- The main window has a transparent title bar (traffic lights at GPUI (14, 16)
  = Tauri's (14, 22)) and `app_owns_titlebar_drag`: rows drawn under it (title
  bar, sidebar header, right panel header, Settings tops, About top) move the
  window through `views::chrome::WindowDrag`; buttons on them stop the press.
- Panel sizes come from the prefs (clamped by boa-core); `views::panel_resize` +
  the root view do the dragging with a window-level canvas.
- Shared bits: `views::chrome` (`icon_button`, `status_dot`, `kbd`, `tooltip`,
  `USED_ICONS` — add new icons there; non-bundled Lucide SVGs go in `assets/icons/`).

## Platform (`src/platform/`)

The only code allowed `unsafe` (scoped `#[allow(unsafe_code)]`, approved by
the user per use): `macos.rs` watches wake-ups and network changes for the
reconnect loop; `terminate.rs` holds quits that bypass ⌘Q (Dock → Quit,
logout, restart). GPUI 0.3.14's app delegate has no
`applicationShouldTerminate:`, so `terminate::intercept` adds one to its
class at startup (`app::run`) and routes it to `app::terminate_handler`:
Now with nothing open / once quitting / when asked within 3 s, else Cancel
and the ⌘Q prompt. The answer comes from a `QuitGuard` of `Cell`s (the hook
never borrows the `App`); `quit_now` marks it quitting, so the confirmed
quit's own terminate goes through. Tests call `terminate_handler`'s function
directly; GPUI is pinned with `=`, so re-check `terminate.rs` when bumping it
(it refuses to hook a delegate that already answers).

## Features and dev hooks

- `dev-hooks`: env-driven hooks (`BOA_GRID_*` in grid/dev.rs, `BOA_EDITOR_TEXT` /
  `BOA_EDITOR_TYPE`, `BOA_DEV_PANELS` / `BOA_DEV_SETTINGS` / `BOA_DEV_TOAST` /
  `BOA_DEV_ABOUT` in views/dev.rs). Off by default: release builds read none of them.
- `screenshot` (implies `dev-hooks`): `BOA_SCREENSHOT=/abs/out.png` renders the main
  window to a PNG and quits.
- `BOA_DEV_INPUT=/abs/commands.txt` (`dev-hooks`, `views/dev_input.rs`): drives the
  running window from a file, one command per appended line (`key cmd-t`, `type …`,
  `click X Y [2] [cmd,shift]`, `rclick X Y`, `wait MS`, `shot /abs/out.png`), through
  the real keymap and hit testing. For end-to-end checks when the shell has no
  Accessibility permission (osascript keystrokes / CGEvents are dropped) and without
  stealing the user's focus; `shot` draws a fresh frame even while the window is
  hidden behind others (an occluded window draws none by itself).
- `BOA_DEV_BACKGROUND=1` (`dev-hooks`, `app::background`): the app never takes the
  focus or shows a window (accessory app, no activation, hidden windows, `front`
  ignored). Use it for scripted runs on a desktop someone is using; `shot` and
  `BOA_SCREENSHOT` still capture.

```sh
dir=$(mktemp -d)
BOA_CONFIG_DIR=$dir BOA_SCREENSHOT=/tmp/boa.png BOA_DEV_SETTINGS=general \
  cargo run -p boa-app --features screenshot
```

## Tests

`cargo test -p boa-app`. Shell tests (src/tests.rs) open the real main window on a
temp config dir (`init(cx, dir)` + `open_main(cx)` → `(Entity<RootView>, VisualTestContext)`)
and drive it with keystrokes and mouse events through the real keymap.
