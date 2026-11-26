# Phase 0: go/no-go summary

Spike of the two hardest pieces (SQL editor, result grid) on the real backend.
Details: [widgets.md](widgets.md) (GPUI / widget-library decision),
[numbers.md](numbers.md) (startup, memory vs. Tauri), [editor.md](editor.md),
[grid.md](grid.md). Screenshot of both panes together: [integrated.png](integrated.png).

## Result: go

- **boa-core** builds and tests without GPUI: the Tauri backend with the Tauri
  layer removed (behaviour and on-disk formats unchanged), an executor-agnostic
  `Backend` bridge (own tokio runtime, futures + async-channel events), and the
  TS logic ported with the same test cases (`sql`: tokenizer, statements,
  completion, guards; `grid`: values, format, CSV, copy-as, staged edits, table
  queries, connection tags). One set of model types. 189 tests incl. the live
  Postgres tests. The SQL port was cross-checked against the TS original on
  ~600k calls with zero differences.
- **Widgets:** gpui-component 0.11.0 (Apache-2.0) for the chrome and its `Editor`
  for the console; our own grid on `uniform_list`. GPUI via `gpui-pre =0.3.14`
  (the exact snapshot gpui-component pins), `runtime_shaders` so building needs
  only the Command Line Tools. No GPL anywhere in the lock file.
- **Editor:** highlighting from the ported tokenizer, schema-aware completion
  from the live catalog (incl. FK join conditions), ⌘↵ runs the statement under
  the caret / the selection, statement outline, undo/redo, IME via the input
  handler. Weak spot: the completion menu is not customisable (text badges
  instead of icons; Tab semantics differ). ~1–1.5 weeks to console parity.
- **Grid:** 100k rows (and 100k × 100 columns) at a steady 120 Hz with 2–4 ms
  CPU per frame; ~90 ms from run to first paint for 100k × 7 (release).
  Cell / range / row selection, ⌘A, Esc, ⌘C TSV, column resize, inline edit
  staged into `Pending` with tints. Weak spot: memory with huge pages
  (100k × 7 = 168 MB footprint; fixable with compact row storage). ~2–2.5 weeks
  to grid parity.
- **Startup / memory** (release, idle, empty workspace): window on screen
  ~160 ms (Tauri ~165 ms, before its WebView has painted); footprint 62 MB vs.
  83 MB for Tauri incl. its WebKit helpers.

## Not verified (no Screen Recording / Accessibility permission for the agents)

Real trackpad scrolling feel, mouse selection in the editor, a real IME,
popup placement at window edges, traffic-light position against the Tauri
app. Everything visual was checked through `render_to_image` screenshots
(`--features screenshot`) and interaction through `VisualTestContext` tests.
