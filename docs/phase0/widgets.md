# Phase 0: GPUI dependency and widget library

Decision: **depend on gpui-component 0.11.0 for the chrome and build the SQL
editor on its `Editor`. Build the results grid ourselves on GPUI's
`uniform_list`.** Everything below was checked against the published crate
sources (`~/.cargo/registry/src/*/gpui-component-0.11.0`, `gpui-base-0.11.0`,
`gpui-pre-0.3.14`) and a clone of https://github.com/longbridge/gpui-component
(HEAD `6d7004f`, 2026-11-26) at `/Users/sharith/Desktop/repos/gpui-component`.

## 1. How we depend on GPUI

| Option | Verdict |
|---|---|
| `gpui` on crates.io | Latest is **0.2.2**: an old snapshot. No gpui-component release builds on it any more. |
| git dependency on a Zed commit | Works, but gpui-component won't build against an arbitrary Zed commit. Two gpuis in one graph would conflict. |
| **`gpui-pre` =0.3.14** (crates.io snapshot of `zed@f126950`) | **Chosen.** gpui-component 0.11.0 pins exactly this (`gpui = { package = "gpui-pre", version = "=0.3.14" }`), it is on crates.io (no git deps), Apache-2.0. |

Root `Cargo.toml` `[workspace.dependencies]`:

```toml
gpui = { package = "gpui-pre", version = "=0.3.14" }
gpui_platform = { package = "gpui-pre-platform", version = "=0.3.14", default-features = false, features = ["font-kit", "runtime_shaders"] }
gpui-component = { version = "=0.11.0", default-features = false }
gpui-component-assets = { package = "gpui-kit-assets", version = "=0.11.0" }
```

- The rename keeps `use gpui::…` and `#[gpui::test]` working as in Zed's code.
- `runtime_shaders` compiles the Metal shaders at launch, so building needs only
  the Command Line Tools. Release builds could switch to precompiled shaders
  once CI has full Xcode (it shortens the cold first launch, see numbers.md).
- `gpui_platform::application()` creates the app (in this snapshot, the platform
  is a separate crate, not part of `gpui`).
- **Upgrade rule:** bump gpui, gpui_platform and gpui-component together, to the
  gpui-pre version that the gpui-component release pins. They have changed the
  pin in almost every release (gpui-pre 0.3.1 → 0.3.14 across 0.6–0.11).
- The project is being renamed "GPUI Kit": the `gpui-kit` umbrella crate re-exports
  everything as `gpui_kit::*`. We depend on the underlying crates directly so our
  code reads as plain gpui. Expect renames on upgrade.

Built here (M5 Pro, CLT only): our skeleton and gpui-component's `hello_world`,
`story` and `example-editor` (tree-sitter languages) all build. A clean build of
the three examples takes 1 min 44 s.

## 2. Licences

All Apache-2.0: gpui-component, gpui-base, gpui-component-macros, gpui-kit-assets
(icons are Lucide, ISC), and every gpui-pre crate. Their direct dependencies are
permissive (ropey, lsp-types, markdown, notify CC0, jiff, rust-i18n, similar, objc2).
A scan of the lock file found no GPL or AGPL crates. `option-ext` is MPL-2.0
(file-level copyleft, fine to use unmodified). The ICU crates are Unicode-3.0. Run
`cargo deny` in CI to keep this true.

Fonts: Geist / Geist Mono v1.9.2 static TTFs (SIL OFL 1.1, `assets/fonts/OFL.txt`).

## 3. Component review

### SQL editor: gpui-component `Editor`. Use it, behind our own wrapper.

| Need | Status | How |
|---|---|---|
| Multi-line, selection, scrolling | yes | `EditorState::new(window, cx)` + `Editor::new(&state)`. Rope buffer (`ropey =2.0.0-beta.1`) |
| Undo / redo | yes | built in, ⌘Z / ⌘⇧Z. `replace_all` keeps the history, `set_value` clears it |
| IME | yes | `EntityInputHandler` with marked-text range |
| Line gutter | yes | `.line_number(true)`, `GutterStyle`, active line number colour |
| No soft wrap, horizontal scroll | yes | `.soft_wrap(false)` |
| **Our own tokenizer for highlighting** | **yes** | `InputHighlighter` trait (`gpui-base/src/input/editor/highlighting.rs`): `update(edit, rope, …)` + `styles(range, resolver) -> Vec<(Range, HighlightStyle)>`. Install with `EditorState::set_highlighter_factory`. No tree-sitter or syntect needed |
| Completion popup | yes, but limited | `CompletionProvider` (`completions(rope, offset, ctx) -> Task<Result<CompletionResponse>>`, `is_completion_trigger`). Items are `lsp_types::CompletionItem` drawn as label + detail. **Rows can't be custom-rendered.** Set `text_edit` for the replace range (`insert_text` alone doesn't replace the typed prefix) |
| ⌘↵ runs statement under caret | yes | ⌘↵ is already `Enter { secondary: true }` → `InputEvent::PressEnter { secondary: true }` (no newline). Caret: `cursor()`, `selected_range()`, `text()` |
| Current-line band, colours | yes | Through the theme only (`theme.highlight_theme.style.editor_*`, `selection`, `caret`). `set_editor_style` is overwritten on render |
| Font | yes | Defaults to `theme.mono_font_family/size` (we set Geist Mono 13) |
| Size limit | ok | Documented as "large text up to about 50K lines". Enough for consoles |

