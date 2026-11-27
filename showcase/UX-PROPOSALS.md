# Boa UX improvement proposals

These proposals come from reviewing all 14 screenshots in the [showcase](README.md). They concern visual design, workflow clarity, and presentation. Runtime performance, keyboard behavior, accessibility, and database correctness were not tested.

Boa's strongest product story is making database changes visible and deliberate. Staged edits, exact SQL previews, production indicators, and visible transaction state already support that story. The next design pass should make the existing functionality easier to understand and give the active task more space.

## Implementation progress

The first implementation pass covers panel defaults and the table toolbar, plus the filter and row-count wording:

- Fresh workspaces show the explorer with Structure and output closed. Saved panel visibility and sizes still take precedence. Query outcomes retain their existing behavior of opening output.
- Pending changes appear in a separate **Review / Apply / Discard** bar. Discard explicitly removes all pending changes in the current table tab; the previous curved-arrow control also discarded all changes rather than undoing one edit.
- The browsing toolbar wraps at narrow widths, and **Find in loaded rows** keeps a minimum width of 200 pixels. Query timing and row counts move to a footer below the grid.
- The footer distinguishes **500 loaded** from **~100,000 in table**.
- Add, delete, and pending-change actions are disabled while a submission is in progress.

Offscreen captures: [fresh workspace](ux-review/empty-workspace.png) · [staged edits](ux-review/staged-edits.png). The captures use temporary settings and local development data; staged edits were not submitted.

The app test suite includes a layout regression check at 720, 1280, and 1600-pixel window widths, ensuring that staging edits leaves the browsing toolbar and search field in place. Session restoration and panel-toggle tests cover the new defaults.

Validation: formatting and Clippy (all workspace targets and features, warnings denied) pass. The workspace test run against local PostgreSQL passes with 713 tests and 3 ignored tests.

Remaining work includes transaction and destructive-action labels, the richer review-dialog summary, contrast and metadata explanations, export-scope wording, and the showcase recording. The more nuanced behavior of respecting a deliberately hidden output panel during query completion is also still a proposal; this pass preserves existing output-opening behavior.

## Preserve these strengths

- **A coherent editing workflow:** highlighted cell changes, struck-through deletions, staged submissions, and SQL previews make edits reviewable.
- **Integrated production awareness:** environment badges, connection colours, destructive-query explanations, and transaction status reinforce one another.
- **A focused daily workflow:** exploring schemas, writing SQL, inspecting results, and editing rows fit together naturally.
- **A consistent visual style:** restrained colours, readable SQL, and typed grid headers suit sustained use.

## Priorities

| Priority | Proposal | Intended outcome |
| --- | --- | --- |
| P1 | Improve panel defaults | More space for SQL and table data |
| P1 | Stabilize the table toolbar | Controls stay readable as edits accumulate |
| P1 | Clarify filter scope and row counts | Users understand which rows they are searching and counting |
| P1 | Label consequential actions | Commit, rollback, and destructive actions are unambiguous |
| P1 | Strengthen the change-review summary | Users can verify the target and effect before applying edits |
| P2 | Improve secondary text and terminology | Available actions and metadata are easier to interpret |
| P2 | Refocus the showcase | The strongest workflows become clear immediately |

## 1. Give the active task more space

**Observation:** In the [SQL console screenshot](05-sql-console.png), the empty Structure panel occupies roughly a quarter of the window width. The [initial explorer](01-database-explorer.png) shows separate empty areas for the workspace, Structure panel, and output panel. In the table view, the side panels leave limited width for columns.

**Proposed changes:**

- Start with unused auxiliary panels collapsed when there is no saved layout preference.
- Remember explicit panel visibility and sizing choices.
- Reveal output when a query produces results or an error, while respecting deliberate user layout choices.
- Keep panel toggles discoverable so users can reopen Structure or output easily.

**Review criteria:** A fresh workspace gives most of its space to the active task. Empty panels do not dominate the initial view. Restoring a session preserves the user's layout.

## 2. Stabilize the table toolbar

**Observation:** The local filter is truncated in the [table screenshot](03-table-tab.png). In the [staged-edits screenshot](07-staged-edits.png), it shrinks further as the change count and submission controls appear. Pagination, row actions, filtering, and export compete for one line.

**Proposed changes:**

- Keep browsing controls in a stable toolbar.
- Show a separate compact bar while changes are pending, for example:

  **5 pending changes · Review · Apply · Discard**

- Give the filter a useful minimum width.
- Use an overflow menu for secondary actions at narrow window widths.
- Make the scope of Discard explicit: it removes all pending changes for the current table tab. Preserve undo as a separate action.

