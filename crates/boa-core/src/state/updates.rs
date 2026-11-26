//! In-app updates, the pure half: ported from `ui/src/lib/updates.ts` (+
//! `updates.test.ts`), the update logic of the Tauri app's `src/updater.rs`,
//! and what `tauri-plugin-updater` (2.x) did with `latest.json`, so the GPUI
//! app can keep updating from the same release feed.
//!
//! - `ReleaseManifest`: `latest.json` as the release workflow writes it
//!   (`{version, notes, pub_date, platforms: {"darwin-aarch64": {url,
//!   signature}, …}}`) and the plugin read it (also the single-platform
//!   "dynamic" shape, `name` for `version`, a leading `v`);
//! - `Version`: SemVer 2.0 parsing and ordering (the plugin offers an
//!   update when the manifest's version is greater than the running one);
//! - `select_update`: manifest + running version + arch → the update to
//!   download (`darwin-<arch>-app`, then `darwin-<arch>`), none, or an error;
//! - `Updater`: the check → download → "Restart to update" state machine,
//!   emitting `UpdateEvent`s (the `boa://update` payload);
//! - the texts of the About window and the status bar pill.
//!
//! No network, signature check or install here: the minisign verification
//! against the public key in tauri.conf.json and the bundle swap belong to
//! whatever does the download (Phase 8).

use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Serialize};

/// The feed: `latest.json` of the newest published, non-prerelease GitHub
/// release (`plugins.updater.endpoints` in tauri.conf.json).
pub const UPDATE_ENDPOINT: &str =
    "https://github.com/sharithg1/boa/releases/latest/download/latest.json";

/// First automatic check after launch.
pub const FIRST_CHECK_MS: u64 = 20_000;
/// Between automatic checks.
pub const CHECK_EVERY_MS: u64 = 6 * 60 * 60 * 1000;
/// Set (to anything) to turn the periodic checks off.
pub const NO_UPDATE_CHECK_ENV: &str = "BOA_NO_UPDATE_CHECK";

/// Periodic checks run in release builds unless [`NO_UPDATE_CHECK_ENV`] is set
/// (`spawn_periodic_checks`). Manual checks always run.
pub fn periodic_checks_enabled(debug_build: bool, no_update_check_env: bool) -> bool {
    !debug_build && !no_update_check_env
}

// ── Versions ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum Ident {
    Num(u64),
    Alpha(String),
}

impl Ident {
    fn parse(s: &str, numeric_leading_zeros: bool) -> Result<Self, String> {
        if s.is_empty() || !s.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-') {
            return Err(format!("invalid identifier {s:?}"));
        }
        if s.bytes().all(|c| c.is_ascii_digit()) {
            if !numeric_leading_zeros && s.len() > 1 && s.starts_with('0') {
                return Err(format!("leading zero in {s:?}"));
            }
            if let Ok(n) = s.parse() {
                if numeric_leading_zeros && s.len() > 1 && s.starts_with('0') {
                    // Build metadata keeps "007" as written.
                    return Ok(Self::Alpha(s.to_owned()));
                }
                return Ok(Self::Num(n));
            }
        }
        Ok(Self::Alpha(s.to_owned()))
    }
}

impl Ord for Ident {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        use std::cmp::Ordering::{Greater, Less};
        match (self, other) {
            (Self::Num(a), Self::Num(b)) => a.cmp(b),
            (Self::Num(_), Self::Alpha(_)) => Less,
            (Self::Alpha(_), Self::Num(_)) => Greater,
            (Self::Alpha(a), Self::Alpha(b)) => a.cmp(b),
        }
    }
}

impl PartialOrd for Ident {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl fmt::Display for Ident {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Num(n) => write!(f, "{n}"),
            Self::Alpha(s) => f.write_str(s),
        }
    }
}

/// A SemVer 2.0 version (`major.minor.patch[-pre][+build]`), ordered as
/// the `semver` crate the updater used orders them: by precedence (a
/// pre-release sorts before its release), then build metadata.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Version {
    pub major: u64,
    pub minor: u64,
    pub patch: u64,
    pre: Vec<Ident>,
    build: Vec<Ident>,
}

