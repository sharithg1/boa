//! Bundled assets: Boa's own files under `crates/boa-app/assets/` (fonts and
//! the app icon `boa-icon.png`, from the Tauri app's `ui/src/assets/`; Lucide
//! icons gpui-component doesn't bundle, ISC, under `icons/`),
//! falling back to gpui-component's icon set for everything else.

use std::borrow::Cow;

use anyhow::Context as _;
use gpui::{App, AssetSource, Result, SharedString};
use rust_embed::RustEmbed;

#[derive(RustEmbed)]
#[folder = "assets"]
#[include = "fonts/*.ttf"]
#[include = "boa-icon.png"]
#[include = "icons/*.svg"]
struct BoaAssets;

/// The app's asset source: Boa's files first, then gpui-component's icons.
#[derive(Debug, Clone, Copy, Default)]
pub struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        if let Some(file) = BoaAssets::get(path) {
            return Ok(Some(file.data));
        }
        gpui_component_assets::Assets.load(path)
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        let mut entries: Vec<SharedString> = BoaAssets::iter()
            .filter(|name| name.starts_with(path))
            .map(|name| SharedString::from(name.into_owned()))
            .collect();
        entries.extend(gpui_component_assets::Assets.list(path)?);
        Ok(entries)
    }
}

/// Registers the bundled Geist and Geist Mono faces with the text system.
///
/// Static TTFs (Regular / Medium / SemiBold, and Italic: preview tab titles,
/// SQL comments, `<null>`): GPUI's CoreText backend can't read WOFF2, and
/// named instances of a variable font aren't selectable by weight, so the
/// variable files the Tauri app ships aren't used. All from vercel/geist-font
/// v1.9.2 under the OFL (`assets/fonts/OFL.txt`).
pub fn load_fonts(cx: &App) -> anyhow::Result<()> {
    let fonts = font_files()
        .into_iter()
        .map(|path| {
            BoaAssets::get(&path)
                .map(|file| file.data)
                .with_context(|| format!("missing bundled font {path}"))
        })
        .collect::<anyhow::Result<Vec<_>>>()?;
    cx.text_system()
        .add_fonts(fonts)
        .context("registering bundled fonts")
}

/// Paths of the bundled font files, sorted.
pub fn font_files() -> Vec<String> {
    let mut files: Vec<String> = BoaAssets::iter()
        .filter(|name| name.starts_with("fonts/") && name.ends_with(".ttf"))
        .map(|name| name.into_owned())
        .collect();
    files.sort();
    files
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundles_geist_and_geist_mono() {
        let files = font_files();
        for face in ["Regular", "Medium", "SemiBold"] {
            assert!(
                files.contains(&format!("fonts/Geist-{face}.ttf")),
                "{files:?}"
            );
            assert!(
                files.contains(&format!("fonts/GeistMono-{face}.ttf")),
                "{files:?}"
            );
        }
    }

    /// OS/2 `fsSelection` bit 0 (ITALIC) of a TrueType file.
    fn is_italic(ttf: &[u8]) -> bool {
        let u16_at = |at: usize| u16::from_be_bytes([ttf[at], ttf[at + 1]]);
        let u32_at =
            |at: usize| u32::from_be_bytes([ttf[at], ttf[at + 1], ttf[at + 2], ttf[at + 3]]);
        let tables = u16_at(4) as usize;
        let os2 = (0..tables)
            .map(|i| 12 + i * 16)
            .find(|&rec| &ttf[rec..rec + 4] == b"OS/2")
            .map(|rec| u32_at(rec + 8) as usize)
            .expect("an OS/2 table");
        u16_at(os2 + 62) & 1 == 1
    }

    #[test]
    fn bundles_the_italic_faces() {
        // TitleBar.tsx shows preview tabs in italic (Geist Mono for tables),
        // SqlHighlight.tsx comments: real italic faces, not a fallback.
        for (path, italic) in [
            ("fonts/Geist-Italic.ttf", true),
            ("fonts/GeistMono-Italic.ttf", true),
            ("fonts/Geist-Regular.ttf", false),
            ("fonts/GeistMono-Regular.ttf", false),
        ] {
            assert!(font_files().contains(&path.to_owned()), "{path}");
            let data = BoaAssets::get(path).expect(path).data;
            assert_eq!(is_italic(&data), italic, "{path}");
        }
        assert!(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("assets/fonts/OFL.txt")
                .exists()
        );
    }

    #[test]
    fn every_icon_the_views_use_loads() {
        for icon in crate::views::chrome::USED_ICONS {
            let path = icon.path();
            assert!(Assets.load(&path).expect("load").is_some(), "{path}");
        }
        for icon in crate::views::chrome::BoaIcon::ALL {
            let path = icon.path();
            assert!(Assets.load(path).expect("load").is_some(), "{path}");
        }
        assert!(Assets.load("boa-icon.png").expect("load").is_some());
    }

    #[test]
    fn falls_back_to_component_icons() {
        let icon = Assets.load("icons/check.svg").expect("load");
        assert!(icon.is_some(), "gpui-component icons should resolve");
        assert!(
            Assets
                .load("fonts/Geist-Regular.ttf")
                .expect("load")
                .is_some()
        );
    }
}
