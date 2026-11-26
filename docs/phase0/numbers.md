# Phase 0: startup and memory

Machine: Apple M5 Pro, 24 GB, macOS 27.2.1. Measured 2026-11-26.

- **Skeleton:** `target/release/boa-app` (thin LTO, `codegen-units = 1`,
  stripped, `runtime_shaders`) at commit `p0/app`. One window, gpui-component
  initialised, theme and fonts loaded, placeholder editor / results split.
- **Tauri:** `/Applications/Boa.app` 0.6.2. Run with an isolated empty profile
  (`HOME=… CFFIXED_USER_HOME=…` pointing at a scratch dir), so it starts with no
  connections or session and the user's real data (and their running instance) are
  untouched.

## Results

| | GPUI skeleton | Tauri Boa 0.6.2 (empty) |
|---|---|---|
| Window on screen after spawn, warm (median of 8) | **~160 ms** (149–172) | **~165 ms** (159–194) |
| Window on screen, first launch after build (cold) | 1,151 ms | 233 ms (first run of the profile) |
| First frame drawn, measured in-process | **~110 ms** after `main` (106–120). Cold: 360 ms | n/a (see below) |
| Physical footprint at idle (15 s) | **62 MB** (1 process) | **83 MB** (27 app + 40 WebContent + 12 WebKit GPU + 4.6 Networking) |
| Peak footprint during startup | 157 MB | 29 MB app process (helpers not sampled) |
| RSS at idle (`ps -o rss`) | 98 MB | 103 MB app + 101 MB helpers |
| Binary | 12.2 MB | 8.6 MB (`boa-desktop`), 8.5 MB `.app` |

For reference, the user's own long-running Tauri instance (10 h uptime, real
workspace): 155 MB footprint (43 app + 87 WebContent + 19 GPU + 6 Networking).

## Method

- **Window on screen:** a JXA script (`osascript -l JavaScript`) records
  `Date.now()`, starts the binary through `doShellScript("… &")`, then polls
  `CGWindowListCopyWindowInfo(onScreenOnly)` every 4 ms for a new layer-0 window,
  at least 700×400, owned by the process. It counts spawn, dyld, init and window
  creation. Six runs each, alternating apps; the first run after a build is reported
  as "cold", the rest as warm.
- **Tauri caveat:** its window is visible before the WebView has painted the React UI,
  so "window on screen" flatters Tauri. The time to a painted UI is later. Without
  Screen Recording permission it couldn't be captured.
- **First frame (GPUI):** `app.rs` logs `first frame after N ms` from
  `window.on_next_frame`, measured from the start of `app::run()`.
- **Memory:** after 15 s idle, `footprint <pids>` (`phys_footprint` per
  process, the number Activity Monitor shows as "Memory") and `ps -o rss=`. For
  Tauri the WebKit XPC helpers (`com.apple.WebKit.{WebContent,GPU,Networking}`)
  started by that instance are included. They are found by diffing the helper PIDs
  before and after the launch. RSS counts shared system frameworks in every
  process, so footprint is the fairer comparison.
- Three memory samples per app. They were identical within 1 MB, except one
  earlier GPUI sample at 116 MB (taken 10 s after launch, before Metal released
  its startup buffers).

## Notes

- Startup is a wash at this size. The cold GPUI launch (1.1 s) is the first run of a
  freshly built binary (code-signature check, page-in) plus runtime compilation of
  the Metal shaders (`runtime_shaders`). Precompiled shaders in release builds
  (needs full Xcode on the build machine) should remove most of the shader part.
- GPUI's idle footprint is mostly graphics (Metal drawables and atlases for a 2× 1280×820
  window). It will grow less with UI complexity than a WebView does. Re-measure at
  the end of Phase 0 with the real editor and a 100k-row result.
