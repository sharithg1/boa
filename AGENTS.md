# Notes for coding agents

Boa is a macOS PostgreSQL (and Redis) client in Rust on [GPUI](https://gpui.rs/) and
[gpui-component](https://github.com/longbridge/gpui-component).

## Layout

```
crates/boa-core/        no UI: Postgres sessions, introspection, CSV export, saved
                        connections, history, SSH tunnels, the async Backend bridge
crates/boa-app/         the GPUI app: windows, views, actions / keymap, theme, menus,
                        models, the in-app updater (see its README.md)
scripts/dev-fixtures.*  Postgres fixtures for development and the live tests
scripts/bundle.sh, rehearse-*.sh, latest-json.sh, check-version.sh, release.sh
                        bundling, update rehearsals and releases
.github/workflows/      ci.yml (checks), cut-release.yml + release.yml (releases)
.agents/skills/         agent skills (`.claude/skills` links here)
```

## Requirements

- macOS 11+ with the Xcode Command Line Tools (full Xcode not needed).
- Rust via rustup; `rust-toolchain.toml` pins the version. In non-interactive
  shells: `export PATH="$HOME/.cargo/bin:$PATH"`.
- PostgreSQL 17 in Docker (no local psql needed) for the live tests and
  fixtures: role `boa` / `boa`, database `boa_dev`, loaded with
  `scripts/dev-fixtures.sh` (idempotent; it runs psql inside the container).
  Never drop or alter `boa_dev.public.customers` / `orders`; put test data in
  your own schema. The container restarts with Docker; create it once with:

  ```sh
  docker run -d --name boa-postgres --restart unless-stopped -p 5432:5432 \
    -e POSTGRES_USER=boa -e POSTGRES_PASSWORD=boa -e POSTGRES_DB=postgres \
    -v boa-pgdata:/var/lib/postgresql/data postgres:17
  ```

  For a psql shell: `docker exec -it boa-postgres psql -U boa -d boa_dev`.

## Build and run

```sh
cargo build                      # debug build of the workspace
BOA_CONFIG_DIR=<scratch dir> BOA_DEV_BACKGROUND=1 cargo run -p boa-app --features dev-hooks
cargo build --release -p boa-app # optimized
scripts/dev.sh                   # rebuild + relaunch on save (needs watchexec)
```

There is no hot reload; `scripts/dev.sh` is the dev loop (see its header).
Agents running it must add `BOA_DEV_BACKGROUND=1` like any other run.

## Checks before merging

CI (`.github/workflows/ci.yml`) runs all of these; run them locally first and
check each exit code (piping into `grep` / `tail` hides failures):

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
PGHOST=localhost PGPORT=5432 PGUSER=boa PGPASSWORD=boa PGDATABASE=boa_dev \
  cargo test --workspace --locked
cargo deny --locked check licenses
```

Without `PGHOST` the live Postgres tests skip, so always set it before merging.
The live Redis tests skip without `BOA_REDIS_PORT` / `BOA_REDIS_AUTH_PORT`
(see `crates/boa-core/README.md`).
While iterating, `cargo test -p boa-core` (or `-p boa-app`) is much cheaper.
Dependencies must stay MIT / Apache-2.0 / BSD-style or unmodified MPL-2.0
(`deny.toml`); no GPL / AGPL / LGPL.

## Running the app while the user works

The user keeps working on this Mac while you test, so the app must never pop up
or take the focus:

- Launch every dev run, screenshot and `BOA_DEV_INPUT` session with
  `BOA_DEV_BACKGROUND=1` (feature `dev-hooks` / `screenshot`). The app then runs
  without a Dock icon, never activates, and opens its windows hidden.
- Capture with `shot <png>` (in the `BOA_DEV_INPUT` file) or `BOA_SCREENSHOT`. Both
  render offscreen.
- Never use the `front` command, `osascript` / System Events activation, `open`
  on the app, or `screencapture` of the live window. Check behaviour with
  `#[gpui::test]` tests (`VisualTestContext`) where possible.
- Native chrome (traffic lights, the menu bar, macOS dialogs) needs a normal,
  visible run: ask the user first, and say when the window will appear.
- Use a scratch `BOA_CONFIG_DIR` (never the real
  `~/Library/Application Support/dev.boa.desktop`), and kill only the processes
  you started (`kill <pid>`, never `pkill`). `BOA_GRID_QUIT_AFTER_MS` makes the
  app quit itself.

The `run-boa` skill (`.agents/skills/run-boa/SKILL.md`) has the full recipe and
the dev environment variables.