impl Version {
    /// Parses a version; leading `v`s are dropped, as the updater did.
    pub fn parse(s: &str) -> Result<Self, String> {
        let s = s.trim_start_matches('v');
        let (rest, build) = match s.split_once('+') {
            Some((r, b)) => (r, Some(b)),
            None => (s, None),
        };
        let (core, pre) = match rest.split_once('-') {
            Some((c, p)) => (c, Some(p)),
            None => (rest, None),
        };
        let nums: Vec<&str> = core.split('.').collect();
        let [major, minor, patch] = nums.as_slice() else {
            return Err(format!("unexpected version {s:?}: want major.minor.patch"));
        };
        let num = |p: &str| -> Result<u64, String> {
            if p.is_empty()
                || !p.bytes().all(|c| c.is_ascii_digit())
                || (p.len() > 1 && p.starts_with('0'))
            {
                return Err(format!("invalid version number {p:?} in {s:?}"));
            }
            p.parse().map_err(|e| format!("{e} in {s:?}"))
        };
        let idents = |text: Option<&str>, zeros: bool| -> Result<Vec<Ident>, String> {
            text.map_or(Ok(Vec::new()), |t| {
                t.split('.').map(|i| Ident::parse(i, zeros)).collect()
            })
        };
        Ok(Self {
            major: num(major)?,
            minor: num(minor)?,
            patch: num(patch)?,
            pre: idents(pre, false)?,
            build: idents(build, true)?,
        })
    }

    /// Has a pre-release part (`0.7.0-rc.1`).
    pub fn is_prerelease(&self) -> bool {
        !self.pre.is_empty()
    }
}

impl Ord for Version {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        use std::cmp::Ordering::{Equal, Greater, Less};
        (self.major, self.minor, self.patch)
            .cmp(&(other.major, other.minor, other.patch))
            .then_with(|| match (self.pre.is_empty(), other.pre.is_empty()) {
                (true, true) => Equal,
                (true, false) => Greater,
                (false, true) => Less,
                (false, false) => self.pre.cmp(&other.pre),
            })
            .then_with(|| self.build.cmp(&other.build))
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)?;
        let join = |ids: &[Ident]| {
            ids.iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(".")
        };
        if !self.pre.is_empty() {
            write!(f, "-{}", join(&self.pre))?;
        }
        if !self.build.is_empty() {
            write!(f, "+{}", join(&self.build))?;
        }
        Ok(())
    }
}

impl std::str::FromStr for Version {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, String> {
        Self::parse(s)
    }
}

// ── latest.json ───────────────────────────────────────────────────────────

/// One platform's download in [`ReleaseManifest::platforms`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlatformAsset {
    /// The `.app.tar.gz` to download.
    pub url: String,
    /// Its minisign signature (the `.sig` file's text, base64).
    pub signature: String,
}

/// `latest.json`, field for field as the release workflow writes it (and
/// in that key order: `version`, `notes`, `pub_date`, `platforms`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReleaseManifest {
    #[serde(alias = "name")]
    pub version: String,
    /// Release notes (Boa puts the release page's URL here).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
    /// RFC 3339.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pub_date: Option<String>,
    /// By updater target (`darwin-aarch64`, `darwin-x86_64`, `darwin-aarch64-app` …).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub platforms: Option<BTreeMap<String, PlatformAsset>>,
    /// The "dynamic" shape: one download, no `platforms`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signature: Option<String>,
}

impl ReleaseManifest {
    /// Parses the feed's JSON.
    pub fn from_json(json: &str) -> Result<Self, UpdateError> {
        serde_json::from_str(json).map_err(|e| UpdateError::Manifest(e.to_string()))
    }
}

/// A newer release to download.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AvailableUpdate {
    /// Normalised (no leading `v`).
    pub version: String,
    pub url: String,
    pub signature: String,
    /// The updater target it was found under (empty for the dynamic shape).
    pub target: String,
    pub notes: Option<String>,
    pub pub_date: Option<String>,
}

/// Why a manifest could not be used.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdateError {
    /// Not valid JSON, or missing `version` / `url` / `signature`.
    Manifest(String),
    /// A version that is not SemVer.
    Version(String),
    /// `pub_date` is not RFC 3339.
    PubDate(String),
    /// Newer, but no download for this Mac (the targets tried).
    TargetsNotFound(Vec<String>),
}

impl fmt::Display for UpdateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Manifest(e) => write!(f, "invalid update manifest: {e}"),
            Self::Version(e) => write!(f, "invalid version: {e}"),
            Self::PubDate(e) => write!(f, "invalid value for `pub_date`: {e}"),
            Self::TargetsNotFound(t) => write!(
                f,
                "None of the fallback platforms `{t:?}` were found in the response `platforms` object"
            ),
        }
    }
}

impl std::error::Error for UpdateError {}

