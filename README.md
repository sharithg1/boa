# Boa

A fast, native PostgreSQL client for macOS: a Database Explorer tree, SQL
consoles with completion, an editable data grid, query history, CSV export and
guard rails for production, in one small Rust app.

This repository is Boa rebuilt on [GPUI](https://gpui.rs/) and
[gpui-component](https://github.com/longbridge/gpui-component), in pure Rust.
It replaces the Tauri 2 + React app (`dbclient`), whose behaviour is the spec,
and keeps its users' data where it is: same bundle id (`dev.boa.desktop`),
same config dir, same `connections.json` and history files.

- How every feature behaves: [docs/USAGE.md](docs/USAGE.md) (with the
  differences from the Tauri app at the end).
- Port plan, parity checklist and progress log: [HANDOFF-PARITY.md](HANDOFF-PARITY.md);
  gap list: [docs/parity-audit.md](docs/parity-audit.md); what only a person
  can check: [docs/MANUAL-CHECKS.md](docs/MANUAL-CHECKS.md).

## Build and run

Needs macOS 11+ (Apple silicon or Intel) with the Xcode Command Line Tools, and
Rust through rustup (`rust-toolchain.toml` pins the version).

```sh
export PATH="$HOME/.cargo/bin:$PATH"     # if cargo isn't on PATH
mkdir -p ~/boa-dev-config
BOA_CONFIG_DIR=~/boa-dev-config cargo run -p boa-app
```

**Use `BOA_CONFIG_DIR` for every development run.** Without it Boa uses the
real config dir, `~/Library/Application Support/dev.boa.desktop/`, which
holds your saved connections (with passwords), history, prefs and session,
shared with an installed Boa.app. With `BOA_CONFIG_DIR` set, the one-time
import of the Tauri app's localStorage looks only in `BOA_IMPORT_HOME` (a
home dir; unset: no import), so a dev run never reads your real WebKit data
either.

Release builds and the `.app` bundle: `scripts/bundle.sh`, see
[docs/RELEASING.md](docs/RELEASING.md).

## Test

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
PGHOST=localhost PGPORT=5432 PGUSER=boa PGPASSWORD=boa PGDATABASE=boa_dev cargo test --workspace
```

Without `PGHOST` the live Postgres tests skip. The local Postgres setup, the
fixtures (`scripts/dev-fixtures.sh`), the dev hooks and the screenshot feature
are in [docs/DEVELOPMENT.md](docs/DEVELOPMENT.md).

## Your data, and moving from the Tauri app

| File in the config dir | Holds |
|---|---|
| `connections.json` | data sources and passwords (plain text, owner-only), same format as the Tauri app |
| `history-<connection id>.jsonl` | query history, same format |
| `prefs.json`, `session.json`, `csv-export.json` | what the Tauri app kept in WebView localStorage (`boa.prefs.v2`, `boa.session.v1`, `boa.csvExport`), same fields |

On first launch Boa imports those three from the Tauri app's localStorage
once. The Tauri app can still be used afterwards: it reads the connections
and history Boa writes (checked by `crates/boa-app/src/tests/migration.rs`).
Details in [docs/USAGE.md](docs/USAGE.md#where-your-data-lives).

## Architecture

```
crates/boa-core/   no UI: Postgres sessions (one operation at a time per session),
                   introspection and DDL, CSV export (COPY), transactions, saved
                   connections, history, SSH tunnels, the async Backend bridge, and
                   the Tauri UI's pure logic ported from TypeScript (sql/, grid/, state/)
crates/boa-app/    the GPUI app: models (GPUI entities replacing the zustand store),
                   views (one per React component), actions + keymap, theme, menus,
                   the in-app updater
```

Start with [crates/boa-core/README.md](crates/boa-core/README.md) (Backend
API, events, module layout) and [docs/DEVELOPMENT.md](docs/DEVELOPMENT.md).

## Licence

MIT. Dependencies are MIT / Apache-2.0 / BSD / ISC / Zlib / Unicode / OFL, or
MPL-2.0 unmodified, enforced by `cargo deny` (`deny.toml`); no GPL.
