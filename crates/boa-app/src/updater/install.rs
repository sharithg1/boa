//! Installing a verified update: unpack the `.app.tar.gz` next to the
//! running bundle and swap the two, the way `tauri-plugin-updater` 2.x does it
//! on macOS (so a bundle either app produced installs the same way).

use std::ffi::OsString;
use std::io::Read as _;
use std::os::unix::fs::MetadataExt as _;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};

use super::{Error, VerifiedUpdate};

/// `…/Boa.app/Contents/MacOS/<exe>` → `…/Boa.app`; `None` for an
/// executable outside a bundle (`target/release/boa-app`).
pub fn bundle_of_executable(exe: &Path) -> Option<PathBuf> {
    let macos = exe.parent()?;
    let contents = macos.parent()?;
    let bundle = contents.parent()?;
    let named = |p: &Path, name: &str| p.file_name().is_some_and(|n| n == name);
    (named(macos, "MacOS")
        && named(contents, "Contents")
        && bundle.extension().is_some_and(|e| e == "app"))
    .then(|| bundle.to_path_buf())
}

/// The bundle this process runs from (the path it was launched by, not
/// resolving symlinks: the swap replaces whatever sits at that path).
pub fn current_bundle() -> Result<PathBuf, Error> {
    let exe = std::env::current_exe()?;
    bundle_of_executable(&exe).ok_or(Error::NotInBundle(exe))
}

fn archive_error(e: impl std::fmt::Display) -> Error {
    Error::Archive(e.to_string())
}

/// Unpacks a `.app.tar.gz` into `dest` (created; must not exist), dropping
/// each entry's first path component as the plugin does: `Boa.app/Contents/…`
/// lands in `dest/Contents/…`. Entries that would leave `dest`, devices and
/// links pointing out of it are refused. The result must look like a bundle
/// (`Contents/Info.plist` and `Contents/MacOS/`).
pub fn unpack(tar_gz: &[u8], dest: &Path) -> Result<(), Error> {
    std::fs::create_dir(dest)?;
    let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(tar_gz));
    archive.set_preserve_permissions(true);
    archive.set_preserve_mtime(true);
    archive.set_overwrite(false);
    let mut roots: Vec<OsString> = Vec::new();
    for entry in archive.entries().map_err(archive_error)? {
        let mut entry = entry.map_err(archive_error)?;
        let path = entry.path().map_err(archive_error)?.into_owned();
        let mut parts = path.components().filter(|c| *c != Component::CurDir);
        match parts.next() {
            Some(Component::Normal(root)) => {
                if !roots.iter().any(|r| r == root) {
                    roots.push(root.to_owned());
                }
            }
            _ => {
                return Err(archive_error(format!(
                    "unexpected entry {}",
                    path.display()
                )));
            }
        }
        let rest: PathBuf = parts.collect();
        if rest
            .components()
            .any(|c| !matches!(c, Component::Normal(_)))
        {
            return Err(archive_error(format!("unsafe path {}", path.display())));
        }
        let kind = entry.header().entry_type();
        if !(kind.is_file() || kind.is_dir() || kind.is_symlink()) {
            return Err(archive_error(format!(
                "unsupported entry {} ({kind:?})",
                path.display()
            )));
        }
        if kind.is_symlink() {
            // Frameworks use relative links inside the bundle; nothing may
            // point outside it.
            let target = entry
                .link_name()
                .map_err(archive_error)?
                .ok_or_else(|| archive_error("link without a target"))?;
            let depth = rest.components().count().saturating_sub(1);
            let mut level: isize = depth as isize;
            for c in target.components() {
                match c {
                    Component::Normal(_) => level += 1,
                    Component::CurDir => {}
                    Component::ParentDir => level -= 1,
                    _ => level = -1,
                }
                if level < 0 {
                    return Err(archive_error(format!(
                        "link {} points outside the bundle",
                        path.display()
                    )));
                }
            }
        }
        if rest.as_os_str().is_empty() {
            // The bundle directory itself.
            continue;
        }
        let out = dest.join(&rest);
        if let Some(parent) = out.parent() {
            std::fs::create_dir_all(parent)?;
        }
        entry.unpack(&out).map_err(archive_error)?;
    }
    // Drain whatever follows the archive (gzip trailer) so a truncated
    // download fails here rather than installing half a bundle.
    let mut rest = Vec::new();
    archive
        .into_inner()
        .read_to_end(&mut rest)
        .map_err(archive_error)?;
    if roots.len() != 1 {
        return Err(archive_error(format!(
            "expected one top-level folder, found {}",
            roots.len()
        )));
    }
    if !dest.join("Contents/Info.plist").is_file() || !dest.join("Contents/MacOS").is_dir() {
        return Err(archive_error("no Contents/Info.plist or Contents/MacOS"));
    }
    Ok(())
}

