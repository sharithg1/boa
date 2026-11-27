---
name: run-boa
description: Run, drive or screenshot the Boa GPUI app (crates/boa-app) against the local Postgres to check a change in the real app, not just in tests — connect, expand the explorer, open tables, type and run SQL, read the grid / Output log, capture PNGs. Always off the user's screen (BOA_DEV_BACKGROUND). Use when asked to run, test, try, verify or screenshot the app.
---

# Run Boa (GPUI) for a check

The user keeps working on this Mac: **the app must never show a window or take
the focus** (see AGENTS.md). Everything below runs it hidden and captures
offscreen. Dev variables: the table at the end; the dev hooks: `crates/boa-app/src/views/dev.rs`, `views/dev_input.rs`, `grid/dev.rs`.

## Prefer a test

Behaviour (keys, focus, menus, models) is cheaper and deterministic as a
`#[gpui::test]` with `VisualTestContext` (real keymap and dispatch). Helpers:
`crates/boa-app/src/tests.rs` (`init`, `open_main`, `fake_connection`); copy
the style of `views/workspace/tests.rs`. Tests whose pretend connection still
reaches the backend need `cx.executor().allow_parking()`. Use a live run for
what tests can't show: layout, pixels, real data, timing.

## 1. Postgres + fixtures (once per boot)

```sh
export PATH="$HOME/.cargo/bin:$PATH"
/opt/homebrew/opt/postgresql@17/bin/pg_ctl -D /opt/homebrew/var/postgresql@17 status \
  || /opt/homebrew/opt/postgresql@17/bin/pg_ctl -D /opt/homebrew/var/postgresql@17 \
       -l /opt/homebrew/var/log/postgresql@17.log start
scripts/dev-fixtures.sh   # idempotent; role boa/boa, db boa_dev (+ boa_fx* schemas, boa_dev2)
```

Never drop or alter `boa_dev.public.customers` / `orders`; make your own schema
for test data.

## 2. Launch hidden, scripted

Use a scratch config dir (never the real `~/Library/Application Support/dev.boa.desktop`)
and a unique scratch folder for your files:

```sh
cargo build -p boa-app --features screenshot   # first build takes minutes
W=$(mktemp -d -t boa-run)                      # config dir, command file, shots
: > "$W/input.txt"
```

Then start it as a **background** command (your agent's background-command mode,
or a command awaited with a short timeout so it moves to the background — not
`&` / `nohup`). `exec` keeps the PID that `$W/pid` records:

```sh
sh -c 'echo $$ > "$0/pid"; exec env BOA_CONFIG_DIR="$0/config" BOA_DEV_BACKGROUND=1 \
  BOA_DEV_CONNECTION=1 BOA_DEV_INPUT="$0/input.txt" BOA_NO_UPDATE_CHECK=1 \
  BOA_GRID_QUIT_AFTER_MS=600000 target/debug/boa-app' "$W"
```

- `BOA_DEV_CONNECTION=1` seeds "Local dev" (localhost boa/boa, `boa_dev`) into
  the empty config dir and connects at launch.
- `BOA_GRID_QUIT_AFTER_MS` is a safety net: the app quits itself after 10 min.

## 3. Drive it and capture

Append commands to `$W/input.txt`; each line runs once, and `$W/input.txt.done`
holds the count done so far (wait until it reaches your line count before
reading a shot).

| Command | Does |
|---|---|
| `key cmd-t alt-1 enter` | keystrokes, GPUI syntax, space separated |
| `type select 1;` | types the rest of the line (`\\n` = Enter) |
| `click X Y [2] [cmd,shift]` | left click at window point (X, Y); 2 = double click |
| `rclick X Y` / `move X Y` | right click / hover |
| `wait 500` | milliseconds |
| `shot /abs/path.png` | draws a fresh frame and writes a PNG (offscreen) |

`front` exists but is ignored with `BOA_DEV_BACKGROUND`; don't use it.

Coordinates are window points; the main window is 1280 × 820 pt and **shots
are 2× (2560 × 1640 px)**. Your image viewer may show a shot scaled down (e.g.
2000 px wide), so convert what you see: point = seen px × (2560 / seen width) / 2.
Take a shot, look at it, then click.

Nothing is selected at launch: **select a database first** (click its row in
the explorer), or ⌘T and the console have nothing to run on. Example (verified)
— select `boa_dev`, open a console, run a query:

```sh
cat >> "$W/input.txt" <<EOF
wait 2500
shot $W/01-start.png
click 96 117
wait 500
key cmd-t
type select status, count(*) from public.orders group by 1;
key cmd-enter
key cmd-enter
wait 1500
shot $W/02-result.png
EOF
```

(`click 96 117` is the `boa_dev` row under "Local dev" in the default layout;
⌘↵ arms the statement under the caret, the second press runs it.) Read the
PNGs to check them.

One-shot capture without scripting: `BOA_SCREENSHOT=/abs/out.png
BOA_SCREENSHOT_DELAY_MS=2000` (renders and quits). Other scripted states
(`BOA_DEV_SETTINGS`, `BOA_DEV_PALETTE`, `BOA_DEV_HISTORY`, `BOA_GRID_SQL`, …)
are listed at the end.

## 4. Clean up

`kill $(cat "$W/pid")` — only the processes **you** started, never `pkill boa-app` (the user or
other agents may be running copies). Delete `$W` when done.

## Needs the user

Offscreen captures don't include native chrome: traffic lights, the menu bar,
macOS dialogs and save panels, real trackpad / IME input. For those, ask the
user before a visible run and say when the window will appear.

## Dev environment variables

Each is read where its name appears in `crates/` (`git grep '"BOA_'`).

| Variable | Effect |
|---|---|
| `BOA_CONFIG_DIR=<dir>` | config dir instead of the real one (always set it) |
| `BOA_LOG=<filter>` | `tracing` filter, e.g. `debug` or `boa_core=trace` (default `info`) |
| `BOA_NO_UPDATE_CHECK=1` | don't check for updates |
| `BOA_SCREENSHOT`, `BOA_SCREENSHOT_DELAY_MS` | render a PNG and quit (feature `screenshot`) |
| `BOA_EDITOR_TEXT`, `BOA_EDITOR_TYPE` | put text in the editor / type it once the catalog is loaded |
| `BOA_GRID_*` | grid hooks: startup SQL, selection, staged edits, scroll, benchmark, `BOA_GRID_QUIT_AFTER_MS` (`crates/boa-app/src/grid/dev.rs`) |
| `BOA_DEV_CONNECTION=1` | (`dev-hooks`) seed "Local dev" into an empty config dir and connect; `BOA_DEV_RECONNECT=1` adds an unreachable one |
| `BOA_DEV_INPUT=<file>` | (`dev-hooks`) drive the window from a command file (`crates/boa-app/src/views/dev_input.rs`) |
| `BOA_DEV_BACKGROUND=1` | (`dev-hooks`) stay off the user's screen; use on every run |
| `BOA_DEV_*` | other scripted states: settings, palette, history, tx, exports, … (`crates/boa-app/src/views/dev.rs`) |
| `BOA_UPDATE_ENDPOINT=<url>` | the `latest.json` feed for update checks |
