# Developing Boa (GPUI)

How Boa is built, run and tested. Boa is a macOS PostgreSQL client written in
Rust on [GPUI](https://gpui.rs/) and [gpui-component](https://github.com/longbridge/gpui-component).
It is a port of the Tauri 2 + React app; that app is the behaviour spec
(`docs/USAGE.md` there), and the port's plan and progress are in
[HANDOFF-PARITY.md](../HANDOFF-PARITY.md).

## Layout

```
Cargo.toml              workspace: members, lints (unsafe_code = deny), dependency pins, release profile
rust-toolchain.toml     pinned compiler (CI and rustup use it)
deny.toml               cargo-deny licence policy
crates/boa-core/        everything without UI: Postgres sessions, introspection, CSV export,
                        saved connections, history, SSH tunnels, the async Backend bridge,
                        and the ported TS logic (sql/, grid/, state/). See its README.md.
crates/boa-app/         the GPUI app: windows, views, actions / keymap, theme, menus,
                        models (GPUI entities), the in-app updater. See its README.md.
scripts/dev-fixtures.*  Postgres fixtures for development and live tests
scripts/bundle.sh, rehearse-*.sh, latest-json.sh, check-version.sh
                        bundling and update rehearsals (docs/RELEASING.md)
docs/USAGE.md           how every feature behaves (the spec, adjusted for this app)
docs/MANUAL-CHECKS.md   what only a person at the Mac can check
docs/parity-audit.md    the gap list against the Tauri app; docs/audit/ its captures
docs/perf.md            startup, memory, scroll and typing numbers
docs/phase0/ … phase7/  per-phase reports and screenshots
```

## Requirements

- macOS 11+ on Apple silicon or Intel, with the Xcode Command Line Tools. Full
  Xcode is not needed: GPUI compiles its Metal shaders at run time
  (`runtime_shaders`).
- Rust via rustup. `rust-toolchain.toml` pins the version; rustup installs it
  on first use (`rustup toolchain install` does it explicitly). In
  non-interactive shells cargo may not be on `PATH`:
  `export PATH="$HOME/.cargo/bin:$PATH"`.
- PostgreSQL 17 for the live tests and fixtures (Homebrew `postgresql@17`).

## Build and run

```sh
cargo build                      # debug build of the workspace
cargo run -p boa-app             # run the app
cargo build --release -p boa-app # optimized (thin LTO, stripped)
```

**Always point a development build at a scratch config dir.** By default Boa
uses the Tauri app's config dir, `~/Library/Application Support/dev.boa.desktop/`,
which holds your real saved connections (with passwords), history, prefs and
session. `BOA_CONFIG_DIR` makes the app use another directory:

```sh
mkdir -p ~/boa-dev-config
BOA_CONFIG_DIR=~/boa-dev-config cargo run -p boa-app
```

Tests and agents must never write to the real config dir or to
`/Applications/Boa.app`. To test migration from a real install, copy the
config dir to a scratch location first and point `BOA_CONFIG_DIR` at the copy.

Other environment variables:

| Variable | Effect |
|---|---|
| `BOA_CONFIG_DIR=<dir>` | config dir to use instead of the real one (development, screenshots, migration tests) |
| `BOA_IMPORT_HOME=<dir>` | with `BOA_CONFIG_DIR` set: the home dir the one-time localStorage import looks in (`<dir>/Library/WebKit/dev.boa.desktop/…`). Unset: no import. Without `BOA_CONFIG_DIR` the import uses `$HOME` |
| `BOA_LOG=<filter>` | `tracing` filter for logs, e.g. `BOA_LOG=debug` or `BOA_LOG=boa_core=trace` (default `info`) |
| `BOA_NO_UPDATE_CHECK=1` | don't check for updates (`boa_core::state::updates`) |
| `BOA_SCREENSHOT`, `BOA_SCREENSHOT_DELAY_MS` | screenshot and quit (feature `screenshot`, below) |
| `BOA_EDITOR_TEXT`, `BOA_EDITOR_TYPE` | put text in the editor / type it at the end once the catalog is loaded (feature `screenshot`) |
| `BOA_GRID_*` | grid dev hooks: startup SQL, selection, staged edits, scroll, benchmark, quit-after (see `crates/boa-app/src/grid/dev.rs`) |
| `BOA_DEV_CONNECTION=1` | (feature `dev-hooks`) seeds a "Local dev" connection (localhost boa/boa, `boa_dev`) into an empty `BOA_CONFIG_DIR` and connects it at launch; `BOA_DEV_RECONNECT=1` adds an unreachable one |
| `BOA_DEV_INPUT=<file>` | (feature `dev-hooks`) drives the main window from a command file: `key`, `type`, `click`, `wait`, `shot <png>` (see `crates/boa-app/src/views/dev_input.rs`) |
| `BOA_DEV_BACKGROUND=1` | (feature `dev-hooks`) stays off the user's screen: no Dock icon or menu bar, never activates, windows open hidden, `front` is ignored. Use it for every scripted or agent run; capture with `shot` / `BOA_SCREENSHOT` (`app::background`) |
| `BOA_DEV_*` | other scripted states for captures (panels, settings, palette, history, tx, exports, …; `crates/boa-app/src/views/dev.rs`) |
| `BOA_UPDATE_ENDPOINT=<url>` | the `latest.json` feed update checks use (`file://`, `http://`, `https://`); see docs/RELEASING.md |
| `BOA_MIGRATION_DIR=<dir>`, `BOA_TAURI_UI_SRC=<dir>` | migration checks, below |

Kill only the app processes you started (`kill <pid>`); never `pkill boa-app`,
other people or agents may be running their own copies.
`BOA_GRID_QUIT_AFTER_MS=<n>` makes the app quit itself.

### Screenshots without Screen Recording

The `screenshot` feature renders the main window to a PNG with GPUI's
`render_to_image` and quits, so scripts and agents can check the UI without
Screen Recording permission:

```sh
BOA_CONFIG_DIR=~/boa-dev-config BOA_SCREENSHOT=/abs/path/out.png BOA_SCREENSHOT_DELAY_MS=2000 \
  cargo run -p boa-app --features screenshot
```

Native parts (traffic lights, menus, separate windows such as dialogs) are not
in the image. With `BOA_DEV_INPUT`, a `shot <png>` line captures the current
state after scripted input (it draws a fresh frame, so the window doesn't need
to be in front). Add `BOA_DEV_BACKGROUND=1` so the run never shows a window or
takes the focus from whoever is using the machine. With Screen Recording granted to the terminal,
`screencapture -l <window id>` captures the real window, chrome included. Combine it with the `BOA_EDITOR_*` / `BOA_GRID_*` hooks to script
the state before the capture. For interactions, write `#[gpui::test]` tests
with `VisualTestContext` (`simulate_keystrokes`, `simulate_input`, mouse
events); they go through the real keymap and dispatch tree.

## Checks

All three must pass before anything is merged (CI runs them too). Check each
command's exit code; piping into `grep` / `tail` hides failures.

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
PGHOST=localhost PGPORT=5432 PGUSER=boa PGPASSWORD=boa PGDATABASE=boa_dev cargo test --workspace
```

Also worth running when you touch the screenshot hooks or licences:

```sh
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo deny check licenses        # cargo install cargo-deny --locked
```

While iterating, `cargo test -p boa-core` (or `-p boa-app`) is much cheaper
than the whole workspace.

### Migration from the Tauri app

`crates/boa-app/src/tests/migration.rs` is the end-to-end check that existing
users' data carries over and stays readable by the Tauri app (users may
downgrade). It builds a synthetic Tauri-era config dir and WebKit
localStorage database in a temp dir, starts the app on it as `app::run`
does, checks connections (with passwords), history, prefs, session and CSV
options, lets the app write every file it owns, and checks each file with
the Tauri app's rules. It runs in the normal test suite.

With the Tauri source at hand, the test also runs the source's own TypeScript
parsers (`state/prefs.ts`, `state/session.ts` `parseSession`,
`state/csvExport.ts`) on the written files through
`crates/boa-app/src/tests/tauri_rules.mjs` (needs node 22.18+ and the source's
`ui/node_modules`; nothing is written to the source tree):

```sh
BOA_TAURI_UI_SRC=/path/to/dbclient/crates/boa-desktop/ui/src cargo test -p boa-app migration -- --nocapture
```

**Rehearsal on a copy of a real install.** Never point anything at the real
config dir. Copy it and WebKit's data into a scratch dir, remove the files
only the GPUI app writes (so the copy looks like a Tauri-era install), then:

```sh
M=$(mktemp -d)/migration && mkdir -p "$M/config" "$M/home/Library/WebKit"
cp -Rp ~/Library/Application\ Support/dev.boa.desktop/. "$M/config/"
cp -Rp ~/Library/WebKit/dev.boa.desktop "$M/home/Library/WebKit/"
rm -f "$M/config/"{prefs.json,session.json,csv-export.json,.localstorage-imported}
BOA_MIGRATION_DIR="$M" BOA_TAURI_UI_SRC=… \
  cargo test -p boa-app -- --ignored real_install_copy --nocapture
```

It prints counts and yes / no answers only (no SQL, names or hosts). To look
at the restored app itself, run it on another copy with
`BOA_CONFIG_DIR="$M/config" BOA_IMPORT_HOME="$M/home"`, after pointing every
connection in the copy's `connections.json` at an address where nothing
listens (e.g. `127.0.0.1` port 1, SSH disabled): session restore reconnects
the connections that were open at quit, and the copy holds real hosts and
passwords. Delete the copies afterwards.

### Live Postgres tests

Without `PGHOST` the live tests print a note and skip; always run them with
the variables above before merging. They create and drop their own schemas
(`boa_test`, …) in `boa_dev`. Timing-sensitive tests (e.g.
`ssh::tests::unreachable_ssh_server_fails_cleanly`) can be flaky while other
heavy builds run; re-run them alone before calling it a failure.

### Local Postgres

Homebrew's `postgresql@17`, started by hand (not as a launch service):

```sh
/opt/homebrew/opt/postgresql@17/bin/pg_ctl -D /opt/homebrew/var/postgresql@17 \
  -l /opt/homebrew/var/log/postgresql@17.log start      # or stop
```

The development role is `boa` / `boa` (superuser) and the database `boa_dev`,
which has `public.customers` (5,000 rows) and `public.orders` (100,000 rows,
FK to customers, jsonb). Don't drop or change those two; put your own test data
in other schemas or databases. On a fresh server:

```sh
createuser -s boa && psql -d postgres -c "alter role boa password 'boa'"
scripts/dev-fixtures.sh   # creates boa_dev (and the two public tables) when missing
```

### Fixtures

`scripts/dev-fixtures.sh` loads `scripts/dev-fixtures.sql` into the server the
`PG*` variables point at (defaults: localhost:5432, boa/boa). It is idempotent:
it drops and rebuilds only what it owns, so run it whenever you like.

```sh
scripts/dev-fixtures.sh
```

| Where | What |
|---|---|
| `boa_dev.public` | `customers`, `orders`: created and seeded only when missing, never altered |
| `boa_dev.boa_fx` | `all_types` (1,000 rows: numeric, real, bool, char/varchar/text incl. long and multi-line, jsonb incl. one large document, json, text[] / int[] / int[][], bytea, uuid, date / time / timestamp / timestamptz, interval, inet / cidr, enum, domain, xml, point, range); `products` (identity PK, defaults, a stored generated column, checks); `counters` (serial, defaults); `bookings` (column, table, multi-column and NOT VALID checks); `regions` → `stores` → `employees` (composite PK / FK, self-reference, ON DELETE / UPDATE actions, partial and expression unique index, INCLUDE index); `order_heads` / `order_lines` (composite FK onto a composite PK); `event_log` (no PK, duplicate rows); `settings_kv` (unique, no PK); `measurements` (partitioned, two partitions); views `active_products`, `store_summary`; materialized view `daily_lines`; functions `slow(seconds)`, `slow_rows(n, delay)`, `fail(msg)` |
| `boa_dev.boa_fx_many` | 50 tables `t01`…`t50` (FK chain, 1–7 extra columns) and 5 views, for the explorer |
| `boa_dev."boa_fx_ünïcødé"` | non-ASCII / quoted names: `"Kunden"` (`"Größe"`, `"名前"`, `"emoji 🐘"`, `"Mixed Case"`, `"select"`, `"has""quote"`), `"Bestellungen_ä"` (`"Betrag €"`), `"表"` (no PK), view `"Ansicht ö"` |
| `boa_dev.boa_fx_empty` | an empty schema |
| `boa_dev2` | a second database: `public.products`, `public.stock` (composite PK, FK), `boa_fx2.notes`, view `boa_fx2.stock_value` |
| role `boa_ro` / `boa_ro` | read-only: SELECT on the fixture schemas only, `default_transaction_read_only = on` |

Cancel tests: `select boa_fx.slow(30)` sleeps 30 s; `select * from
boa_fx.slow_rows(10, 0.5)` returns a row every half second.

## CI

`.github/workflows/ci.yml` runs on every pull request and push to `main`:

- `rustfmt` and `cargo deny check licenses` (Linux);
- clippy (default and all features) and the full test suite on macOS arm64,
  against a Homebrew Postgres 17 set up like the local one and loaded with
  `scripts/dev-fixtures.sh` (service containers don't run on macOS runners);
- an x86_64 build of every target, cross-compiled on the arm64 runner (tests
  run too when the runner has Rosetta 2).

Releases: Actions → Cut release bumps, tags and pushes; the tag runs
`.github/workflows/release.yml`, which builds both architectures and publishes
the GitHub release with the update feed. See [RELEASING.md](RELEASING.md).

## Licences

Boa is MIT. Dependencies must be MIT / Apache-2.0 / BSD / ISC / Zlib / Unicode /
OFL / CC0 / BSL / 0BSD, or MPL-2.0 used unmodified; `deny.toml` is the list and
CI enforces it. No GPL / AGPL / LGPL: in particular no Zed crates other than
`gpui*`. Zed's source is fine to read for patterns, never to copy.

## Working with parallel agents

The port is built by a lead and several agents working in parallel
(HANDOFF-PARITY.md §6):

1. **Contract first.** The lead lands the shared types, entity skeletons,
   events and module stubs on `main`; each agent then owns a disjoint set of files.
2. **One worktree per agent:** `git worktree add .worktrees/<name> -b p<N>/<name>`
   (`.worktrees/` is gitignored). Agents commit to their own branch only and never push.
3. **Agent brief:** the environment and rules above (checks must pass, commit
   trailer, no system installs, don't touch other worktrees or the source repo),
   the contract, the source files that are the spec for the slice, the
   required tests and screenshots, and a short report (built / API / tests /
   not verified / gotchas).
4. **Merging (lead):** `git merge --no-ff --no-commit`, resolve (`Cargo.lock`:
   take one side and let cargo re-resolve), run all three checks with exit
   codes checked, then commit. Never commit a red `main`.
5. **Review the reports:** fix, schedule or record every "not verified" and
   "deviation" item.
6. Keep 2–4 agents per wave; more and the merges and the machine become the bottleneck.

Port TS logic with its tests (same cases), keep on-disk formats byte-compatible
with the Tauri app, and end every commit message with the attribution line
the project asks for.