/// Exchanges `a` and `b` in one step (APFS); errors where the file system
/// can't (HFS+ answers ENOTSUP).
fn swap(a: &Path, b: &Path) -> std::io::Result<()> {
    use rustix::fs::{CWD, RenameFlags, renameat_with};
    renameat_with(CWD, a, CWD, b, RenameFlags::EXCHANGE).map_err(std::io::Error::from)
}

/// Moves `staged` to `target`, keeping `target` at `backup` until that has
/// worked (and putting it back if not).
fn replace(target: &Path, staged: &Path, backup: &Path) -> std::io::Result<()> {
    std::fs::rename(target, backup)?;
    if let Err(e) = std::fs::rename(staged, target) {
        let _ = std::fs::rename(backup, target);
        return Err(e);
    }
    Ok(())
}

/// Installs `update` over the bundle at `app` (normally [`current_bundle`]).
/// The new bundle is unpacked in a temp folder on the same volume (the
/// system temp dir if it is on it, else next to `app`), then the two are
/// swapped; the old bundle is deleted with the temp folder. On any error
/// `app` is left as it was. The running process keeps working (its files
/// stay open); relaunch with [`schedule_relaunch`].
pub fn install(update: &VerifiedUpdate, app: &Path) -> Result<(), Error> {
    let denied = |e: std::io::Error| {
        if e.kind() == std::io::ErrorKind::PermissionDenied {
            Error::PermissionDenied(app.to_path_buf())
        } else {
            Error::Io(e)
        }
    };
    let meta = std::fs::symlink_metadata(app)?;
    if !meta.is_dir() {
        return Err(Error::NotInBundle(app.to_path_buf()));
    }
    let tmp_root = [
        std::env::temp_dir(),
        app.parent().map(Path::to_path_buf).unwrap_or_default(),
    ]
    .into_iter()
    .filter(|d| !d.as_os_str().is_empty())
    .find(|d| d.metadata().is_ok_and(|m| m.dev() == meta.dev()))
    .ok_or_else(|| {
        Error::Io(std::io::Error::other(
            "no temporary folder on the app's volume",
        ))
    })?;
    let staging = tempfile::Builder::new()
        .prefix("boa-update-")
        .tempdir_in(&tmp_root)
        .map_err(denied)?;
    let new_app = staging.path().join("new.app");
    unpack(update.bytes(), &new_app)?;
    match swap(&new_app, app) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => return Err(denied(e)),
        Err(e) => {
            tracing::debug!("can't swap the bundles ({e}); moving them instead");
            replace(app, &new_app, &staging.path().join("old.app")).map_err(denied)?;
        }
    }
    // LaunchServices caches bundles by path: a newer mtime makes it re-read
    // Info.plist (the plugin does the same).
    let _ = Command::new("/usr/bin/touch")
        .arg(app)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    tracing::info!(version = update.version(), app = %app.display(), "update installed");
    // `staging` (now holding the previous bundle) is removed here.
    Ok(())
}

/// Waits for this process to exit (at most 60 s), then opens `app` as a new
/// instance. Every `BOA_*` variable of this process is passed on
/// (`open --env`), so a dev run's `BOA_CONFIG_DIR` survives the restart.
const RELAUNCH_SCRIPT: &str = r#"pid=$1; app=$2; shift 2
i=0
while kill -0 "$pid" 2>/dev/null && [ "$i" -lt 600 ]; do sleep 0.1; i=$((i+1)); done
exec /usr/bin/open -n "$@" "$app""#;

/// Starts the relaunch helper for `app`; quit the app right after (through
/// the quit guard: open transactions are asked about first). The helper
/// opens the new bundle once this process is gone, so the new instance reads
/// the session the old one flushed on quit.
pub fn schedule_relaunch(app: &Path) -> std::io::Result<()> {
    let mut cmd = Command::new("/bin/sh");
    cmd.arg("-c")
        .arg(RELAUNCH_SCRIPT)
        .arg("boa-relaunch")
        .arg(std::process::id().to_string())
        .arg(app);
    for (key, value) in std::env::vars_os() {
        if key.to_string_lossy().starts_with("BOA_") {
            let mut pair = key;
            pair.push("=");
            pair.push(value);
            cmd.arg("--env").arg(pair);
        }
    }
    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map(|_| ())
}
