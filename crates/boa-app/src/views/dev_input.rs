//! `BOA_DEV_INPUT=<file>` (feature `dev-hooks`): drive the main window from a
//! script, for end-to-end checks of the real app when the shell running the
//! agent has no Accessibility permission (System Events / CGEvent input is
//! dropped). Every line appended to the file is one command; events go
//! through `Window::dispatch_event` / `dispatch_keystroke`, so the real
//! keymap, key contexts, hit testing and focus handle them (only the native
//! NSEvent layer is skipped). After each command the number of commands done
//! is written to `<file>.done`.
//!
//! - `key cmd-t alt-1 enter`: keystrokes (GPUI syntax, space separated)
//! - `type some text`: types the rest of the line (`\\n` types Enter)
//! - `click X Y [COUNT] [MODS]`: left click at window point (X, Y); COUNT 2 =
//!   double click; MODS like `cmd,shift`
//! - `rclick X Y`, `move X Y`
//! - `wait MS`
//! - `front`: activate the app and bring the window to the front (ignored
//!   with `BOA_DEV_BACKGROUND`, which keeps the app off the user's screen)
//! - `shot PATH` (feature `screenshot`): draws a fresh frame and writes it to
//!   PATH as a PNG, without bringing the window to the front (an occluded
//!   window draws no frames, and `screencapture` would need it in front)
//!
//! Never compiled into release builds.

use std::path::PathBuf;
use std::time::Duration;

use gpui::{
    App, AppContext as _, Keystroke, Modifiers, MouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, PlatformInput, Window, point, px,
};
use gpui_component::Root;

/// The variable naming the command file.
pub const ENV: &str = "BOA_DEV_INPUT";

/// Starts polling the command file named by [`ENV`], if set.
pub fn start(window: gpui::WindowHandle<Root>, cx: &mut App) {
    let Some(path) = std::env::var_os(ENV).filter(|v| !v.is_empty()) else {
        return;
    };
    let path = PathBuf::from(path);
    let done_path = {
        let mut p = path.clone().into_os_string();
        p.push(".done");
        PathBuf::from(p)
    };
    cx.spawn(async move |cx| {
        let mut done = 0usize;
        loop {
            cx.background_executor()
                .timer(Duration::from_millis(50))
                .await;
            let text = std::fs::read_to_string(&path).unwrap_or_default();
            let lines: Vec<String> = text.lines().map(str::to_owned).collect();
            while done < lines.len() {
                let line = lines[done].trim().to_owned();
                if let Some(ms) = line.strip_prefix("wait ") {
                    let ms = ms.trim().parse().unwrap_or(0);
                    cx.background_executor()
                        .timer(Duration::from_millis(ms))
                        .await;
                } else if !line.is_empty() {
                    // Not `WindowHandle::update`: that leases the root view, and
                    // drawing a frame (`shot`) needs it.
                    let ok = cx
                        .update_window(window.into(), |_, window, cx| run(&line, window, cx))
                        .is_ok();
                    if !ok {
                        return;
                    }
                }
                done += 1;
                let _ = std::fs::write(&done_path, done.to_string());
                // Let the frame after each command draw.
                cx.background_executor()
                    .timer(Duration::from_millis(30))
                    .await;
            }
        }
    })
    .detach();
}

fn modifiers(spec: Option<&str>) -> Modifiers {
    let mut m = Modifiers::default();
    for part in spec.unwrap_or("").split(',') {
        match part.trim() {
            "cmd" => m.platform = true,
            "shift" => m.shift = true,
            "alt" => m.alt = true,
            "ctrl" => m.control = true,
            _ => {}
        }
    }
    m
}

fn run(line: &str, window: &mut Window, cx: &mut App) {
    // Hit testing uses the last drawn frame, and an occluded window draws
    // none by itself: draw one first so the event lands on what's current.
    window.draw(cx).clear(cx);
    let (cmd, rest) = line.split_once(' ').unwrap_or((line, ""));
    let args: Vec<&str> = rest.split_whitespace().collect();
    let at = |i: usize| -> f32 { args.get(i).and_then(|v| v.parse().ok()).unwrap_or(0.) };
    match cmd {
        "key" => {
            for source in &args {
                match Keystroke::parse_with_os(source, cx.platform_info().os).ok() {
                    Some(k) => {
                        window.dispatch_keystroke(k, cx);
                    }
                    None => tracing::warn!("dev input: bad keystroke {source}"),
                }
            }
        }
        "type" => {
            for ch in rest.replace("\\n", "\n").chars() {
                let k = if ch == '\n' {
                    Keystroke::parse("enter").expect("enter parses")
                } else {
                    let key = ch.to_string();
                    Keystroke {
                        modifiers: Modifiers::default(),
                        key: key.to_lowercase(),
                        key_char: Some(key),
                    }
                };
                window.dispatch_keystroke(k, cx);
            }
        }
        "move" => {
            window.dispatch_event(
                PlatformInput::MouseMove(MouseMoveEvent {
                    position: point(px(at(0)), px(at(1))),
                    pressed_button: None,
                    modifiers: Modifiers::default(),
                }),
                cx,
            );
        }
        "click" | "rclick" => {
            let position = point(px(at(0)), px(at(1)));
            let button = if cmd == "rclick" {
                MouseButton::Right
            } else {
                MouseButton::Left
            };
            let count = args
                .get(2)
                .and_then(|v| v.parse().ok())
                .unwrap_or(1usize)
                .max(1);
            let modifiers = modifiers(args.get(3).copied());
            window.dispatch_event(
                PlatformInput::MouseMove(MouseMoveEvent {
                    position,
                    pressed_button: None,
                    modifiers,
                }),
                cx,
            );
            for click_count in 1..=count {
                window.dispatch_event(
                    PlatformInput::MouseDown(MouseDownEvent {
                        button,
                        position,
                        modifiers,
                        click_count,
                        first_mouse: false,
                    }),
                    cx,
                );
                window.dispatch_event(
                    PlatformInput::MouseUp(MouseUpEvent {
                        button,
                        position,
                        modifiers,
                        click_count,
                    }),
                    cx,
                );
            }
        }
        // Bring the app and this window to the front, so the window draws
        // frames again and `screencapture -l` sees what is current.
        "front" if crate::app::background() => {
            tracing::warn!("dev input: `front` ignored (BOA_DEV_BACKGROUND)");
        }
        "front" => {
            cx.activate(true);
            window.activate_window();
        }
        #[cfg(feature = "screenshot")]
        "shot" => {
            window.refresh();
            window.draw(cx).clear(cx);
            window.refresh();
            window.draw(cx).clear(cx);
            match window.render_to_image() {
                Ok(image) => {
                    if let Err(err) = image.save(rest.trim()) {
                        tracing::error!("dev input: saving {rest}: {err}");
                    }
                }
                Err(err) => tracing::error!("dev input: render_to_image: {err:#}"),
            }
            return;
        }
        other => tracing::warn!("dev input: unknown command {other:?}"),
    }
    window.refresh();
}