/// The `platforms` keys tried for this Mac, in order: `darwin-<arch>-app`,
/// `darwin-<arch>` (`arch`: `std::env::consts::ARCH`, `aarch64` / `x86_64`).
pub fn updater_targets(arch: &str) -> [String; 2] {
    [format!("darwin-{arch}-app"), format!("darwin-{arch}")]
}

/// What the updater's `check()` decided from a fetched manifest: the update
/// to download when its version is greater than `current`, `None` when not
/// (a release that is not newer is fine even without this Mac's platform),
/// or why the manifest is unusable. As in the plugin, a bad `pub_date` or a
/// dynamic manifest without `url` / `signature` fails even when not newer.
pub fn select_update(
    manifest: &ReleaseManifest,
    current: &str,
    arch: &str,
) -> Result<Option<AvailableUpdate>, UpdateError> {
    let version = Version::parse(&manifest.version).map_err(UpdateError::Version)?;
    if let Some(date) = &manifest.pub_date
        && parse_rfc3339_ms(date).is_none()
    {
        return Err(UpdateError::PubDate(date.clone()));
    }
    if manifest.platforms.is_none() {
        if manifest.url.is_none() {
            return Err(UpdateError::Manifest(
                "the `url` field was not set on the updater response".into(),
            ));
        }
        if manifest.signature.is_none() {
            return Err(UpdateError::Manifest(
                "the `signature` field was not set on the updater response".into(),
            ));
        }
    }
    let current = Version::parse(current).map_err(UpdateError::Version)?;
    if version <= current {
        return Ok(None);
    }
    let (target, asset) = match &manifest.platforms {
        Some(platforms) => {
            let targets = updater_targets(arch);
            let found = targets
                .iter()
                .find_map(|t| platforms.get(t).map(|a| (t.clone(), a.clone())));
            found.ok_or_else(|| UpdateError::TargetsNotFound(targets.to_vec()))?
        }
        None => (
            String::new(),
            PlatformAsset {
                url: manifest.url.clone().unwrap_or_default(),
                signature: manifest.signature.clone().unwrap_or_default(),
            },
        ),
    };
    Ok(Some(AvailableUpdate {
        version: version.to_string(),
        url: asset.url,
        signature: asset.signature,
        target,
        notes: manifest.notes.clone(),
        pub_date: manifest.pub_date.clone(),
    }))
}

// ── The update state machine ──────────────────────────────────────────────

/// Where an update check stands (`UpdatePhase`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum UpdatePhase {
    Checking,
    UpToDate,
    Downloading,
    /// Downloaded and verified: "Restart to update" installs it.
    Ready,
    Error,
}

/// The `boa://update` payload and `check_for_updates`' result (`UpdateEvent`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateEvent {
    pub phase: UpdatePhase,
    /// This app's version.
    pub current: String,
    /// The available version (downloading / ready).
    pub version: Option<String>,
    /// Started by the user (menu, About window), not the periodic check.
    pub manual: bool,
    /// The error (phase `error`).
    pub message: Option<String>,
}

/// What [`Updater::begin_check`] decided.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheckStart {
    /// A check or download is running (the Tauri app queued behind it;
    /// show its events instead).
    Busy,
    /// Already downloaded: nothing to do; this (`Ready`) event is emitted.
    Ready(UpdateEvent),
    /// Fetch the manifest now; this (`Checking`) event is emitted.
    Check(UpdateEvent),
}

/// The update state (`Updates` in src/updater.rs, without the bytes): the
/// downloaded version and the last event, for windows opened after it
/// (`update_status`). Checks never overlap.
///
/// Flow: [`begin_check`](Self::begin_check) → fetch + [`select_update`] →
/// [`checked`](Self::checked) → (if `Downloading`) download + verify →
/// [`downloaded`](Self::downloaded) → `Ready` → "Restart to update"
/// (after the open-transactions guard) installs what [`ready`](Self::ready) names.
#[derive(Debug, Clone)]
pub struct Updater {
    current: String,
    ready: Option<String>,
    busy: Option<bool>,
    last: Option<UpdateEvent>,
}

impl Updater {
    /// `current`: this app's version.
    pub fn new(current: impl Into<String>) -> Self {
        Self {
            current: current.into(),
            ready: None,
            busy: None,
            last: None,
        }
    }

    /// The last event, if any check ran (`update_status`).
    pub fn last(&self) -> Option<&UpdateEvent> {
        self.last.as_ref()
    }

    /// The downloaded version, waiting to be installed.
    pub fn ready(&self) -> Option<&str> {
        self.ready.as_deref()
    }

