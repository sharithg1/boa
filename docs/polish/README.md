# Visual polish (p10/visual)

Fixes for the P2 "look" list of [docs/parity-audit.md](../parity-audit.md) §4.
Every capture is a real window (`screencapture -l`) of a dev build
(`--features screenshot`, scratch `BOA_CONFIG_DIR`, `BOA_DEV_CONNECTION=1` with the
connection tagged Production), driven through `BOA_DEV_INPUT`, with the new
`front` command bringing the window forward first (an occluded window draws no
frames, so `screencapture` showed a stale picture). 1280 × 820 at 2×, the same
size and scale as the source captures in [docs/audit/src/](../audit/src/).

| Screen | Before | After | Source (Tauri UI) |
|---|---|---|---|
| Console, statement outlined, results | [before-console.png](before-console.png) | [after-console.png](after-console.png) | [main-console.png](../audit/src/main-console.png) |
| Table tab with staged edits | [before-table.png](before-table.png) | [after-table.png](after-table.png) | [table-edits.png](../audit/src/table-edits.png) |
| Settings › General | [before-settings.png](before-settings.png) | [after-settings.png](after-settings.png) | [settings.png](../audit/src/settings.png) |
| New data source dialog | [before-dialog.png](before-dialog.png) | [after-dialog.png](after-dialog.png) | [connection-dialog.png](../audit/src/connection-dialog.png) |

`before-console.png` is from the same build as the others but its connection
was still named "Local dev".

## The root cause of most of the list

gpui-component's `Root` sets the window's rem to `Theme::font_size`, which is
Boa's 13 px body size. Every rem-based GPUI helper and every gpui-component size
therefore comes out at 13/16 of its Tailwind namesake: `gap_3()` is 9.75 px,
`px_2()` 6.5 px, a medium `Input` or `Button` 26 px tall with 11.4 px text,
an `xsmall` icon 9.75 px. Views ported class by class looked "tighter" and
"smaller" for that reason. The polished views now write Tailwind values in px
(`gap-3` → `.gap(px(12.))`); `theme.rs` documents this.

## Fixes (audit §4 item → what changed)

1. **Editor gutter.** 56 px with a 1 px `editor-divider` line, numbers ending
   20 px before it, the text 12 px after it (column 0 at 68 px, as Workspace.tsx's
   `w-14 border-r pr-5` + `pl-3`). Test: `the_text_starts_after_the_56_px_gutter`.
2. **Statement outline.** One box per line around that line's own text, 3 px
   padding both sides, blank lines one cell wide (`rangePath`), instead of
   DataGrip's shape. Separate commit, see "Deviations" below.
3. **"Tx: Auto".** Label then a 12 px chevron, 12 px text in `text-quiet`,
   foreground on hover (`tool_menu_trigger`).
4. **Default-schema picker.** Same trigger, label in Geist Mono, chevron after.
6. **Explorer "+".** 16 px plus in `text-muted-foreground` on a 24 px trigger.
7. **Table toolbar.** Preview… / Submit are `size="xs"` buttons (24 px, 12 px
   medium text; they were 16 px tall with 9.75 px text); the Export icon is 14 px
   in `text-quiet` (it was 9.75 px, white); toolbars' `px-2` is 8 px.
8. **WHERE / ORDER BY labels.** `text-2xs` (11 px), not 10 px.
10. **Settings.** Header block, section labels and cards at the source's
    positions (spacing in px, line heights from the type scale); choice
    triggers are the source's 28 px faint-bordered SelectTrigger with a normal
    weight value and a small 50 % chevron.
11. **Connection dialog.** 36 px inputs and selects, 12 px form gaps, the
    dialog surface (centred, `bg-popover`, `rounded-xl`, 18 / 16 px padding,
    its own 16 px close X), 30 px `size="md"` footer buttons, `text-soft`
    checkbox labels, ringed colour swatches. Its height is 632 px like the
    source's (539 before).

Checked side by side after the change: the console toolbar, the editor's
gutter and outline, the table toolbar and filter bar, the Settings page and the
dialog line up with the source captures within 1–3 px.

## Deviations

- The outline commit replaces the DataGrip shape that 218a215 / d24e6f7
  introduced on purpose (every line but the last as wide as the widest). The
  audit and the Tauri app trace each line; the commit stands alone and can be
  dropped to keep DataGrip's shape.

## Remaining differences

- **Title bar "…"** (audit item 5) is smaller than the source's 16 px Ellipsis,
  and the "…" menu's Disconnect is not styled destructive. Both are in
  `title_bar.rs`, which holds the user's own change; not touched.
- **Text rendering.** GPUI's text is a little heavier than Chrome's at the same
  size and weight (most visible in inputs and labels); not a style setting.
- **Checkbox boxes** are 14.6 px (gpui-component's large size; its sizes are rem
  based), not 16 px.
- **Other gpui-component dialogs** (guard dialogs, password prompt, SQL
  preview, discard prompt) still sit a tenth of the window down on the window
  background; `connection_dialog::dialog_surface` gives any of them the
  source's surface.
- **Other views ported with rem-based helpers** (sidebar, title bar, bottom
  panel, status bar, palette, history, …) still come out at 13/16 where they
  used them; the sidebar toolbar's icons, for example, sit ~1.5 px further left
  than the source's.
- The PROD tag reads bolder than the source's (`conn_tag.rs`), and the
  connection status dot has a dark ring the source does not.
- The `tracking-wide` letter spacing of the WHERE / ORDER BY labels is not
  available in GPUI.
- The settings switches sit ~2 px left of the source's.
- Traffic lights: still only checked in GPUI (the browser preview has none).

