# Manual checks

What the automated tests and scripted captures can't prove: real hardware
input, the native menu bar and window chrome, sleep and the network, an SSH
server, the installed Tauri app, and signing keys. HANDOFF-PARITY.md §11 calls
these **M** items. Nothing here has been run yet (2026-11-26); please tick
each box (or note what went wrong) and the lead copies the result into §11 /
§13.

**Before you start**

- Use a development build with a scratch config dir unless a check says
  otherwise: `BOA_CONFIG_DIR=~/boa-dev-config cargo run -p boa-app`
  (`--features dev-hooks` and `BOA_DEV_CONNECTION=1` seed a "Local dev"
  connection to the local `boa_dev`). Checks marked **real install** use
  your real config dir: back it up first with
  `cp -Rp ~/Library/Application\ Support/dev.boa.desktop ~/boa-config-backup`.
- For transaction checks: open a console on a database, switch **Tx** to
  Manual (or type `begin;`) and run e.g.
  `create temp table m(x int); insert into m values (1);`, so the status bar
  shows *Transaction open*.
- `BOA_LOG=info` prints what the app does (connections checked, reconnects,
  quit decisions) to the terminal.

## Connections and the network

- [ ] **M1. SSH tunnel with a real server** (§11 items 1, 18). Turn on Remote
  Login or use a server you have, with its host key already in
  `known_hosts`. Add a data source with **SSH tunnel**: once with the agent,
  once with a key file chosen with Browse…. Test connection, then Save and
  connect, run a query, Cancel a `select pg_sleep(30)`, disconnect.
  *Expect:* all work; while connected `pgrep -fl "ssh -N"` shows one tunnel
  per data source, none after Disconnect. Also try a host whose key isn't
  known: ssh's own "Host key verification failed" is shown.
- [ ] **M2. SSH tunnels at quit.** Connect through a tunnel, ⌘Q, then
  `pgrep -fl "ssh -N"`. *Expect:* no tunnel left. (Parity-audit P1-2: `main`
  at `087179f` doesn't close them; p10/keys adds the fix. Run this on a build
  that has it.)
- [ ] **M3. Sleep / wake** (§11 item 18). Connect, open a transaction, sleep
  the Mac for a minute or more, wake it. *Expect:* the connection is checked
  at once ("checking connections" in the log); a dropped session is reopened
  on next use, and the lost transaction shows "Transaction rolled back".
- [ ] **M4. Network change.** Connect to a server on another machine, turn
  Wi-Fi off, wait, turn it on. *Expect:* the data source shows
  *reconnecting* (amber) while unreachable, then reconnects within a few
  seconds of the network coming back; one toast per connection, not one per
  retry.
- [ ] **M5. Wrong password (28P01) from a real server.** Against a server
  with password authentication, connect with a wrong saved password.
  *Expect:* the password prompt opens with the server's message; a correct
  password with "Save password" on stores it in `connections.json`.

## Transactions and quitting

All with a transaction open (see above), on a development build.

- [ ] **M6. ⌘Q** (§11 item 19). *Expect:* "Quit with an open transaction?";
  Cancel keeps the app and the transaction; "Roll back and quit" quits; ⌘Q
  twice within 3 s quits without waiting.
- [ ] **M7. Boa → Quit Boa** from the menu bar: the same prompt.
- [ ] **M8. The red close button** of the main window: the same prompt; with
  no transaction open it quits at once.
- [ ] **M9. Dock → Quit, logging out, restarting** (P0-2, `platform/terminate.rs`).
  Use a normal (visible, not `BOA_DEV_BACKGROUND`) build with a Dock icon,
  e.g. the bundle from `scripts/bundle.sh` or `cargo run -p boa-app`.
  1. Open a transaction (Tx: Manual + an INSERT, or run `begin`); the
     status bar says "Transaction open".
  2. Right-click Boa's Dock icon → Quit. *Expect:* the app stays and shows
     "Quit with an open transaction?" (Tauri did the same). Cancel: the app
     and the transaction stay.
  3. Dock → Quit again, then within 3 s Dock → Quit once more. *Expect:* the
     second one quits without waiting (the ⌘Q rule; one gate for both).
  4. Reopen, open a transaction, Dock → Quit, "Roll back and quit". *Expect:*
     the app quits once (no second prompt); next launch reconnects and the
     row isn't there.
  5. With *no* transaction open, Dock → Quit. *Expect:* quits at once.
  6. Transaction open, Apple menu → Log Out… (or Restart…). *Expect:* the prompt
     appears and macOS shows "Boa cancelled logout" (as with the Tauri app).
     Cancel the prompt and log in normally; or answer "Roll back and quit"
     and log out again (now nothing stops it). With no transaction open,
     logging out isn't held at all.
  Also check no hang and no crash report in each case.
  *Seen by the agent (p12/dock-quit):* steps 2 and 4 on a background dev
  build, with the quit Apple event the Dock sends
  (`NSRunningApplication.terminate` from `osascript -l JavaScript`): held
  (`applicationShouldTerminate: reply=Cancel` in the log), the prompt shown,
  "Roll back and quit" closed the `idle in transaction` session, the next
  terminate answered Now and the process exited. Logout / restart not run.
