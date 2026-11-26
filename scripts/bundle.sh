#!/usr/bin/env bash
# Build Boa.app (the GPUI app) and its release artifacts, locally or in CI.
#
#   scripts/bundle.sh                      # universal Boa.app + updater tarball in target/bundle/
#   scripts/bundle.sh --arch aarch64 --dmg # one architecture, plus a DMG
#   scripts/bundle.sh --base-url URL       # also latest.json pointing at URL/<file>
#
# Options:
#   --arch universal|aarch64|x86_64   what to build (default universal: both, lipo'd)
#   --out DIR                          output folder (default target/bundle; emptied first)
#   --dmg                              also build Boa_<v>_macos-<arch>.dmg (hdiutil)
#   --base-url URL                     write latest.json (needs the updater signature)
#   --notes URL                        latest.json "notes" (default: the base URL)
#   --features LIST                    cargo features (e.g. dev-hooks for rehearsals; never for releases)
#   --no-build                         use <target dir>/<triple>/release/boa-app as built already
#   --sign IDENTITY                    codesign identity (default $APPLE_SIGNING_IDENTITY, else "-" = ad-hoc)
#   --notarize                         notarize + staple (needs a Developer ID identity and
#                                      NOTARY_PROFILE, or APPLE_ID + APPLE_PASSWORD + APPLE_TEAM_ID)
#
# Environment:
#   TAURI_SIGNING_PRIVATE_KEY (+ _PASSWORD), or TAURI_SIGNING_PRIVATE_KEY_PATH:
#       the updater's minisign key; when set, the .app.tar.gz gets its .sig
#       (`tauri signer sign --app-version <v>`, the format installed apps verify)
#   TAURI_CLI: how to run the Tauri CLI (default: npx --yes @tauri-apps/cli@2.12.3)
#   BOA_UPDATER_PUBKEY: build with this updater public key instead of the
#       production one (local rehearsals with a throwaway key ONLY)
#
# Outputs (in --out): Boa.app, Boa_<v>_macos-<arch>.app.tar.gz (+ .sig),
# Boa_<v>_macos-<arch>.app.zip, [.dmg], [latest.json], SHA256SUMS.
# Nothing is uploaded. See docs/RELEASING.md.
set -euo pipefail

cd "$(dirname "$0")/.."
export PATH="$HOME/.cargo/bin:$PATH"

usage() { sed -n '2,32p' "$0" | sed 's/^# \{0,1\}//'; exit "${1:-0}"; }
die() { echo "error: $*" >&2; exit 1; }
step() { echo "==> $*"; }

arch=universal
out=target/bundle
dmg=0
base_url=""
notes=""
features=""
build=1
identity="${APPLE_SIGNING_IDENTITY:--}"
notarize=0
while [ $# -gt 0 ]; do
    case "$1" in
        --arch) arch=${2:?}; shift ;;
        --out) out=${2:?}; shift ;;
        --dmg) dmg=1 ;;
        --base-url) base_url=${2:?}; shift ;;
        --notes) notes=${2:?}; shift ;;
        --features) features=${2:?}; shift ;;
        --no-build) build=0 ;;
        --sign) identity=${2:?}; shift ;;
        --notarize) notarize=1 ;;
        -h|--help) usage ;;
        *) echo "unknown argument: $1" >&2; usage 1 ;;
    esac
    shift
done
case "$arch" in
    universal) triples=(aarch64-apple-darwin x86_64-apple-darwin) ;;
    aarch64) triples=(aarch64-apple-darwin) ;;
    x86_64) triples=(x86_64-apple-darwin) ;;
    *) die "--arch must be universal, aarch64 or x86_64" ;;
esac
[ -n "$identity" ] || identity=-
if [ "$notarize" = 1 ] && [ "$identity" = - ]; then
    die "--notarize needs a Developer ID signing identity (--sign or APPLE_SIGNING_IDENTITY)"
fi

version=$(scripts/check-version.sh)
name="Boa_${version}_macos-${arch}"
step "Boa $version ($arch) -> $out"
if [ -n "${BOA_UPDATER_PUBKEY:-}" ]; then
    echo "WARNING: BOA_UPDATER_PUBKEY is set: this build trusts a NON-PRODUCTION updater key." >&2
    echo "WARNING: it is for local rehearsals only and must never be published." >&2
fi
[ -z "$features" ] || echo "note: cargo features: $features (not a release build)" >&2

# --- build -------------------------------------------------------------------
# macOS 11 is the oldest supported system (LSMinimumSystemVersion).
export MACOSX_DEPLOYMENT_TARGET=11.0
target_dir=${CARGO_TARGET_DIR:-target}
bins=()
for triple in "${triples[@]}"; do
    if [ "$build" = 1 ]; then
        step "cargo build --release --target $triple"
        cargo build --release --locked -p boa-app --target "$triple" ${features:+--features "$features"}
    fi
    bin="$target_dir/$triple/release/boa-app"
    [ -x "$bin" ] || die "$bin is missing (build it, or drop --no-build)"
    bins+=("$bin")
done

mkdir -p "$out"
# Only what this script writes; never anything else in --out.
rm -rf "$out/Boa.app" "$out/dmg-staging"
rm -f "$out"/Boa_*_macos-*.{app.tar.gz,app.tar.gz.sig,app.zip,dmg} "$out/latest.json" "$out/SHA256SUMS"

