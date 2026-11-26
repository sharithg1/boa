# Performance numbers (HANDOFF-PARITY.md §11, last item)

Machine: Apple M5 Pro (18 cores), 24 GB, macOS 27.2.1, built-in 120 Hz Retina
display. Local Postgres 17 (`boa_dev`: `public.orders` 100k rows × 7 columns).
Measured 2026-11-26 at branch `p9/core-conc` (main at `0e02bc0` + session
concurrency). **Other agents were building on the same machine at the time** (load
average 4–12 on 18 cores), so the startup times below are noisier than Phase 0's.

Builds: `cargo build --release -p boa-app` (thin LTO, `codegen-units = 1`,
stripped, `runtime_shaders`): 20.9 MB binary, used for startup and the empty-app
memory. The same with `--features dev-hooks` for the scripted runs (grid and
typing benchmarks, memory with data); the hooks read env vars once and cost nothing
when unset. Every run used a fresh temporary `BOA_CONFIG_DIR`.

The Tauri numbers are the ones measured in Phase 0 (`docs/phase0/numbers.md`,
Boa 0.6.2 with an empty profile); Tauri was not re-run.

## Summary

| Target | GPUI (now) | Tauri 0.6.2 | Met? |
|---|---|---|---|
| Startup: window on screen after spawn, warm (median of 8) | **~207 ms** (192–269; load ≈ 5). An earlier batch at load ≈ 12: ~237 ms (190–367) | ~165 ms (159–194) — its window shows before the WebView paints | **No** (≈ 40 ms slower on this metric, see notes) |
| Startup: first frame, in-process (from `app::run`) | **~139 ms** (125–159) | n/a | — |
| Idle memory, empty app (15 s after launch) | **65 MB** (3 runs, identical) | 83 MB (app + WebKit helpers) | **Yes** |
| Idle memory, connected, console with a 5,000-row result | **88 MB** steady (4 of 4 runs by 60 s; see the transient below) | not measured (the user's long-running instance: 155 MB) | — |
| Idle memory, connected, 2,000-line console | **119 MB** at 60 s | not measured | — |
| 5,000-row page scroll at display rate | **120 Hz held**: 0 dropped frames in 1,320 scripted frames (2 runs), 1 late frame in the big-window run | — | **Yes** |
| Typing in a 2,000-line console | keystroke handled **~1.2–1.4 ms** + frame drawn **~1.8 ms** (p50); p99 4.4 + 2.6 ms; 1 late frame in 600 per run | — | **Yes** |

## Startup

Method (as Phase 0): a JXA script (`osascript -l JavaScript winwait.js <bin> <env> <log>`, below)
records `Date.now()`, starts the binary with `doShellScript("… &")`, and polls
`CGWindowListCopyWindowInfo(onScreenOnly)` every 4 ms for a layer-0 window of at
least 700×400 owned by that pid. It counts spawn, dyld, init and window creation.
The app logs `first frame after N ms` (`BOA_LOG=info`) from `window.on_next_frame`,
measured from the start of `app::run()`. Empty config dir (no connections, no
session), so no reconnect at launch. Each app was killed by its pid after 2 s.

| Batch (load avg) | Window on screen, ms | First frame, ms |
|---|---|---|
| 1 (≈ 12) | cold 910; warm 190, 367, 210, 220, 254, 205, 310, 337 (median ~237) | 261 cold; 123, 290, 139, 146, 147, 136, 234, 194 (median ~147) |
| 2 (≈ 5) | 229, 221, 211, 269, 203, 194, 192, 204 (median ~207) | 159, 155, 139, 143, 135, 129, 125, 139 (median ~139) |

Phase 0's skeleton measured ~160 ms / ~110 ms on a quieter machine. The full app adds
~30 ms in process (models, keymap, menus, session restore, the real views) and a
binary 1.7× larger (21 MB vs 12 MB) to map at spawn. On a loaded machine that puts
the window ~40 ms behind Tauri's *window* — but Tauri's window is empty until its
WebView has loaded and React has rendered, which that metric doesn't capture. A fair
comparison (time to a painted UI) needs a screen capture of the Tauri app; not done.
Not tried yet: precompiled Metal shaders (needs full Xcode on the build machine),
deferring non-visible work (history, updates, platform monitors) past the first frame.

## Memory

`footprint <pid>` (`phys_footprint`, what Activity Monitor shows as "Memory")
of our own process only, after the stated idle time; `ps -o rss=` alongside.

| State | Footprint | RSS | Notes |
|---|---|---|---|
| Empty app, 15 s | 65 MB ×3 (peak 160 MB) | 107 MB | |
| Connected, `select * from orders` with `rowLimit` 5,000 (5,000 × 7 page) | 88 MB at 60 s, 4 runs | 118 MB | IOSurface 49 MB, Malloc Small 22 MB |
| Same with `rowLimit` 500 | 90 MB at 15 s | 112 MB | the page itself is ~3 MB of the heap |
| Connected, 2,000-line console (no result) | 214 MB at 20 s → 119 MB at 60 s | — | IOSurface 82 MB, Malloc Small 21 MB |

Transient: right after a result is first painted the footprint is 180–217 MB, of
which ~90 MB is "Owned physical footprint (unmapped) (graphics)" (Metal's startup
buffers, as noted in Phase 0). The OS reclaims it after 5 s to over 30 s (one run
still showed 183 MB at 30 s, 88 MB at 60 s), so a single sample at 15 s can read
either value. The steady numbers above are after it went. Why the 2,000-line console
holds 82 MB of IOSurface (vs 49 MB otherwise) was not investigated.

## Scrolling a 5,000-row page

`BOA_GRID_BENCH=1` (grid/dev.rs) on the real path: `BOA_DEV_LIVE=1` connects,
`BOA_EDITOR_TEXT` + `BOA_DEV_RUN=1` runs the statement in a console with
`prefs.json` `{"rowLimit":5000}`, the result lands in the Database Output panel's
grid, and once painted the bench scrolls it one step per frame from `on_next_frame`.
"Interval" is what the display link delivered (8.33 ms = 120 Hz), "render→paint" the
CPU time from the grid's `render` to the end of its paint (GPU work excluded).

| Run | Phase (frames) | Interval p50 / p95 / max ms | Late frames | render→paint p50 / p95 / max ms |
|---|---|---|---|---|
| 5,000 × 7, default window (grid ~8 rows visible) | scroll 40 px (600) | 8.33 / 8.36 / 8.59 | 0 | 0.91 / 2.18 / 2.84 |
| | fling 400 px (300) | 8.33 / 8.35 / 8.38 | 0 | 1.97 / 2.24 / 3.64 |
| | scroll right 30 px (300) | 8.33 / 8.35 / 9.53 | 0 | 0.65 / 1.80 / 2.39 |
| | jump to random rows (120) | 8.33 / 8.38 / 8.39 | 0 | 1.97 / 3.37 / 3.96 |
| same, second run | all four phases | 8.33 / ≤ 8.38 / ≤ 8.67 | 0 | ≤ 2.42 / ≤ 2.89 / ≤ 3.28 |
| 5,000 × 16 (orders ⋈ customers + md5s), 2400×1400 window, ~29 rows × 15 columns visible | scroll 40 px | 8.33 / 8.35 / 14.31 | 1 | 2.13 / 2.47 / 3.84 |
| | fling 400 px | 8.33 / 8.34 / 8.36 | 0 | 4.39 / 4.74 / 7.29 |
| | scroll right 30 px | 8.33 / 8.35 / 8.37 | 0 | 1.88 / 2.17 / 2.41 |
| | jump to random rows | 8.33 / 8.52 / 9.03 | 0 | 7.54 / 7.80 / 8.26 |

Load to first paint: the grid paints 13–14 ms after the 5,000-row result arrives.
The big-window "jump" phase spends 7.5 ms of the 8.3 ms budget on the CPU (every
row and cell is new each frame); it held 120 Hz here but has the least headroom.

## Typing latency in a 2,000-line console

`BOA_EDITOR_BENCH_LINES=2000 BOA_EDITOR_BENCH=600` (views/workspace/dev.rs):
a console with 2,000 lines of SQL (500 four-line statements), the caret at the end
of a `where` line in the middle; then 600 keystrokes (" and o.total > 100 or
c.name = x", repeated) go through `window.dispatch_keystroke` — the real keymap,
then the editor's input handler — one per frame, from `on_next_frame`. Each key
is drawn by the frame that follows its callback. Logged: the dispatch time (keymap +
text insert + model updates + completion trigger), the frame intervals, and with
`ZED_MEASUREMENTS=1` GPUI's own "frame duration" (draw + present, CPU) of each of
the 600 frames. The typed text was checked in the saved `session.json` afterwards
(completion didn't swallow keys). 120 keystrokes/s is far faster than a person types.

| Run | Keystroke handling p50 / p95 / p99 / max ms | Frame draw+present p50 / p95 / p99 / max ms | Frame interval p50 / max ms (late) |
|---|---|---|---|
| 1 | 1.17 / 1.23 / 1.27 / 4.82 | 1.79 / 2.32 / 2.45 / 2.74 | 8.33 / 16.68 (1 of 600) |
| 2 | 1.36 / 3.29 / 4.37 / 9.86 | 1.72 / 2.34 / 2.57 / 3.04 | 8.33 / 16.68 (1 of 600) |

So a keystroke is on a finished frame ~3 ms (p50; ~7 ms p99) after it arrives, and
shows on screen at the next display refresh (≤ 8.3 ms at 120 Hz) plus the
compositor's latency, which this doesn't measure. Run 2's tail is the machine's load.

## Reproducing

`winwait.js` (startup):

```js
ObjC.import('CoreGraphics');
function run([bin, env, log]) {
  const app = Application.currentApplication(); app.includeStandardAdditions = true;
  const t0 = Date.now();
  const pid = parseInt(app.doShellScript(env + ' ' + bin + ' > ' + log + ' 2>&1 & echo $!'));
  while (Date.now() - t0 < 15000) {
    const list = ObjC.castRefToObject($.CGWindowListCopyWindowInfo($.kCGWindowListOptionOnScreenOnly, 0));
    for (let i = 0; i < list.count; i++) {
      const w = list.objectAtIndex(i), b = w.objectForKey('kCGWindowBounds');
      if (w.objectForKey('kCGWindowOwnerPID').js === pid && w.objectForKey('kCGWindowLayer').js === 0
          && b.objectForKey('Width').js >= 700 && b.objectForKey('Height').js >= 400)
        return JSON.stringify({pid, ms: Date.now() - t0});
    }
    delay(0.004);
  }
  return JSON.stringify({pid, ms: null});
}
```

Benchmarks (`--features dev-hooks` build):

```sh
dir=$(mktemp -d); echo '{"rowLimit":5000}' > $dir/prefs.json
BOA_CONFIG_DIR=$dir BOA_LOG=info BOA_DEV_LIVE=1 BOA_EDITOR_TEXT='select * from orders' \
  BOA_DEV_RUN=1 BOA_GRID_BENCH=1 BOA_GRID_BENCH_QUIT=1 target/release/boa-app   # --features dev-hooks
dir=$(mktemp -d)
BOA_CONFIG_DIR=$dir BOA_LOG=info ZED_MEASUREMENTS=1 BOA_DEV_LIVE=1 BOA_EDITOR_BENCH_LINES=2000 \
  BOA_EDITOR_BENCH=600 BOA_EDITOR_BENCH_QUIT=1 target/release/boa-app 2> typing.log
```

`BOA_DEV_LIVE` uses the libpq variables (default the local dev server). The window
must stay frontmost and unoccluded: an occluded GPUI window gets no frames, and the
benchmark then waits (one grid run stalled at "starting" this way and was discarded).
