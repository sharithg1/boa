# Using Boa

How each part of Boa behaves, in detail. For what it is and how to install
it, see the [README](../README.md).

## Layout

Panels follow DataGrip: a Database Explorer tree (connection › database ›
schema › tables › columns), the editor (query consoles and table data
editors), a full-width Database Output panel (result grids + Output log, ⌥J)
and breadcrumbs in the status bar.

## Data sources

Boa supports **PostgreSQL only** for now. Add a data source (⌘O, or + in the
explorer; a `postgresql://` URL fills the form), connect, browse schemas /
tables / columns, open table data and run statements (⌘↵) from a console.
Like DataGrip, only picked databases and schemas are shown and introspected:
click the "N of M" badge on a connection or database (defaults: the
connection's database, the `public` schema).

Under each table, after its columns, the explorer has DataGrip's **keys**,
**foreign keys**, **indexes** and **checks** folders (empty ones are left
out), read with the rest of the schema (one catalog query each, so large
schemas stay fast). Primary-key columns show a yellow key, foreign-key
columns a blue one. Right-click an entry to copy its name or definition.

**DDL** (table toolbar, or "Show DDL" on a table in the explorer) opens the
server's view of the relation in a new console: `CREATE TABLE` with column
types, defaults, identity / generated columns, `NOT NULL` and constraints
(`pg_get_constraintdef`), then the indexes that aren't part of a constraint
(`pg_get_indexdef`), triggers and `COMMENT ON`s; views and materialized
views come from `pg_get_viewdef`. Ownership, grants, storage options and
column collations are left out. Needs PostgreSQL 12 or later.

In a console, completion after `JOIN t ON` offers the conditions of the
foreign keys between `t` and the tables before it (`o.user_id = u.id`).

Settings are saved in the app config dir (`connections.json`), passwords
too, in plain text for now (the file is owner-only on Unix).

### SSH tunnel

Like DataGrip's SSH/SSL tab, a data source can be reached through an SSH
server: tick **SSH tunnel** in the data source dialog and give the SSH host
(a host name, IP or `~/.ssh/config` alias), and optionally the port (default:
22 or the config's `Port`), the user (default: the config's `User` or your
login name) and the authentication: the **SSH agent** (plus the keys ssh finds
itself, e.g. `IdentityFile` in `~/.ssh/config`) or a **key file**. The
Host / Port above are then the database as the SSH server sees it
(`localhost`: the SSH server itself; a Unix socket directory works too).

Boa runs the system `/usr/bin/ssh -N -L 127.0.0.1:<free port>:<db host>:<db port>`
in batch mode, so `~/.ssh/config` (aliases, `ProxyJump`), the agent and
`known_hosts` all apply, but nothing is prompted for: the host key must
already be in `known_hosts` (connect once with `ssh` in Terminal), and a key
with a passphrase must be in the agent (`ssh-add --apple-use-keychain`). No
SSH password or passphrase is stored. ssh's own error (host key verification
failed, permission denied, …) is shown when the tunnel can't be opened.

One tunnel per connected data source carries all its sessions (one per
database), Cancel and Test connection (which opens a tunnel of its own). It
is closed on disconnect, when connecting fails and at quit; if ssh exits
while connected, the next statement reopens it. TLS (the SSL setting) still
runs end to end with the database, and Verify full checks the database's
host name, not the tunnel's `127.0.0.1`.

## Editing table data

Table data is editable like DataGrip's data editor, for tables with a
primary key (views, key-less tables and console results stay read-only and
say why). Double-click, Enter or start typing to edit a cell (⇧↵ opens a
larger editor for long text / JSON), ⌘⌥N sets NULL (not the same as an
empty string), ⌘N adds a row (untouched columns are left out of the
`INSERT`, so defaults apply), ⌘⌫ deletes the row, ⌘⌥Z reverts; also from the
right-click menu.

Changes are only staged, tinted per kind and kept per tab, until Submit: the
preview (⌘↵) shows the exact statements (`UPDATE` / `DELETE` by the row's
original key, `INSERT`), which then run in one transaction (or a savepoint
inside an open one). Each `UPDATE` / `DELETE` must hit exactly one row; any
error rolls everything back and points at the failing statement. Reloading,
paging, changing the filter or sort, or closing a tab with pending changes
asks first.

## Filtering and sorting table data

The bar above a table's data has a **WHERE** and an **ORDER BY** field, like
DataGrip's: type a condition (`status = 'active' and id > 10`) or an
ordering (`created_at desc, id`) and press Enter; empty a field (or click its
×) and press Enter to remove it, Escape puts back the applied text. They run
on the server, as `select * from <table> where … order by …`, so they apply
to every row, not just the loaded page, and the data starts again from the
first page.

Clicking a column header sorts by it: ascending, descending, then off. It
rewrites the ORDER BY field (a column already in it changes in place, another
one replaces the sort; ⇧-click adds it after the others), and the header
shows an arrow, with its position when sorting by several columns.

A bad condition shows the server's error in place of the rows, keeping what
was typed. Each field must be one clause of one statement: a `;` outside a
string, unbalanced parentheses or an unterminated string or comment are
refused before anything runs. The filter and sort are kept per tab (and
restored with the session); Export and "Open query console" use the filtered,
sorted statement.

## Selecting and copying rows

