#!/usr/bin/env bash
# Rehearse the BRIDGE release on this Mac: an installed Tauri Boa.app updating
# itself to the GPUI app through the Tauri app's own updater
# (tauri-plugin-updater 2.14.1, run by scripts/bridge-rehearsal on Tauri's mock
# runtime). Throwaway key, local feed, copies in a work folder only.
#
#   scripts/rehearse-bridge.sh /path/to/Tauri/Boa.app [WORKDIR]
#
# e.g. the Tauri repo's target/release/bundle/macos/Boa.app (read, never changed).
# 1. builds the GPUI bundle (universal, production updater key) with
#    scripts/bundle.sh, its update tarball signed with a throwaway key;
# 2. serves it + latest.json on 127.0.0.1;
# 3. the Tauri updater (configured with the throwaway public key, as the real
#    app is with the real one) checks, downloads, verifies and installs it over
#    a copy of the Tauri Boa.app;
# 4. checks the swapped bundle (version, executable, signature) and starts it
#    on an empty BOA_CONFIG_DIR until it draws its first frame.
#
# What it does not cover: the Tauri app's UI ("Restart to update") and its
# relaunch (`open -n` on the bundle; tauri 2.12 process::restart).
# Needs: the Tauri CLI (TAURI_CLI), python3. SKIP_BUILD=1 reuses WORKDIR/gpui.
set -euo pipefail

cd "$(dirname "$0")/.."
export PATH="$HOME/.cargo/bin:$PATH"
tauri_app=${1:?usage: scripts/rehearse-bridge.sh /path/to/Tauri/Boa.app [WORKDIR]}
[ -d "$tauri_app/Contents" ] || { echo "error: $tauri_app is not an app bundle" >&2; exit 1; }
work=${2:-$(mktemp -d -t boa-bridge)}
mkdir -p "$work"
work=$(cd "$work" && pwd)
port=${REHEARSAL_PORT:-8766}
base="http://127.0.0.1:$port"
read -r -a tauri_cli <<<"${TAURI_CLI:-npx --yes @tauri-apps/cli@2.12.3}"
step() { echo; echo "==> $*"; }
fail() { echo "BRIDGE REHEARSAL FAILED: $*" >&2; exit 1; }
pids=()
cleanup() { for p in "${pids[@]}"; do kill "$p" 2>/dev/null || true; done; }
trap cleanup EXIT

version=$(scripts/check-version.sh)
plist() { /usr/libexec/PlistBuddy -c "Print :$2" "$1/Contents/Info.plist"; }
from=$(plist "$tauri_app" CFBundleShortVersionString)

step "throwaway key"
mkdir -p "$work/key"
if [ ! -f "$work/key/bridge.key" ]; then
    "${tauri_cli[@]}" signer generate --ci -p throwaway -w "$work/key/bridge.key" >/dev/null
fi
pubkey=$(cat "$work/key/bridge.key.pub")

if [ "${SKIP_BUILD:-0}" != 1 ]; then
    step "GPUI bundle $version (universal)"
    TAURI_SIGNING_PRIVATE_KEY_PATH="$work/key/bridge.key" TAURI_SIGNING_PRIVATE_KEY_PASSWORD=throwaway \
        scripts/bundle.sh --out "$work/gpui" --base-url "$base"
fi
[ -f "$work/gpui/latest.json" ] || fail "no $work/gpui/latest.json"

step "Tauri updater harness"
(cd scripts/bridge-rehearsal && cargo build --release --locked -q)
harness=scripts/bridge-rehearsal/target/release/bridge-rehearsal

step "feed on $base"
python3 -m http.server "$port" --bind 127.0.0.1 --directory "$work/gpui" >"$work/http.log" 2>&1 &
pids+=($!)
for _ in $(seq 50); do curl -fsS "$base/latest.json" >/dev/null 2>&1 && break; sleep 0.1; done

step "Tauri $from -> GPUI $version through tauri-plugin-updater"
rm -rf "$work/install"; mkdir -p "$work/install"
app="$work/install/Boa.app"
ditto "$tauri_app" "$app"
"$harness" "$base/latest.json" "$pubkey" "$app" "$from"

step "the swapped bundle"
[ "$(plist "$app" CFBundleShortVersionString)" = "$version" ] || fail "Info.plist says $(plist "$app" CFBundleShortVersionString)"
[ "$(plist "$app" CFBundleIdentifier)" = dev.boa.desktop ] || fail "bundle id changed"
[ -x "$app/Contents/MacOS/$(plist "$app" CFBundleExecutable)" ] || fail "no executable"
codesign --verify --strict --verbose=1 "$app" || fail "the installed bundle's signature is broken"
lipo -info "$app/Contents/MacOS/boa-desktop"
mkdir -p "$work/config"
BOA_CONFIG_DIR="$work/config" BOA_NO_UPDATE_CHECK=1 BOA_LOG=info \
    "$app/Contents/MacOS/boa-desktop" >"$work/app.log" 2>&1 &
pid=$!
pids+=("$pid")
for _ in $(seq 150); do grep -q "first frame" "$work/app.log" && break; sleep 0.1; done
kill "$pid" 2>/dev/null || true
grep -q "first frame" "$work/app.log" || fail "the GPUI app did not draw (see $work/app.log)"
echo
echo "BRIDGE REHEARSAL PASSED: Tauri $from installed GPUI $version with its own updater; the result runs."
echo "Work folder: $work"