    fn emit(
        &mut self,
        phase: UpdatePhase,
        manual: bool,
        version: Option<String>,
        message: Option<String>,
    ) -> UpdateEvent {
        let e = UpdateEvent {
            phase,
            current: self.current.clone(),
            version,
            manual,
            message,
        };
        self.last = Some(e.clone());
        e
    }

    /// Starts a check (`manual`: from the menu / About window).
    pub fn begin_check(&mut self, manual: bool) -> CheckStart {
        if self.busy.is_some() {
            return CheckStart::Busy;
        }
        if let Some(v) = self.ready.clone() {
            return CheckStart::Ready(self.emit(UpdatePhase::Ready, manual, Some(v), None));
        }
        self.busy = Some(manual);
        CheckStart::Check(self.emit(UpdatePhase::Checking, manual, None, None))
    }

    /// The check's outcome: [`select_update`]'s result (an `Err` for fetch
    /// failures too). `Downloading` means: download it now and report with
    /// [`downloaded`](Self::downloaded).
    pub fn checked(&mut self, result: Result<Option<&AvailableUpdate>, String>) -> UpdateEvent {
        let manual = self.busy.unwrap_or(false);
        match result {
            Ok(Some(update)) => self.emit(
                UpdatePhase::Downloading,
                manual,
                Some(update.version.clone()),
                None,
            ),
            Ok(None) => {
                self.busy = None;
                self.emit(UpdatePhase::UpToDate, manual, None, None)
            }
            Err(message) => {
                self.busy = None;
                self.emit(UpdatePhase::Error, manual, None, Some(message))
            }
        }
    }

    /// The download's outcome (downloaded and signature verified, or the error).
    pub fn downloaded(&mut self, version: &str, result: Result<(), String>) -> UpdateEvent {
        let manual = self.busy.take().unwrap_or(false);
        match result {
            Ok(()) => {
                self.ready = Some(version.to_owned());
                self.emit(UpdatePhase::Ready, manual, Some(version.to_owned()), None)
            }
            Err(message) => self.emit(UpdatePhase::Error, manual, None, Some(message)),
        }
    }
}

/// The toast title when installing fails (status bar pill / About window).
pub const INSTALL_FAILED_TITLE: &str = "Could not install the update";

/// The status bar pill (shown only in phase `ready`).
pub fn update_pill_label(version: &str, installing: bool) -> String {
    if installing {
        "Restarting…".into()
    } else {
        format!("Restart to update to {version}")
    }
}

/// The pill's tooltip.
pub fn update_pill_title(version: &str) -> String {
    format!("Boa {version} is downloaded. Restart to install it; your tabs come back.")
}

/// The About window's status line under its buttons (`handed_over`: its
/// "Restart to Update" was pressed in the last 4 s).
pub fn about_status_line(status: Option<&UpdateEvent>, handed_over: bool) -> Option<String> {
    let s = status?;
    let v = s.version.as_deref().unwrap_or("");
    Some(match s.phase {
        UpdatePhase::Ready if handed_over => {
            "Restarting… Check the Boa window if it asks about open transactions.".into()
        }
        UpdatePhase::Checking => "Checking for updates…".into(),
        UpdatePhase::Downloading => format!("Downloading Boa {v}…"),
        UpdatePhase::Ready => format!("Boa {v} is ready to install."),
        UpdatePhase::UpToDate => "Boa is up to date.".into(),
        UpdatePhase::Error => format!(
            "Update check failed: {}",
            s.message.as_deref().unwrap_or("")
        ),
    })
}

/// "Check for Updates…" is disabled (and spins) while checking or downloading.
pub fn about_busy(status: Option<&UpdateEvent>) -> bool {
    status.is_some_and(|s| matches!(s.phase, UpdatePhase::Checking | UpdatePhase::Downloading))
}

// ── Version info ──────────────────────────────────────────────────────────

/// Version details for the About window and "Copy version info" (`AppInfo`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppInfo {
    pub name: String,
    pub version: String,
    /// `aarch64` / `x86_64`.
    pub arch: String,
    /// The commit built from; empty when unknown.
    pub commit: String,
    /// That commit's date, RFC 3339; empty when unknown.
    pub commit_date: String,
    /// e.g. `macOS 15.4.1`.
    pub os: String,
}

/// "Apple Silicon" / "Intel"; other architectures as given. (`archLabel`)
pub fn arch_label(arch: &str) -> &str {
    match arch {
        "aarch64" => "Apple Silicon",
        "x86_64" => "Intel",
        other => other,
    }
}