---

# Rem pass in the remaining views (p11/rem-polish)

The views the visual pass above did not own: the explorer, Database Output,
status bar, palette, query history, right panel, scope picker, guard dialogs,
password prompt, SQL preview, CSV export, toasts, JSON viewer, value editor and
the grid's chrome. Captures as above (`screencapture -l` of a dev build,
1280 × 820 at 2×, `BOA_DEV_CONNECTION=1` tagged Production, driven through
`BOA_DEV_INPUT`); "before" is `main` at 6b496aa, "after" this branch. These
were taken before the switch to `BOA_DEV_BACKGROUND=1` / offscreen `shot`
(CLAUDE.md); later checks should use those.

| Screen | Before | After | Source |
|---|---|---|---|
| Password prompt | [rem-before-prompt.png](rem-before-prompt.png) | [rem-after-prompt.png](rem-after-prompt.png) | ConnectionDialog.tsx `PasswordPrompt` |
| "Run a destructive statement on production?" | [rem-before-guard.png](rem-before-guard.png) | [rem-after-guard.png](rem-after-guard.png) | GuardDialogs.tsx |
| SQL preview of staged edits | [rem-before-preview.png](rem-before-preview.png) | [rem-after-preview.png](rem-after-preview.png) | TableEdits.tsx `PreviewDialog` |
| Settings (switches) | [rem-before-settings.png](rem-before-settings.png) | [rem-after-settings.png](rem-after-settings.png) | [settings.png](../audit/src/settings.png) |
| Console with results, explorer, status bar | [rem-before-main.png](rem-before-main.png) | [rem-after-main.png](rem-after-main.png) | [main-console.png](../audit/src/main-console.png) |
| Query history overlay | [rem-before-history.png](rem-before-history.png) | [rem-after-history.png](rem-after-history.png) | QueryHistory.tsx |
| Output log | [rem-before-output.png](rem-before-output.png) | [rem-after-output.png](rem-after-output.png) | BottomPanel.tsx |
| JSON viewer | [rem-before-json.png](rem-before-json.png) | [rem-after-json.png](rem-after-json.png) | JsonViewer.tsx |

## What changed

1. **Rem helpers → px.** Every rem-based spacing / size helper left in these
   views (147 of them: `gap_2()`, `px_3()`, `pl_1()`, `size_N()`, …) is now
   its Tailwind value in px. The ports were class by class, so this moves each
   one from 13/16 of its value to the value itself (the explorer toolbar's
   `px-2`, the Output tabs' `pl-2` / `pr-1`, the log's `px-3 py-2`, the
   palette and history rows, the viewers' headers, the grid's cell padding…).
   Spinners are the source's `size-3` / `size-3.5` (they were 9.75 px).
2. **Dialog surface.** The guard dialogs, the password prompt and the SQL
   preview use `connection_dialog::dialog_surface` (centred, `bg-popover`,
   `rounded-xl`, 18 / 16 px padding). Footer buttons are `size="md"` (30 px,
   `md_button`); the guards' title and description sit 6 px apart
   (AlertDialogHeader `gap-1.5`) and Run / "Roll back and …" are solid
   `bg-destructive` with white text (gpui-component's danger button is a
   tint). The prompt has the 36 px input, a 16 px checkbox and dialog.tsx's own
   16 px close X. The preview is `p-0`: header `py-3 pl-4 pr-12`, the list
   full-bleed on `bg-editor`, footer `px-4 py-2.5` with `size="xs"` buttons
   (24 px, 12 px icons) and the X at `top-4 right-4`.
3. **Checkboxes 16 px.** `views::checkbox` lays gpui-component's `Checkbox` out
   under a 16 px rem (`WithRem`, test
   `with_rem_lays_its_child_out_with_a_16_px_rem`): a 16 px box, 8 px gap,
   12 px check. Used by the password prompt and the connection dialog.
4. **Settings switches.** The Tauri switch is not the 36 × 20 its classes
   suggest: switch.tsx's `data-[size=default]:w-8 h-[1.15rem]` win, so it is
   32 × 18.4 and its thumb sticks out 2 px when on. Measured in the source
   capture and now identical (track x 2224–2277, thumb 2260–2291 at 2×).
5. **PROD tag** at medium weight (semibold in the source; GPUI's text is
   heavier, see the crops above), its lock 4 px away.
6. **Explorer context menus** use `chrome::menu_row` like the "+" menus.

## Remaining differences

- **Context-menu rows** are 28 px (`menu_row`, the dropdown size); context-menu.tsx's
  items are 26 px. The grid's context menu still uses gpui-component's stock
  (rem-sized) rows: converting it means rebuilding its action / shortcut rows.
- **The "Discard N pending changes?", "Delete connection?" and "Clear history"
  confirms** are gpui-component `AlertDialog`s and still have its surface
  (not in this pass's files, and `AlertDialog` has no surface override).
- **`chrome::vsep`** (`mx_1`, 3.25 px) and other shared helpers in
  `chrome.rs` were left alone (not owned here).
- **Line heights.** Where a view sets a text size without a line height, GPUI's
  default (≈1.6) is used instead of the type scale's; the explorer, dialogs and
  settings set them, some panel rows do not (±1–2 px per row).
- **Checkmark** is 12 px (gpui-component's medium mark); checkbox.tsx's is
  `size-3.5`.
- **`tracking-wide`** on the PROD tag is not available in GPUI.
- Not compared against a source capture (none exists): the guard dialogs, the
  prompt, the preview, history, the JSON viewer and the value editor were
  checked against their TSX classes only.
