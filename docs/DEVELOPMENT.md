# Developing Boa

How Boa is built, run and tested. For what it is and how to install it, see
the [README](../README.md); for how each feature behaves, see
[USAGE.md](USAGE.md).

Boa is a Tauri 2 app: a Rust backend (`crates/boa-desktop/src`) that owns
the Postgres sessions, and a React + Tailwind v4 frontend
(`crates/boa-desktop/ui`) in the system WebView. The chrome (sidebar,
title-bar tabs, right panel, status bar, ⌘K palette, settings) is carried
over from Civet's design.

## Layout

```
crates/boa-desktop/              Tauri app (Rust side: src/lib.rs)
crates/boa-desktop/ui/           React frontend (Vite, Tailwind v4, shadcn/ui)
  src/state/store.ts             app state: connections, tabs, panels, toasts
  src/state/tx.ts, guards.ts     transaction mode / state per session; confirmations
  src/lib/guard.ts               destructive and read-only statement checks
  src/state/mock.ts              sample connections / schemas
  src/state/mockData.ts          generated rows for the browser mock (mockQuery.ts: its WHERE / ORDER BY)
  src/lib/tableQuery.ts          a table tab's SELECT with its filter bar's WHERE / ORDER BY
  src/lib/backend.ts             Tauri commands (or the browser mock)
  src/state/history.ts           query history (persisted; QueryHistory.tsx is the view)
  src/components/                Sidebar, TitleBar, Workspace, BottomPanel, DataGrid, RightPanel, StatusBar, …
crates/boa-desktop/src/          pg.rs (sessions, introspection, queries), tx.rs (transaction state probe),
                                 connections.rs (saved settings + passwords), ssh.rs (SSH tunnels),
                                 edits.rs (submitting grid edits),
                                 history.rs (query history)
```

## Run

```sh
# UI only, in a browser (http://localhost:1420; add ?overlay to preview the macOS title-bar inset)
pnpm --dir crates/boa-desktop/ui install
pnpm --dir crates/boa-desktop/ui dev:web

# Desktop app
pnpm --dir crates/boa-desktop/ui dev
```

In a plain browser (`dev:web`) a mock backend serves the sample data in
`ui/src/state/mock.ts`: CSV export builds the file from the sample rows (at
most 20,000) and downloads it, submitted edits persist until the page is
refreshed, and query history lives in `localStorage`.

Set `BOA_LOG` (a `tracing` filter, e.g. `BOA_LOG=debug`) for backend logs.

## Checks

```sh
pnpm --dir crates/boa-desktop/ui typecheck
pnpm --dir crates/boa-desktop/ui lint
pnpm --dir crates/boa-desktop/ui test
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

The live Postgres test runs when `PGHOST` (and `PGPORT`, `PGUSER`,
`PGPASSWORD`, `PGDATABASE`) point at a server; it creates and drops a
`boa_test` schema.

CI (`.github/workflows/ci.yml`) runs these on every pull request and push to
`main`. Releases are cut from Actions → Cut release (or
`scripts/release.sh`); see [RELEASING.md](RELEASING.md).