# --- Boa.app -----------------------------------------------------------------
app="$out/Boa.app"
step "assemble $app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
exe="$app/Contents/MacOS/boa-desktop"
if [ "${#bins[@]}" -gt 1 ]; then
    lipo -create -output "$exe" "${bins[@]}"
else
    cp "${bins[0]}" "$exe"
fi
chmod 755 "$exe"
lipo -info "$exe"
sed "s/@VERSION@/$version/g" crates/boa-app/bundle/Info.plist > "$app/Contents/Info.plist"
plutil -lint -s "$app/Contents/Info.plist"
cp crates/boa-app/bundle/icon.icns "$app/Contents/Resources/icon.icns"
plist_version=$(/usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' "$app/Contents/Info.plist")
[ "$plist_version" = "$version" ] || die "Info.plist version $plist_version != $version"
# Extended attributes (quarantine, provenance) would break the seal or end up in the tarball.
xattr -cr "$app"

# --- codesign / notarize -----------------------------------------------------
# Hardened runtime, no entitlements: as the Tauri app was signed.
if [ "$identity" = - ]; then
    step "codesign (ad-hoc)"
    codesign --force --options runtime --sign - "$app"
else
    step "codesign (Developer ID)"
    codesign --force --options runtime --timestamp --sign "$identity" "$app"
fi
codesign --verify --strict --verbose=2 "$app"

notarize() { # <file>
    if [ -n "${NOTARY_PROFILE:-}" ]; then
        xcrun notarytool submit "$1" --keychain-profile "$NOTARY_PROFILE" --wait
    else
        : "${APPLE_ID:?APPLE_ID, APPLE_PASSWORD and APPLE_TEAM_ID (or NOTARY_PROFILE) are needed to notarize}"
        xcrun notarytool submit "$1" --apple-id "$APPLE_ID" --password "${APPLE_PASSWORD:?}" \
            --team-id "${APPLE_TEAM_ID:?}" --wait
    fi
}
if [ "$notarize" = 1 ]; then
    step "notarize Boa.app"
    ditto -c -k --keepParent "$app" "$out/notarize.zip"
    notarize "$out/notarize.zip"
    rm -f "$out/notarize.zip"
    xcrun stapler staple "$app"
    xcrun stapler validate "$app"
fi

# --- updater tarball + zip -----------------------------------------------------
# The layout tauri-plugin-updater unpacks: one top-level Boa.app/ folder, no
# AppleDouble (._*) files (they would land inside the bundle and break its seal).
step "$name.app.tar.gz"
tgz="$out/$name.app.tar.gz"
COPYFILE_DISABLE=1 tar --no-mac-metadata --no-xattrs -czf "$tgz" -C "$out" Boa.app
if tar -tzf "$tgz" | grep -Ev '^Boa\.app(/|$)' | grep -q .; then
    die "$tgz has entries outside Boa.app/"
fi
if tar -tzf "$tgz" | grep -q '/\._'; then
    die "$tgz has AppleDouble (._) entries"
fi
ditto -c -k --keepParent "$app" "$out/$name.app.zip"

if [ -n "${TAURI_SIGNING_PRIVATE_KEY:-}" ] || [ -n "${TAURI_SIGNING_PRIVATE_KEY_PATH:-}" ]; then
    step "updater signature (tauri signer sign --app-version $version)"
    read -r -a tauri_cli <<<"${TAURI_CLI:-npx --yes @tauri-apps/cli@2.12.3}"
    "${tauri_cli[@]}" signer sign --app-version "$version" "$tgz" >/dev/null
    [ -s "$tgz.sig" ] || die "no signature written to $tgz.sig"
else
    echo "note: no TAURI_SIGNING_PRIVATE_KEY: the update tarball is not signed (no .sig, no latest.json)" >&2
fi

# --- DMG -----------------------------------------------------------------------
if [ "$dmg" = 1 ]; then
    step "$name.dmg"
    staging="$out/dmg-staging"
    mkdir -p "$staging"
    ditto "$app" "$staging/Boa.app"
    ln -s /Applications "$staging/Applications"
    hdiutil create -quiet -volname Boa -srcfolder "$staging" -ov -format UDZO "$out/$name.dmg"
    rm -rf "$staging"
    if [ "$identity" != - ]; then
        codesign --force --timestamp --sign "$identity" "$out/$name.dmg"
    fi
    if [ "$notarize" = 1 ]; then
        notarize "$out/$name.dmg"
        xcrun stapler staple "$out/$name.dmg"
    fi
fi

# --- latest.json ---------------------------------------------------------------
if [ -n "$base_url" ]; then
    case "$arch" in universal) archs="aarch64 x86_64" ;; *) archs=$arch ;; esac
    ARCHS=$archs scripts/latest-json.sh "$out" "$version" "$base_url" "${notes:-$base_url}" > "$out/latest.json"
    step "latest.json"
    cat "$out/latest.json"
fi

(
    cd "$out"
    files=(Boa_*_macos-*)
    if [ -f latest.json ]; then files+=(latest.json); fi
    shasum -a 256 -- "${files[@]}" > SHA256SUMS
)
step "done"
ls -l "$out"
