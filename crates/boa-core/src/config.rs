//! Where Boa keeps its files: the Tauri app's config dir, so existing users
//! keep their `connections.json` and `history-<id>.jsonl`.

use std::path::PathBuf;

/// The bundle identifier of the app (and the name of its config dir).
pub const APP_IDENTIFIER: &str = "dev.boa.desktop";

/// The app's config dir, as Tauri's `app_config_dir` resolves it:
/// `~/Library/Application Support/dev.boa.desktop/` on macOS
/// (`$XDG_CONFIG_HOME` or `~/.config` elsewhere). None without a home dir.
/// It is not created here; the writers create it when they first write.
pub fn config_dir() -> Option<PathBuf> {
    base_config_dir().map(|d| d.join(APP_IDENTIFIER))
}

#[cfg(target_os = "macos")]
fn base_config_dir() -> Option<PathBuf> {
    home().map(|h| h.join("Library").join("Application Support"))
}

#[cfg(target_os = "windows")]
fn base_config_dir() -> Option<PathBuf> {
    std::env::var_os("APPDATA")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn base_config_dir() -> Option<PathBuf> {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| home().map(|h| h.join(".config")))
}

#[cfg(not(target_os = "windows"))]
fn home() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}

#[cfg(test)]
mod tests {
    use super::config_dir;

    #[test]
    fn ends_with_the_bundle_identifier() {
        let dir = config_dir().unwrap();
        assert!(dir.ends_with("dev.boa.desktop"));
        #[cfg(target_os = "macos")]
        assert!(
            dir.to_string_lossy()
                .ends_with("/Library/Application Support/dev.boa.desktop")
        );
    }
}
