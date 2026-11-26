# Phase 6 — transactions and guard rails

Spec: USAGE.md "Transactions and guard rails"; TxControls.tsx, GuardDialogs.tsx,
state/tx.ts, state/guards.ts, lib/guard.ts, Tauri lib.rs `RunEvent::ExitRequested`.

## What's built

| Piece | Where |
|---|---|
| `Transactions` model on `boa_core::state::tx::TxSessions`: mode (Auto / Manual) and probed state (idle / open / aborted) per session; every `run_query` outcome (consoles, pages, table tabs) reported through `transactions::report`; connect / disconnect / delete forget the connection's sessions; `TxLost` from Connections makes the session idle and emits `TransactionsEvent::TxLost`; `set_mode` (`Backend::set_tx_mode`), `end` (COMMIT / ROLLBACK run like a typed statement: Output log + history, the console shows it running / failed) | `crates/boa-app/src/models/transactions.rs` |
| TxControls: "Tx: Auto ▾" menu (Auto-commit / Manual), Commit (✓) / Rollback (↶, red when aborted) while Manual or a transaction is open, the state badge; status bar "Tx: Manual · Transaction open"; ⌘⇧↵ / ⌘⇧⌫ only when they apply (no commit of an aborted transaction); switching to Auto with a transaction open asks first | `crates/boa-app/src/views/tx_controls.rs` |
| Guard dialogs (one at a time, a new one answers the earlier "no"): destructive-statement confirmation (pref; Production always; Enter = Cancel), close-with-open-transaction (disconnect / reconnect / quit / update; ⌘⇧⌫ = "Roll back and …"), "Switch to auto-commit?" (⌘⇧↵ Commit, ⌘⇧⌫ Rollback); installed as the Connections close guard | `crates/boa-app/src/views/guard_dialogs.rs` |
| Quit guard: ⌘Q / Boa → Quit Boa / File → Close Window / the main window's close button ask when transactions are open ("Roll back and quit" closes those connections' sessions, then quits); a second ⌘Q within 3 s quits anyway (`QuitGate`) | `crates/boa-app/src/app.rs` (`request_quit`, `main_window_should_close`) |

Read-only enforcement is the console's existing `check_run` (`Not run: …`) plus the server
(`default_transaction_read_only`); both are covered by a live test with the `boa_ro` role.

## Screenshots (`--features screenshot`, `BOA_DEV_LIVE=1 BOA_DEV_TX=…`)

- `tx-open.png`: Manual mode with an open transaction: toolbar (Tx: Manual, Commit, Rollback, badge) and status bar.
- `tx-menu.png`: the Tx menu (`BOA_DEV_TX=menu BOA_DEV_TX_CLICK=394,57`).
- `destructive-confirm.png`: DELETE without WHERE on a Production-tagged connection (`BOA_DEV_ENV=production`).
- `quit-guard.png`: ⌘Q with a transaction open.
- `end-tx.png`: switching back to Auto with a transaction open.

```sh
dir=$(mktemp -d)
BOA_CONFIG_DIR=$dir BOA_DEV_LIVE=1 BOA_DEV_TX=open BOA_DEV_PANELS=sidebar,bottom \
  BOA_EDITOR_TEXT='insert into boa_txg.shot values (1);' \
  BOA_SCREENSHOT=/abs/out.png BOA_SCREENSHOT_DELAY_MS=5000 cargo run -p boa-app --features screenshot
```

## Tests

`crates/boa-app/src/models/transactions/tests.rs` (17): model (outcomes, connection events,
TxLost), status bar, shortcuts, destructive confirmation (on / off / Production), one prompt at a
time + dialog shortcuts, disconnect guard, quit guard (ask, second ⌘Q, cancel, roll back and
quit, close button), and live: Manual → insert → open → ⌘⇧↵ → idle; ⌘⇧⌫ rollback; aborted
(25P02) state; leaving Manual (rollback / commit); typed BEGIN / COMMIT / ROLLBACK;
destructive confirmation against real rows; read-only connection (`boa_ro`) refusals;
disconnect with an open transaction rolls it back.

## Manual checks (for the user)

1. Real ⌘Q with a transaction open (Manual mode + an INSERT): the prompt appears; Cancel keeps
   the app; ⌘Q twice within 3 s quits without waiting.
2. Boa → Quit Boa from the menu bar: same prompt.
3. Red close button of the main window with a transaction open: same prompt; with none, it quits.
4. Dock → Quit and logging out with a transaction open: **not held** (see deviations); the app
   quits and the server rolls the transaction back. *(Later held by p12/dock-quit,
   `platform/terminate.rs`; see MANUAL-CHECKS M9.)*
5. Editing a connected data source with a transaction open → "Reconnect with an open transaction?".
6. Sidebar → Disconnect with a transaction open → "Disconnect with an open transaction?".
7. ⌘⇧↵ / ⌘⇧⌫ from the editor and from a table tab's grid.

## Deviations

- System quits that bypass the menu / ⌘Q (Dock → Quit, logout, restart) can't be held: GPUI
  0.3.14 has no `applicationShouldTerminate:` hook (only `applicationWillTerminate:` → `on_app_quit`).
  Holding them needs an `unsafe` Objective-C method on the app delegate — ask the user first.
  *(Done since, with the user's OK: p12/dock-quit adds `applicationShouldTerminate:` to GPUI's
  delegate, `crates/boa-app/src/platform/terminate.rs`.)*
- The quit prompt offers Cancel / "Roll back and quit" as the source does (no commit option).
- In guard dialogs ⌘⇧↵ / ⌘⇧⌫ answer the dialog (the source ignored them while a dialog was open);
  Enter is Cancel (the source focused Cancel).
- A COMMIT / ROLLBACK error shows in the console's failure strip and the Output log; it does not
  connect first (the transaction is gone with a closed connection anyway).
