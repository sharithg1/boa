# Releasing Boa (GPUI)

Status: **prepared, not published.** The build scripts, the in-app updater and a
manual-only CI build exist and were rehearsed locally (below). Nothing has been
signed with a Developer ID or the real updater key, no tag or GitHub release
exists. Every step that needs a credential is listed in
[What the user must run](#what-the-user-must-run-credentials).

The Tauri app's process (source repo `docs/RELEASING.md`, `release.yml`) is the
model: a `vX.Y.Z` GitHub release with per-arch DMGs / zips, the update tarballs,
`latest.json` and `SHA256SUMS`.

## Version

One source: `[workspace.package] version` in the root `Cargo.toml`
(`scripts/check-version.sh [vX.Y.Z]` prints / checks it). `scripts/bundle.sh`
writes it into Info.plist (`CFBundleShortVersionString` = `CFBundleVersion`).
The Tauri app's `scripts/release.sh` (bump + commit + tag) is not ported yet;
bump the version by hand and run `cargo update --workspace --offline`.

**The first GPUI release must be newer than every published Tauri release**, or
installed Tauri apps will not offer it. The source repo is at 0.6.1 (tag
`v0.6.1`), but `/Applications/Boa.app` on the dev machine is 0.6.2, so 0.6.2 is out
there: check `gh release list -R sharithg1/boa` and use 0.7.0 or higher.

## Building: `scripts/bundle.sh`

```sh
scripts/bundle.sh                          # universal Boa.app, ad-hoc signed, target/bundle/
scripts/bundle.sh --arch aarch64 --dmg     # one arch + DMG
scripts/bundle.sh --help                   # all options
```

It runs `cargo build --release --locked -p boa-app --target <triple>` per arch
(`MACOSX_DEPLOYMENT_TARGET=11.0`; `--arch universal` builds both and `lipo`s
them), assembles `Boa.app`, signs it (hardened runtime, no entitlements; ad-hoc
unless `--sign` / `APPLE_SIGNING_IDENTITY`), optionally notarizes + staples
(`--notarize`), then writes into `--out`:

| File | What |
|---|---|
| `Boa.app` | the bundle |
| `Boa_<v>_macos-<arch>.app.tar.gz` | in-app update bundle: one top-level `Boa.app/`, no `._*` AppleDouble entries (checked) |
| `….app.tar.gz.sig` | its updater signature, when `TAURI_SIGNING_PRIVATE_KEY` (+`_PASSWORD`) or `…_PATH` is set (`tauri signer sign --app-version <v>`; `TAURI_CLI` says how to run the Tauri CLI, default `npx --yes @tauri-apps/cli@2.12.3`) |
| `Boa_<v>_macos-<arch>.app.zip` | `ditto` zip of the app (as the Tauri release shipped) |
| `Boa_<v>_macos-<arch>.dmg` | with `--dmg`: plain UDZO image, `Boa.app` + `/Applications` link |
| `latest.json` | with `--base-url URL` and a signature: `scripts/latest-json.sh`, urls `URL/<file>` |
| `SHA256SUMS` | of the above |

The bundle (`crates/boa-app/bundle/`): `Info.plist` has exactly the keys of the
Tauri build (`CFBundleIdentifier dev.boa.desktop`, `CFBundleExecutable
boa-desktop` — the Tauri name, kept so anything that knew the path still finds
it — `LSMinimumSystemVersion 11.0`, `LSApplicationCategoryType
public.app-category.developer-tools`, `NSHighResolutionCapable`,
`LSRequiresCarbon`, `CSResourcesFileMapped`, `CFBundleIconFile icon.icns`, …);
`icon.icns` is the source's `crates/boa-desktop/icons/icon.icns` unchanged. The
Metal shaders are compiled at run time (`runtime_shaders`), which works under
the hardened runtime without entitlements (checked: both slices draw).

Sizes (0.6.1): arm64 tarball 9.4 MB, universal 17.4 MB, universal DMG 18.8 MB
(the Tauri arm64 tarball was 4.3 MB).

## In-app updates

`crates/boa-app/src/updater/` (the I/O half; the pure half is
`boa_core::state::updates`, the GPUI model `models/updates.rs`):

- **Feed:** `https://github.com/sharithg1/boa/releases/latest/download/latest.json`
  (`UPDATE_ENDPOINT`, the Tauri app's `plugins.updater.endpoints`);
  `BOA_UPDATE_ENDPOINT` replaces it at run time. Plain `http` only to loopback.
- **Key:** the Tauri app's `plugins.updater.pubkey` (minisign key id
  `A427C18232B67439`) is compiled in (`PRODUCTION_PUBKEY`).
  `BOA_UPDATER_PUBKEY=<base64>` **at build time** replaces it, for rehearsals
  with a throwaway key only; `bundle.sh` warns and the CI build refuses it.
  `updater::pubkey_is_production()` tells which.
- **Check / download:** `updater::check(&endpoint(), current_version(),
  current_arch())` → `Option<AvailableUpdate>`; `updater::download(&update,
  embedded_pubkey(), progress)` → `VerifiedUpdate` (minisign check via
  `minisign-verify`, the crate the plugin uses; the signed `version:` must be
  present and equal the announced version). Blocking: run them on
  `cx.background_spawn`.
- **Install:** `updater::install(&verified, &current_bundle()?)` unpacks the
  tarball (first path component dropped, as the plugin does; `..`, absolute
  paths, devices and links out of the bundle refused) into a temp folder on the
  app's volume and swaps it with the bundle in one `renameatx_np(RENAME_SWAP)`
  (rustix; two renames with restore where the volume can't swap), then touches
  the bundle for LaunchServices. Errors leave the bundle as it was.
- **Restart:** run the quit guard first (open transactions, as quitting does),
  then `updater::schedule_relaunch(&bundle)` and quit: a helper waits for this
  process to exit and `open -n`s the bundle, passing every `BOA_*` variable on.
- Not ported from the plugin: the "administrator privileges" install for a
  bundle the user can't write (reported as `Error::PermissionDenied` instead).

## The bridge release

Installed Tauri builds update through **their own** updater
(`tauri-plugin-updater` 2.14.1). The first GPUI release is delivered by it, so it
must be published exactly as the Tauri releases were:

1. **Same endpoint:** a published, non-prerelease GitHub release on
   `sharithg1/boa` with an asset named `latest.json` (the URL
   `releases/latest/download/latest.json` resolves to the newest such release).
2. **Same `latest.json`:** `{"version": "X.Y.Z", "notes": "<release URL>", "pub_date":
   "<RFC 3339>", "platforms": {"darwin-aarch64": {"url", "signature"},
   "darwin-x86_64": {"url", "signature"}}}`; `signature` is the text of the
   `.sig` file. `scripts/latest-json.sh` writes exactly this (per-arch tarballs, or
   one universal tarball for both keys).
3. **Signed with the real updater key** (`TAURI_SIGNING_PRIVATE_KEY` /
   `_PASSWORD`, the secrets the Tauri release workflow used), with
   `tauri signer sign --app-version X.Y.Z` (bundle.sh does it). Tauri apps don't
   require the signed version but check it when present; the GPUI app requires it.
4. **The `.app.tar.gz` layout the plugin unpacks:** a single top-level
   `Boa.app/` (the plugin drops the first component and swaps the result into
   wherever the app is installed), no AppleDouble files (they would break the
   code signature), Info.plist with `dev.boa.desktop`. bundle.sh checks this.
5. **Newer version** than any published Tauri release (see [Version](#version)).
6. Signed / notarized like the Tauri releases (or ad-hoc, as they currently are
   with `ALLOW_UNSIGNED_RELEASES`): the updater doesn't quarantine what it
   installs, so an ad-hoc bundle starts after the swap.
7. After the swap the Tauri app restarts through LaunchServices (`open -n` on
   the bundle path, tauri 2.12 `process::restart`); its fallback reads
   `CFBundleExecutable` from the new Info.plist.

From then on the GPUI app reads the same feed with the same key, so later
releases are published the same way. Users on Tauri versions before the
updater existed update by hand, as before.

## Rehearsals (local, throwaway key)

### GPUI → GPUI: `scripts/rehearse-update.sh [WORKDIR]`

Generates a throwaway key, builds N (this tree) and N+1 (a copy with the patch
version bumped) with `--features dev-hooks` and `BOA_UPDATER_PUBKEY`, serves N+1
with `python3 -m http.server` on 127.0.0.1:8765, then runs N from the work
folder with `BOA_DEV_UPDATE=1` (the `updater::dev` driver: check → download →
verify → install → relaunch → quit, without the UI) on an empty
`BOA_CONFIG_DIR`: first against a feed whose tarball was changed after signing
(must be rejected), then against the good feed.

Result (2026-11-26, arm64, 0.6.1 → 0.6.2): **passed.**

```
[pid 63502 v0.6.1] running 0.6.1 from …/install/Boa.app/Contents/MacOS/boa-desktop; config dir …/run2/config; key OVERRIDDEN (BOA_UPDATER_PUBKEY); checking http://127.0.0.1:8765/tampered/latest.json
[pid 63502 v0.6.1] update 0.6.2 available at http://127.0.0.1:8765/tampered/Boa_0.6.2_macos-aarch64.app.tar.gz
[pid 63502 v0.6.1] download rejected: the update's signature is not valid: The signature verification failed
[pid 63696 v0.6.1] running 0.6.1 … checking http://127.0.0.1:8765/latest.json
[pid 63696 v0.6.1] update 0.6.2 available at http://127.0.0.1:8765/Boa_0.6.2_macos-aarch64.app.tar.gz
[pid 63696 v0.6.1] downloaded and verified 0.6.2 (9444107 bytes)
[pid 63696 v0.6.1] installed 0.6.2 at …/install/Boa.app
[pid 63696 v0.6.1] relaunch scheduled; quitting
[pid 63759 v0.6.2] running 0.6.2 from …/install/Boa.app/Contents/MacOS/boa-desktop; config dir …/run2/config; …
[pid 63759 v0.6.2] up to date
```

The script also checks: the rejected run left the bundle (version, signature)
unchanged; after the update Info.plist says N+1, `codesign --verify --strict`
passes, nothing is left next to the bundle, and the relaunched instance ran
from the swapped bundle on the rehearsal's config dir (not the user's).

### Tauri → GPUI (the bridge): `scripts/rehearse-bridge.sh /path/to/Tauri/Boa.app [WORKDIR]`

Builds the GPUI bundle (universal, production key in the app) with its tarball
signed by a throwaway key, serves it, and runs `scripts/bridge-rehearsal` — the
Tauri app's own `tauri-plugin-updater` 2.14.1 (Cargo.lock copied from the Tauri
app) on Tauri's mock runtime, configured with the throwaway public key — against a
copy of a Tauri `Boa.app`: check, download, verify, install. Then it checks the
swapped bundle and starts it until it draws.

Result (2026-11-26, the source repo's Tauri 0.2.1 arm64 build → GPUI 0.6.1 universal): **passed**
(`0.2.1 -> 0.6.1 … downloaded and verified 17376338 bytes … installed`; the
result is universal, `dev.boa.desktop`, `boa-desktop`, signature valid, first
frame after ~0.4 s). Not covered: the Tauri UI's "Restart to update" and its
relaunch, and a Tauri build with the *real* key (needs the private key).

### Other checks done

- The real Tauri 0.2.1 update tarball and `.sig` verify against
  `PRODUCTION_PUBKEY` with our verifier (`BOA_TEST_TAURI_BUNDLE_DIR=… BOA_TEST_TAURI_BUNDLE_VERSION=0.2.1
  cargo test -p boa-app verifies_a_real_tauri_bundle`), and fail when announced as 0.2.2.
- Universal bundle: `lipo` x86_64 + arm64, `minos 11.0` on both slices, ad-hoc
  signature with `runtime` flag, both slices start and draw (x86_64 under Rosetta);
  DMG mounts with `Boa.app` + `Applications`, the app in it verifies;
  `shasum -c SHA256SUMS` passes.

## CI: `.github/workflows/release.yml`

`workflow_dispatch` only (never on push / tag / PR). Builds each arch with
`scripts/bundle.sh --arch <arch> [--dmg]` on `macos-15`, then the universal
bundle from both binaries (`--no-build`), ad-hoc signed, and keeps everything
as workflow artifacts. No secrets are read; nothing is published. Not run yet.
`ci.yml` (fmt, clippy, tests with Postgres, cargo deny, x86_64 build) is unchanged.

## What the user must run (credentials)

None of this has been run. In order, for version X.Y.Z (after bumping it):

1. **Developer ID certificate** in the login keychain ("Developer ID Application:
   Name (TEAMID)"), and notarization credentials stored once:
   `xcrun notarytool store-credentials boa-notary --apple-id <apple id> --team-id <TEAMID> --password <app-specific password>`
2. **Build, sign, notarize, staple** each architecture (bundle.sh signs with
   `codesign --options runtime --timestamp`, submits the app with `notarytool
   submit --wait`, `stapler staple`s it *before* making the update tarball, and
   does the same for the DMG):
   ```sh
   export APPLE_SIGNING_IDENTITY="Developer ID Application: Name (TEAMID)" NOTARY_PROFILE=boa-notary
   export TAURI_SIGNING_PRIVATE_KEY_PATH=~/.tauri/boa-updater.key TAURI_SIGNING_PRIVATE_KEY_PASSWORD=…
   base=https://github.com/sharithg1/boa/releases/download/vX.Y.Z
   scripts/bundle.sh --arch aarch64 --dmg --notarize --out dist/aarch64
   scripts/bundle.sh --arch x86_64  --dmg --notarize --out dist/x86_64
   mkdir dist/release && cp dist/*/Boa_* dist/release/
   scripts/latest-json.sh dist/release X.Y.Z "$base" https://github.com/sharithg1/boa/releases/tag/vX.Y.Z > dist/release/latest.json
   rm dist/release/*.sig   # the signatures live in latest.json
   (cd dist/release && shasum -a 256 -- * > SHA256SUMS)
   ```
   (Or one universal build: `scripts/bundle.sh --dmg --notarize --base-url "$base"`.)
   Check: `spctl -a -vv dist/aarch64/Boa.app` says "Notarized Developer ID";
   `xcrun stapler validate` on the app and DMG.
3. **Rehearse the bridge with the real artifacts** before publishing: serve
   `dist/release` locally with a `latest.json` whose urls point at it, and run
   `scripts/bridge-rehearsal/target/release/bridge-rehearsal <url> <real pubkey from tauri.conf.json> <copy of /Applications/Boa.app> <its version>`
   on a *copy* of the installed Tauri app.
4. **Tag and publish** (`git tag -a vX.Y.Z`, push, `gh release create vX.Y.Z dist/release/* --draft`,
   download back and compare checksums, then publish) — or add the Tauri
   workflow's `publish` job (with `TAURI_SIGNING_PRIVATE_KEY`, `APPLE_*` secrets) to
   `release.yml` and trigger it on the tag.
5. Without a Developer ID (as today, `ALLOW_UNSIGNED_RELEASES`): skip 1 and
   `--notarize`; users clear the quarantine flag once after a manual download
   (`xattr -dr com.apple.quarantine /Applications/Boa.app`); in-app updates are not
   quarantined.

**Keep the updater private key**: installed apps accept only updates signed by it.