Gaps to accept or work around: no public `scroll_to(offset)` (moving the caret
scrolls), completion rows are label + detail only (no per-kind icons beyond what
the menu draws), the ropey beta pin. Wrap it in our own `SqlEditor` so a later
swap to a hand-written editor stays local.

### Results grid: build our own on `uniform_list`

gpui-component's `DataTable` (`TableDelegate`) virtualizes rows and columns, has
column resize / reorder / fixed columns, custom cells (`render_td` returns any
element, so an inline `Input` works), header sort and row context menus. But:

- **No range selection.** `TableSelection` is `None | Row | Column | Cell`: one
  cell at most. No shift-click or drag extension, no ⌘A, and the selection mode is a private enum.
- **No copy.** `cell_text` / `dump_range` exist for export, but no ⌘C.
- **Untested at our size.** Its tests and benches use 20–5,000 rows, and table test coverage
  is thin (about 15 tests, against about 500 for input).

Boa's grid needs rectangular selection, ⌘C as TSV, keyboard extension and 100k
rows, so adding range selection to DataTable would mean fighting its private
state. A grid on `uniform_list` (fixed row height) is a contained job and can
still reuse `resizable`, `ContextMenuExt`, the scrollbar, the theme colours and
the `Input` for inline edit.

### Chrome: use gpui-component

| Piece | Fit |
|---|---|
| `Root` | **Required** as each window's root view (dialogs, sheets, notifications, popovers live in it). `Root::new(view, window, cx)` |
| Resizable panels | `v_resizable/h_resizable` + `ResizableState` (read `sizes()`, set `resize_panel`) for persisting sizes. Used in the skeleton |
| Dock | `DockArea` with serializable state. More than Boa needs |
| Tree | virtualized, expand/collapse events for lazy loading, context menu: fits the explorer |
| Dialog / alert | `window.open_dialog`, `open_alert_dialog` |
| Popover, Tooltip, Kbd | fine. `Kbd::binding_for_action` for shortcut hints |
| Menus | `ContextMenuExt`, `DropdownMenu`, `PopupMenu`, `NativeMenu` (real NSMenu) |
| Notifications | `window.push_notification`: toasts |
| Tabs, Select, Sidebar, TitleBar | fine. We draw our own title bar (tabs live in it) |
| Theme | `Theme::update(cx, …)` sets any of 136 colours, `font_family`, `mono_font_family`, sizes, radii; JSON themes also possible. Ours is set in code (`theme.rs`) |

## 4. Risks

1. **API churn and lockstep pin.** We can only move GPUI when gpui-component does,
   and both change often (GPUI Kit rename in progress). Mitigation: exact pins,
   upgrade deliberately, keep components behind our own types (`SqlEditor`,
   `ResultGrid`).
2. **Key bindings it installs.** In debug builds ⌘⌥I opens its inspector. Root
   binds tab / shift-tab / ⌘C (copies selected static text). The `Input` context
   has about 86 bindings (⌘F search, ⌘. code actions, ⌘⌥↑/↓ add cursor, …). Our own
   bindings in narrower contexts win. Check for clashes with Boa's shortcut list
   in Phase 1.
3. **Editor colours only through the theme**: per-editor overrides get reset on render.
4. **Completion menu look** is theirs (label + detail). If Boa's custom rows
   (kind badges, FK join hints) matter, we'd need to fork the menu or draw our own popup.
5. **Memory**: GPUI's idle footprint is dominated by Metal buffers, not by
   gpui-component (see numbers.md).

## 5. Recommendation

Go with **gpui-component for chrome + its Editor for the SQL console + our own
grid**. Writing an editor with IME, undo, a gutter and a completion popup on raw
GPUI would take months. The grid is the one place where the library would cost
more than it saves.