In any result grid or table data, click a cell, then drag, ⇧-click or
⇧-arrow for a rectangular range; click a row number for the whole row, drag
or ⇧-click for a run of rows, ⌘-click to add or remove one; ⌘A selects every
loaded row, Esc goes back to the single cell. ⌘C copies the selection as
tab-separated values (a single cell: just its text).

The right-click menu's **Copy as ›** writes the selection as TSV (with or
without a header), CSV, a Markdown table, JSON (an array of objects keyed by
column name), SQL `INSERT` statements, SQL `UPDATE` statements (table data
of tables with a primary key: sets the selected non-key columns, `WHERE` by
the row's key) or an SQL `IN` list (`'a', 'b', 'c'`; one column only). The
values are the ones shown, pending edits included. Column types decide the
quoting: NULL is an empty field in TSV / CSV, `NULL` in Markdown and SQL,
`null` in JSON; numbers and booleans are bare in SQL and JSON; json / jsonb
values are embedded as JSON, Postgres arrays become JSON arrays; strings are
escaped per format (`'` doubled in SQL, `|` and line breaks in Markdown, CSV
quoting per RFC 4180). `INSERT`s from table data target its table; from a
console result they target `MY_TABLE`, to be replaced.

## Transactions and guard rails

Each database session (shared by every console and table tab on that
database) is in **Tx: Auto** (each statement commits) or **Manual** mode (a
transaction is opened before the first statement and stays open until
Commit / Rollback: ⌘⇧↵ / ⌘⇧⌫, or typed `commit` / `rollback`). After every
statement the backend probes the server for the real state (idle, open,
aborted: 25P02 until rolled back), shown in the console toolbar and the
status bar; typed `begin` works in either mode. Disconnecting, reconnecting
(after editing the data source) or quitting with a transaction open asks
first, and rolls it back. A transaction lost with a dropped connection is
reported (see [Staying connected](#staying-connected)).

Destructive statements (UPDATE / DELETE without WHERE, TRUNCATE, DROP,
ALTER TABLE … DROP COLUMN; found with the editor's tokenizer, so comments,
strings and CTEs don't fool it) ask for confirmation; Settings can turn that
off, except on connections tagged Production.

A data source can be marked read-only (enforced by the server:
`default_transaction_read_only`, and put back if a statement turns it off)
and given an environment tag and a colour, shown on the explorer row, the
editor, the console toolbar and the status bar.

## Query history

⌘⌥E, the console toolbar's history button, or "Query History" in ⌘K. Every
statement run from a console is saved per connection in the app config dir
(`history-<connection id>.jsonl`, JSON lines, owner-only; the newest 5,000
kept; deleted with the connection), failed ones with their error. Paging
through a result does not add entries.

Search matches words in the SQL as you type, with an errors-only filter and
a toggle for all connections; ⏎ inserts the statement at the console's
caret, ⌘⏎ opens it in a new console on its database, ⌘C copies, ⌘⌫ deletes.
Entries can be deleted one by one or cleared, and recording turned off in
Settings → Queries. The Output log (⌥J) stays a session-only log.

## CSV export

The Export menu (download icon) on a result grid or table toolbar, or
"Export to CSV…" on a table / view in the explorer, asks for a file and
writes the whole result, not just the loaded page: the server streams
`COPY (<statement>) TO STDOUT WITH (FORMAT csv)` into a temporary file that
is renamed into place when done (progress and Cancel in the status bar; a
toast and an Output log line at the end).

Only read-only statements are re-run this way, in their own transaction, or
under a savepoint that is rolled back when the session is inside a user
transaction (so its uncommitted rows are included and a failure leaves it
intact). Results of statements that change data (`INSERT … RETURNING`) are
never re-run: their loaded rows are written instead.

The same menu copies the loaded page as CSV and sets the delimiter (comma,
tab, semicolon) and header row. NULL is an empty field, an empty string
`""`, as in Postgres's COPY.

## Updates

Boa checks for a new release shortly after launch and every few hours, and
downloads it in the background. When it is ready, the status bar shows
**Restart to update to X.Y.Z**; Boa asks first if a transaction is open, as
on quit, and your tabs come back after the restart. **Boa → Check for
Updates…** checks right away, and **Boa → About Boa** shows the version
(with **Copy version info** for bug reports).

## Session restore

On launch Boa reopens the last session: the open tabs, each console's SQL,
each table tab's filter and sort, the active tab and the panel layout.
It reconnects every data source that was connected at quit (the selected
one first; the others skip a password prompt), retrying for about half a
minute while the server can't be reached (VPN not up yet, network still
coming back).

## Staying connected

A connected data source stays connected until you disconnect it:

- **Dropped connections** (server restart, sleep, Wi-Fi or VPN change) are
  noticed within about a minute (TCP keepalives), and the session is reopened
  on next use. A read (`SELECT`, `TABLE`, `SHOW`, other pages, the explorer)
  that hits a dropped connection is re-run on a new one automatically; a
  statement that writes is not, and its error says the connection was lost.
- **Server away:** the data source shows as *reconnecting* (amber) and is
  retried with growing delays for about two minutes, then as *failed* with a
  Retry toast. Waking the machine or the network coming back retries at once.
- **Lost transactions:** if a session drops with a transaction open, the
  server rolls it back. Boa says so ("Transaction rolled back") and does not
  run the next statement as if the transaction were still there.
- **Acting on a closed data source** connects it first: running a statement,
  selecting one of its databases, or the **Connect** button on its table tabs.
