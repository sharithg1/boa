# Notes for coding agents

## Running the app while the user works

The user keeps working on this Mac while you test, so the app must never pop up
or take the focus:

- Launch every dev run, screenshot and `BOA_DEV_INPUT` session with
  `BOA_DEV_BACKGROUND=1` (feature `dev-hooks` / `screenshot`). The app then runs
  without a Dock icon, never activates, and opens its windows hidden.
- Capture with `shot <png>` (in the `BOA_DEV_INPUT` file) or `BOA_SCREENSHOT`. Both
  render offscreen.
- Never use the `front` command, `osascript` / System Events activation, `open`
  on the app, or `screencapture` of the live window. Check behaviour with
  `#[gpui::test]` tests (`VisualTestContext`) where possible.
- Native chrome (traffic lights, the menu bar, macOS dialogs) needs a normal,
  visible run: ask the user first, and say when the window will appear.
- Use a scratch `BOA_CONFIG_DIR`, and kill only the processes you started
  (`kill <pid>`, never `pkill`). `BOA_GRID_QUIT_AFTER_MS` makes the app quit itself.

See docs/DEVELOPMENT.md for the environment variables and the checks that must
pass before merging.