/// The About window's version line: "Version 0.3.0 (Apple Silicon)".
pub fn about_version_line(info: &AppInfo) -> String {
    format!("Version {} ({})", info.version, arch_label(&info.arch))
}

/// "Boa 0.2.0 (Apple Silicon)" plus commit and OS, for bug reports. (`versionInfoText`)
pub fn version_info_text(info: &AppInfo) -> String {
    let mut lines = vec![format!("Boa {} ({})", info.version, arch_label(&info.arch))];
    if !info.commit.is_empty() {
        lines.push(format!("Commit: {}", info.commit));
    }
    if !info.commit_date.is_empty() {
        lines.push(format!("Date: {}", info.commit_date));
    }
    lines.push(format!("OS: {}", info.os));
    lines.join("\n")
}

/// Days since 1970-01-01 of a proleptic Gregorian date.
fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = i64::from((m + 9) % 12);
    let doy = (153 * mp + 2) / 5 + i64::from(d) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// (year, month, day) of a day number since 1970-01-01.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = u32::try_from(doy - (153 * mp + 2) / 5 + 1).unwrap_or(1);
    let m = u32::try_from(if mp < 10 { mp + 3 } else { mp - 9 }).unwrap_or(1);
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

/// Milliseconds since the epoch of an ISO 8601 / RFC 3339 date-time
/// (`2026-11-10T12:00:00Z`, `…+02:00`, fractional seconds) or a bare date
/// (`2026-11-10`). A time without an offset is taken as UTC.
pub fn parse_rfc3339_ms(s: &str) -> Option<i64> {
    let b = s.as_bytes();
    let digits = |r: std::ops::Range<usize>| -> Option<u32> {
        let t = s.get(r)?;
        t.bytes()
            .all(|c| c.is_ascii_digit())
            .then(|| t.parse().ok())?
    };
    if b.len() < 10 || b[4] != b'-' || b[7] != b'-' {
        return None;
    }
    let (y, mo, d) = (digits(0..4)?, digits(5..7)?, digits(8..10)?);
    let leap = (y % 4 == 0 && y % 100 != 0) || y % 400 == 0;
    let month_days = [
        31,
        if leap { 29 } else { 28 },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
    if !(1..=12).contains(&mo) || d == 0 || d > month_days[(mo - 1) as usize] {
        return None;
    }
    let mut ms = days_from_civil(i64::from(y), mo, d) * 86_400_000;
    if b.len() == 10 {
        return Some(ms);
    }
    if !matches!(b[10], b'T' | b't' | b' ') || b.len() < 16 || b[13] != b':' {
        return None;
    }
    let (h, mi) = (digits(11..13)?, digits(14..16)?);
    let mut i = 16;
    let mut sec = 0;
    if b.get(i) == Some(&b':') {
        sec = digits(17..19)?;
        i = 19;
        if b.get(i) == Some(&b'.') {
            let start = i + 1;
            i = start;
            while b.get(i).is_some_and(u8::is_ascii_digit) {
                i += 1;
            }
            if i == start {
                return None;
            }
            let frac = &s[start..i.min(start + 3)];
            ms += i64::from(frac.parse::<u32>().ok()?) * 10_i64.pow(3 - frac.len() as u32);
        }
    }
    if h > 23 || mi > 59 || sec > 60 {
        return None;
    }
    ms += (i64::from(h) * 3600 + i64::from(mi) * 60 + i64::from(sec)) * 1000;
    match &s[i..] {
        "" | "Z" | "z" => Some(ms),
        off if off.len() == 6
            && matches!(off.as_bytes()[0], b'+' | b'-')
            && off.as_bytes()[3] == b':' =>
        {
            let oh = i64::from(digits(i + 1..i + 3)?);
            let om = i64::from(digits(i + 4..i + 6)?);
            let sign = if off.starts_with('-') { -1 } else { 1 };
            Some(ms - sign * (oh * 60 + om) * 60_000)
        }
        _ => None,
    }
}

const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

/// "Oct 14, 2026 (2 days ago)": the build's commit date in the About window;
/// empty for a missing or bad date. `now_ms`: ms since the epoch. The date
/// is the UTC one (the TS used the local time zone). (`releaseDateLabel`)
pub fn release_date_label(iso: &str, now_ms: i64) -> String {
    let Some(at) = parse_rfc3339_ms(iso) else {
        return String::new();
    };
    let (y, m, d) = civil_from_days(at.div_euclid(86_400_000));
    let days = (now_ms - at).div_euclid(86_400_000);
    let ago = match days {
        i64::MIN..=0 => "today".to_owned(),
        1 => "1 day ago".to_owned(),
        n => format!("{n} days ago"),
    };
    format!("{} {d}, {y} ({ago})", MONTHS[(m - 1) as usize])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info() -> AppInfo {
        AppInfo {
            name: "boa-desktop".into(),
            version: "0.3.0".into(),
            arch: "aarch64".into(),
            commit: "abc123".into(),
            commit_date: "2026-11-10T12:00:00Z".into(),
            os: "macOS 26.1".into(),
        }
    }

    // describe('archLabel')

    #[test]
    fn names_the_mac_architectures_and_passes_others_through() {
        assert_eq!(arch_label("aarch64"), "Apple Silicon");
        assert_eq!(arch_label("x86_64"), "Intel");
        assert_eq!(arch_label("riscv64"), "riscv64");
    }

    // describe('versionInfoText')

    #[test]
    fn lists_version_commit_date_and_os() {
        assert_eq!(
            version_info_text(&info()),
            "Boa 0.3.0 (Apple Silicon)\nCommit: abc123\nDate: 2026-11-10T12:00:00Z\nOS: macOS 26.1"
        );
    }

    #[test]
    fn leaves_out_an_unknown_commit_and_date() {
        let i = AppInfo {
            commit: String::new(),
            commit_date: String::new(),
            ..info()
        };
        assert_eq!(
            version_info_text(&i),
            "Boa 0.3.0 (Apple Silicon)\nOS: macOS 26.1"
        );
    }

    // describe('releaseDateLabel')

    fn now() -> i64 {
        parse_rfc3339_ms("2026-11-15T12:00:00Z").unwrap()
    }

    #[test]
    fn says_how_long_ago() {
        assert_eq!(
            release_date_label("2026-11-15T08:00:00Z", now()),
            "Nov 15, 2026 (today)"
        );
        assert_eq!(
            release_date_label("2026-11-14T08:00:00Z", now()),
            "Nov 14, 2026 (1 day ago)"
        );
        assert_eq!(
            release_date_label("2026-11-10T12:00:00Z", now()),
            "Nov 10, 2026 (5 days ago)"
        );
    }

    #[test]
    fn is_empty_for_a_missing_or_bad_date() {
        assert_eq!(release_date_label("", now()), "");
        assert_eq!(release_date_label("not a date", now()), "");
    }

    // Beyond the TS tests: what src/updater.rs and the updater plugin did.

    #[test]
    fn parses_dates() {
        assert_eq!(parse_rfc3339_ms("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(parse_rfc3339_ms("1970-01-01"), Some(0));
        assert_eq!(parse_rfc3339_ms("1970-01-01T01:00:00.250+01:00"), Some(250));
        assert_eq!(
            parse_rfc3339_ms("2000-02-29T00:00:00Z"),
            Some(951_782_400_000)
        );
        assert_eq!(parse_rfc3339_ms("2001-02-29T00:00:00Z"), None);
        assert_eq!(parse_rfc3339_ms("2026-13-01T00:00:00Z"), None);
        assert_eq!(parse_rfc3339_ms("2026-11-10T12:00:00 UTC"), None);
        assert_eq!(
            release_date_label("2024-02-29T23:59:59-00:00", now())
                .split(' ')
                .next(),
            Some("Feb")
        );
    }

    #[test]
    fn orders_versions_as_semver() {
        let v = |s: &str| Version::parse(s).unwrap();
        assert!(v("0.6.2") > v("0.6.1"));
        assert!(v("0.10.0") > v("0.9.9"));
        assert!(v("1.0.0") > v("1.0.0-rc.1"));
        assert!(v("1.0.0-rc.2") > v("1.0.0-rc.1"));
        assert!(v("1.0.0-rc.10") > v("1.0.0-rc.9"));
        assert!(v("1.0.0-alpha.beta") > v("1.0.0-alpha.1"));
        assert!(v("1.0.0-alpha.1") > v("1.0.0-alpha"));
        assert_eq!(v("v0.6.1"), v("0.6.1"));
        assert_eq!(v("0.6.1+b.007").to_string(), "0.6.1+b.007");
        assert!(v("0.6.1+2") > v("0.6.1"));
        assert!(v("1.0.0-rc.1").is_prerelease());
        for bad in [
            "", "1", "1.2", "1.2.3.4", "01.2.3", "1.2.3-", "1.2.3-01", "1.2.x", " 1.2.3", "1.2.3+",
        ] {
            assert!(Version::parse(bad).is_err(), "{bad:?}");
        }
    }

    /// latest.json exactly as the release workflow's jq writes it.
    const LATEST: &str = r#"{
  "version": "0.7.0",
  "notes": "https://github.com/sharithg1/boa/releases/tag/v0.7.0",
  "pub_date": "2026-11-20T10:00:00Z",
  "platforms": {
    "darwin-aarch64": {
      "url": "https://github.com/sharithg1/boa/releases/download/v0.7.0/Boa_0.7.0_macos-aarch64.app.tar.gz",
      "signature": "c2lnLWFhcmNoNjQ=\n"
    },
    "darwin-x86_64": {
      "url": "https://github.com/sharithg1/boa/releases/download/v0.7.0/Boa_0.7.0_macos-x86_64.app.tar.gz",
      "signature": "c2lnLXg4Ng==\n"
    }
  }
}"#;

    #[test]
    fn reads_latest_json_and_picks_this_macs_download() {
        let m = ReleaseManifest::from_json(LATEST).unwrap();
        let u = select_update(&m, "0.6.1", "aarch64").unwrap().unwrap();
        assert_eq!(u.version, "0.7.0");
        assert_eq!(u.target, "darwin-aarch64");
        assert!(u.url.ends_with("Boa_0.7.0_macos-aarch64.app.tar.gz"));
        assert_eq!(u.signature, "c2lnLWFhcmNoNjQ=\n");
        assert_eq!(u.pub_date.as_deref(), Some("2026-11-20T10:00:00Z"));
        let u = select_update(&m, "0.6.1", "x86_64").unwrap().unwrap();
        assert_eq!(u.target, "darwin-x86_64");
        // Same or older: no update.
        assert_eq!(select_update(&m, "0.7.0", "aarch64").unwrap(), None);
        assert_eq!(select_update(&m, "0.8.0-rc.1", "aarch64").unwrap(), None);
        // Newer, but nothing for this Mac.
        assert_eq!(
            select_update(&m, "0.6.1", "riscv64").unwrap_err(),
            UpdateError::TargetsNotFound(vec![
                "darwin-riscv64-app".into(),
                "darwin-riscv64".into()
            ])
        );
        // Written back with the same keys, in the same order.
        let back = serde_json::to_string_pretty(&m).unwrap();
        assert_eq!(back, LATEST);
    }

    #[test]
    fn prefers_the_app_bundle_target_and_reads_the_other_shapes() {
        let m = ReleaseManifest::from_json(
            r#"{"name":"v1.0.0","platforms":{"darwin-aarch64":{"url":"a","signature":"s1"},"darwin-aarch64-app":{"url":"b","signature":"s2"}}}"#,
        )
        .unwrap();
        let u = select_update(&m, "0.9.0", "aarch64").unwrap().unwrap();
        assert_eq!(
            (u.version.as_str(), u.url.as_str(), u.target.as_str()),
            ("1.0.0", "b", "darwin-aarch64-app")
        );
        // The dynamic shape: one download.
        let m =
            ReleaseManifest::from_json(r#"{"version":"1.0.0","url":"u","signature":"s"}"#).unwrap();
        assert_eq!(
            select_update(&m, "0.9.0", "aarch64").unwrap().unwrap().url,
            "u"
        );
        let m = ReleaseManifest::from_json(r#"{"version":"1.0.0","url":"u"}"#).unwrap();
        assert!(matches!(
            select_update(&m, "2.0.0", "aarch64"),
            Err(UpdateError::Manifest(_))
        ));
        // Bad versions and dates fail even when not newer.
        let m = ReleaseManifest::from_json(r#"{"version":"1.0","platforms":{}}"#).unwrap();
        assert!(matches!(
            select_update(&m, "0.1.0", "aarch64"),
            Err(UpdateError::Version(_))
        ));
        let m = ReleaseManifest::from_json(
            r#"{"version":"1.0.0","pub_date":"yesterday","platforms":{}}"#,
        )
        .unwrap();
        assert!(matches!(
            select_update(&m, "2.0.0", "aarch64"),
            Err(UpdateError::PubDate(_))
        ));
        assert!(ReleaseManifest::from_json("{}").is_err());
        assert!(ReleaseManifest::from_json("<html>").is_err());
    }

    #[test]
    fn checks_downloads_and_then_offers_the_restart() {
        let mut u = Updater::new("0.6.1");
        assert_eq!(u.last(), None);
        let CheckStart::Check(e) = u.begin_check(false) else {
            panic!()
        };
        assert_eq!(
            (e.phase, e.manual, e.current.as_str()),
            (UpdatePhase::Checking, false, "0.6.1")
        );
        // Checks never overlap.
        assert_eq!(u.begin_check(true), CheckStart::Busy);
        let m = ReleaseManifest::from_json(LATEST).unwrap();
        let avail = select_update(&m, "0.6.1", "aarch64").unwrap().unwrap();
        let e = u.checked(Ok(Some(&avail)));
        assert_eq!(
            (e.phase, e.version.as_deref()),
            (UpdatePhase::Downloading, Some("0.7.0"))
        );
        assert_eq!(u.begin_check(true), CheckStart::Busy);
        let e = u.downloaded("0.7.0", Ok(()));
        assert_eq!(
            (e.phase, e.version.as_deref()),
            (UpdatePhase::Ready, Some("0.7.0"))
        );
        assert_eq!(u.ready(), Some("0.7.0"));
        assert_eq!(u.last(), Some(&e));
        // A later check does not download again.
        let CheckStart::Ready(e) = u.begin_check(true) else {
            panic!()
        };
        assert_eq!((e.phase, e.manual), (UpdatePhase::Ready, true));
        assert_eq!(
            serde_json::to_string(&e).unwrap(),
            r#"{"phase":"ready","current":"0.6.1","version":"0.7.0","manual":true,"message":null}"#
        );
    }

    #[test]
    fn reports_up_to_date_and_errors() {
        let mut u = Updater::new("0.7.0");
        let _ = u.begin_check(true);
        let e = u.checked(Ok(None));
        assert_eq!((e.phase, e.manual), (UpdatePhase::UpToDate, true));
        let _ = u.begin_check(false);
        let e = u.checked(Err("error sending request".into()));
        assert_eq!(
            (e.phase, e.message.as_deref()),
            (UpdatePhase::Error, Some("error sending request"))
        );
        let _ = u.begin_check(false);
        let avail = AvailableUpdate {
            version: "0.8.0".into(),
            url: "u".into(),
            signature: "s".into(),
            target: "darwin-aarch64".into(),
            notes: None,
            pub_date: None,
        };
        let _ = u.checked(Ok(Some(&avail)));
        let e = u.downloaded("0.8.0", Err("signature mismatch".into()));
        assert_eq!(e.phase, UpdatePhase::Error);
        assert_eq!(u.ready(), None);
        assert!(matches!(u.begin_check(false), CheckStart::Check(_)));
    }

    #[test]
    fn about_window_and_pill_texts() {
        let ev = |phase, version: Option<&str>, message: Option<&str>| UpdateEvent {
            phase,
            current: "0.6.1".into(),
            version: version.map(Into::into),
            manual: true,
            message: message.map(Into::into),
        };
        let line = |e: &UpdateEvent, h| about_status_line(Some(e), h).unwrap();
        assert_eq!(about_status_line(None, false), None);
        assert_eq!(
            line(&ev(UpdatePhase::Checking, None, None), false),
            "Checking for updates…"
        );
        assert_eq!(
            line(&ev(UpdatePhase::Downloading, Some("9.9.9"), None), false),
            "Downloading Boa 9.9.9…"
        );
        assert_eq!(
            line(&ev(UpdatePhase::Ready, Some("9.9.9"), None), false),
            "Boa 9.9.9 is ready to install."
        );
        assert_eq!(
            line(&ev(UpdatePhase::Ready, Some("9.9.9"), None), true),
            "Restarting… Check the Boa window if it asks about open transactions."
        );
        assert_eq!(
            line(&ev(UpdatePhase::UpToDate, None, None), true),
            "Boa is up to date."
        );
        assert_eq!(
            line(&ev(UpdatePhase::Error, None, Some("offline")), false),
            "Update check failed: offline"
        );
        assert!(about_busy(Some(&ev(
            UpdatePhase::Downloading,
            Some("1.0.0"),
            None
        ))));
        assert!(!about_busy(Some(&ev(
            UpdatePhase::Ready,
            Some("1.0.0"),
            None
        ))));
        assert!(!about_busy(None));
        assert_eq!(
            update_pill_label("0.7.0", false),
            "Restart to update to 0.7.0"
        );
        assert_eq!(update_pill_label("0.7.0", true), "Restarting…");
        assert_eq!(
            update_pill_title("0.7.0"),
            "Boa 0.7.0 is downloaded. Restart to install it; your tabs come back."
        );
        assert_eq!(about_version_line(&info()), "Version 0.3.0 (Apple Silicon)");
        assert!(periodic_checks_enabled(false, false));
        assert!(!periodic_checks_enabled(true, false));
        assert!(!periodic_checks_enabled(false, true));
    }
}
