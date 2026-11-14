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

Settings are saved in the app config dir (`connections.json`), passwords
too, in plain text for now (the file is owner-only on Unix).

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
paging or closing a tab with pending changes asks first.

## Transactions and guard rails

Each database session (shared by every console and table tab on that
database) is in **Tx: Auto** (each statement commits) or **Manual** mode (a
transaction is opened before the first statement and stays open until
Commit / Rollback: ⌘⇧↵ / ⌘⇧⌫, or typed `commit` / `rollback`). After every
statement the backend probes the server for the real state (idle, open,
aborted: 25P02 until rolled back), shown in the console toolbar and the
status bar; typed `begin` works in either mode. Disconnecting, reconnecting
(after editing the data source) or quitting with a transaction open asks
first, and rolls it back.

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

## Session restore

On launch Boa reopens the last session: the open tabs, each console's SQL,
the active tab and the panel layout.