**Review criteria:** Staging edits does not squeeze the filter or move unrelated browsing controls. Primary actions remain readable at the smallest supported window size.

## 3. Make filter scope and row counts explicit

**Observation:** The UI offers both a local filter and server-side `WHERE` conditions. Their different scopes are explained in the showcase, but are less obvious in the interface. In the [filtered table screenshot](04-filter-and-sort.png), `~100,000 total` could be interpreted as the number of matching rows.

**Proposed changes:**

- Label the local filter **Find in loaded rows**.
- Keep `WHERE` and `ORDER BY` visibly associated with database filtering and sorting.
- Distinguish loaded rows from the estimated table size, for example:

  **500 loaded · ~100,000 in table**

- If an exact matching count is unavailable, avoid presenting the table estimate as a filtered result count.
- Make export scope clear when local search and server filtering are both active.

**Review criteria:** Users can tell whether an operation affects the loaded page or the database query. Row-count labels remain accurate with a server filter applied, without requiring an additional expensive count query.

## 4. Use words for consequential actions

**Observation:** In the [manual transaction screenshot](12-manual-transactions.png), commit and rollback use a checkmark and curved arrow. A curved arrow also represents undo in the table workflow. The [destructive confirmation](11-destructive-guard.png) explains the consequence clearly, but its primary button says only Run.

**Proposed changes:**

- Use visible **Commit** and **Roll back** labels alongside their icons.
- Keep transaction actions visually distinct from undoing a staged cell edit.
- Replace generic confirmation labels with the actual action where practical, such as **Delete all rows** for a `DELETE` without `WHERE`.
- Preserve the existing connection, database, environment, and consequence text in destructive confirmations.

**Review criteria:** Users can identify transaction and destructive actions without relying on icon interpretation or tooltips.

## 5. Strengthen the change-review summary

**Observation:** The [submit preview](08-submit-preview.png) shows exact SQL and names `public.orders`. It does not prominently identify the connection, database, and environment within the dialog. Its five statements also require reading the SQL to understand the mix of operations.

**Proposed changes:**

- Show the connection, database, environment, and target table inside the review dialog.
- Add a plain-language summary, such as:

  **3 updates · 1 deletion · 1 insertion**

- Retain the exact SQL and the Copy and Open in console actions.
- Explain the transaction outcome in terms of the active mode. In manual mode, make it explicit that applying the changes leaves them awaiting commit.
- Keep the distinction between applying staged edits and committing a transaction consistent across the dialog, toolbar, and status bar.

**Review criteria:** Users can verify the destination, operation mix, and commit behavior without inspecting the background UI or inferring them from SQL.

## 6. Improve secondary text and terminology

**Observation:** Toolbar icons, metadata, and placeholders often use similarly dim tones. Available actions can resemble disabled controls. Labels such as `1 of 4`, `Tx: Auto`, and nullable `?` require interpretation.

**Proposed changes:**

- Increase contrast selectively for available actions and useful metadata.
- Give disabled controls a distinguishable treatment.
- Explain scope badges with wording or tooltips such as **1 of 4 databases shown** and **1 of 7 schemas shown**.
- Consider **Auto-commit** and **Manual** as clearer transaction-mode labels.
- Explain the nullable `?` convention in the Structure panel, or show an explicit nullable property where space permits.

**Review criteria:** Available and disabled actions are distinguishable. Compact metadata has an accessible explanation. Check the result at actual application size as well as in screenshots.

## 7. Lead the showcase with the strongest workflow

**Observation:** The annotated tour is useful documentation, but the opening explorer views give significant space to empty panels. Yellow annotations sometimes cover the details they explain, including the production colour rule.

**Proposed changes:**

- Lead with staged edits, SQL review, and production safeguards.
- Keep the complete annotated tour as supporting documentation.
- Add clean screenshots that show the interface without annotation overlays.
- Add a short recording of one complete workflow: open a table, change a value, review the generated SQL, and apply the change.
- Move annotations away from the actual indicators they describe.

**Review criteria:** The opening visuals communicate Boa's value quickly. Readers can inspect the real interface clearly and then use the annotated tour to learn the details.

## Suggested implementation order

1. Adjust panel defaults and table-toolbar layout together, since both determine available workspace.
2. Clarify filter scope, row counts, and action labels.
3. Update the change-review dialog and manual-transaction wording.
4. Refine contrast and compact metadata explanations.
5. Refresh screenshots and record the complete editing workflow after the UI changes settle.

Validate the changes in the running app at both narrow and wide window sizes, with empty and populated panels, pending edits, active filters, and both transaction modes. Screenshot review alone cannot establish whether the revised interactions work well.