- [ ] **M10. Reconnect and disconnect guards.** Edit the connected data
  source (change its colour) and Save: "Reconnect with an open
  transaction?". Explorer → right-click → Disconnect: "Disconnect with an
  open transaction?". Both roll back when confirmed.
- [ ] **M11. ⌘⇧↵ / ⌘⇧⌫ on a real keyboard**, from the editor and from a table
  tab's grid: commit / roll back; the state in the toolbar and status bar
  follows.

## Keyboard, input and menus

- [ ] **M12. The native Edit menu** (§11 item 22). In the editor, a filter
  field and the data source dialog, use Edit → Undo, Redo, Cut, Copy, Paste,
  Select All from the menu bar (not the keys). *Expect:* each acts on the
  focused field.
- [ ] **M13. Native menus.** Click through Boa / File / Edit / View / Window /
  Help: About Boa, Check for Updates…, Settings… (⌘,), Hide (⌘H), Hide
  Others (⌥⌘H), Show All, Minimize (⌘M), Zoom, Enter Full Screen (⌃⌘F),
  Close Window. *Expect:* each does what it says; compare with the Tauri
  app's menus (the GPUI app adds Settings… and Show All). With p10/keys
  merged, ⌘W closes the About window, and the main window when no tab is
  open (P1-3).
- [ ] **M14. A German keyboard layout** (the ⌥ rule, `keymap.rs`). Add
  "German" in System Settings → Keyboard → Input Sources and switch to it.
  In the editor type ⌥5, ⌥6, ⌥L, ⌥N, ⌥7. *Expect:* "[", "]", "@", "~", "|"
  are typed, and ⌥B / ⌥E / ⌥J (which type nothing ASCII there) still toggle
  the panels; ⌥1–⌥9 switch tabs only where they don't type a character.
  Switch back to US: ⌥B / ⌥E / ⌥J / ⌥N / ⌥1 are shortcuts again.
- [ ] **M15. Trackpad and mouse.** Two-finger scroll in the grid (5,000 rows),
  the explorer and the editor, with momentum; drag-select text in the
  editor; drag tabs and connections to reorder them.
- [ ] **M16. An input method (IME).** With Japanese or Chinese input, type
  in the editor and a filter field. *Expect:* the candidate window sits at
  the caret and the composed text lands once.

## Look and window chrome

- [ ] **M17. Traffic lights and title bar vs the Tauri app** (§11 item 24).
  Open both apps side by side at the same window size. *Expect:* the
  traffic lights sit at the same place (GPUI (14, 16) = Tauri (14, 22));
  dragging the title bar moves the window, double-clicking it zooms.
- [ ] **M18. Overall look** against `dbclient/docs/images/boa.png` and the
  running Tauri app: the remaining differences are listed in
  [parity-audit.md](parity-audit.md) §4. Say which ones matter to you.

## Data and migration

- [ ] **M19. CSV export through the native save panel** (§11 item 16).
  Export a table, pick a folder and a name in the save panel, then click
  "Reveal in Finder" in the done toast. *Expect:* the file is there, whole,
  with no `.part` file left; Finder shows it selected.
- [ ] **M20. The Tauri app reads what the GPUI app wrote** — **real install**
  (§11 item 25). The automated test (`crates/boa-app/src/tests/migration.rs`)
  and a rehearsal on a copy of your data show the files keep the Tauri
  format, but the Tauri app itself has never been opened on them.
  1. Back up the config dir (above). Quit the Tauri app.
  2. Run the GPUI app on the real dir (a bundled Boa.app from
     `scripts/bundle.sh`, or `cargo run -p boa-app` without
     `BOA_CONFIG_DIR`). Edit a data source (e.g. its colour), run a query,
     quit.
  3. Open `/Applications/Boa.app` (Tauri 0.6.2). *Expect:* every data
     source is listed with the new colour and connects without asking for a
     password; Query History shows the query run in the GPUI app.
  4. Not expected to carry back: Settings, tabs and CSV options changed in
     the GPUI app (the Tauri app reads its own localStorage, which still
     holds the values from before the import; see USAGE "Where your data
     lives").
  Note: your real config dir already holds files the GPUI app wrote
  (`prefs.json`, `session.json`, `.localstorage-imported` dated
  2026-11-26 12:42), so a GPUI build has already run on it once.

## Updates and release (with the lead; ask before each step)

- [ ] **M21. The update bridge with the real keys** (§11 item 21). Follow
  [RELEASING.md](RELEASING.md) "What the user must run": build and sign
  with the real updater key, serve the release locally, and run
  `bridge-rehearsal` on a *copy* of the installed Tauri Boa.app. *Expect:*
  the Tauri updater accepts the GPUI build, swaps it in and relaunches into
  the GPUI app with your data.
- [ ] **M22. In-app update GPUI → GPUI**, once "Restart to update" is wired
  (P1-4): the pill appears, asks first with a transaction open, and the tabs
  come back after the relaunch.
- [ ] **M23. Signed, notarized, both architectures, macOS 11** (§11 item 26):
  `spctl -a -vv Boa.app` says "Notarized Developer ID"; the app starts on
  an Intel Mac (or under Rosetta) and on macOS 11 if one is available.

## Results

| Check | Date | Result / notes |
|---|---|---|
| | | |
