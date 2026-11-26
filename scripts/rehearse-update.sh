#!/usr/bin/env bash
# Rehearse an in-app update end to end on this Mac, with a THROWAWAY key and
# a local feed. Nothing is signed with the real key, uploaded or installed
# outside the work folder; the app runs on an empty BOA_CONFIG_DIR.
#
#   scripts/rehearse-update.sh [WORKDIR]     # default: a new mktemp -d folder
#
# 1. generates a throwaway updater key (tauri signer generate);
# 2. builds version N (this tree) and N+1 (a copy with the patch version
#    bumped) with scripts/bundle.sh --arch <this Mac> --features dev-hooks,
#    trusting the throwaway key (BOA_UPDATER_PUBKEY);
# 3. serves N+1's tarball + latest.json with python3 -m http.server on 127.0.0.1;
# 4. tampered feed: runs N (BOA_DEV_UPDATE=1) against a feed whose tarball was
#    altered after signing; N must reject it and stay N;
# 5. good feed: runs N again; it must download, verify, swap itself for N+1
#    and relaunch, and the relaunched N+1 must find nothing newer.
#
# Needs: the Tauri CLI (TAURI_CLI, default npx --yes @tauri-apps/cli@2.12.3), python3.
# The app windows open briefly (they quit by themselves).
set -euo pipefail

cd "$(dirname "$0")/.."
export PATH="$HOME/.cargo/bin:$PATH"
root=$PWD
work=${1:-$(mktemp -d -t boa-rehearsal)}
mkdir -p "$work"
work=$(cd "$work" && pwd)
port=${REHEARSAL_PORT:-8765}
arch=$(uname -m); [ "$arch" = arm64 ] && arch=aarch64
read -r -a tauri_cli <<<"${TAURI_CLI:-npx --yes @tauri-apps/cli@2.12.3}"
step() { echo; echo "==> $*"; }
fail() { echo "REHEARSAL FAILED: $*" >&2; exit 1; }

pids=()
cleanup() {
    for p in "${pids[@]}"; do kill "$p" 2>/dev/null || true; done
    # Any copy of the rehearsal app still running (only this work folder's).
    pkill -f "^$work/install/Boa.app/Contents/MacOS/boa-desktop" 2>/dev/null || true
}
trap cleanup EXIT

step "throwaway key in $work/key"
mkdir -p "$work/key"
"${tauri_cli[@]}" signer generate --ci -f -p throwaway -w "$work/key/rehearsal.key" >/dev/null
export BOA_UPDATER_PUBKEY
BOA_UPDATER_PUBKEY=$(cat "$work/key/rehearsal.key.pub")
export TAURI_SIGNING_PRIVATE_KEY_PATH="$work/key/rehearsal.key"
export TAURI_SIGNING_PRIVATE_KEY_PASSWORD=throwaway

v_n=$(scripts/check-version.sh)
IFS=. read -r major minor patch <<<"${v_n%%-*}"
v_next="$major.$minor.$((patch + 1))"
base="http://127.0.0.1:$port"

step "build N = $v_n"
scripts/bundle.sh --arch "$arch" --features dev-hooks --out "$work/n"

step "build N+1 = $v_next (copy of the tree, version bumped)"
rm -rf "$work/src-next"
mkdir -p "$work/src-next"
rsync -a --exclude target --exclude .git --exclude .worktrees ./ "$work/src-next/"
sed -i '' "s/^version = \"$v_n\"$/version = \"$v_next\"/" "$work/src-next/Cargo.toml"
(cd "$work/src-next" && cargo update --workspace --offline -q)
(cd "$work/src-next" && CARGO_TARGET_DIR="$root/${CARGO_TARGET_DIR:-target}" \
    scripts/bundle.sh --arch "$arch" --features dev-hooks --out "$work/next" --base-url "$base")

step "feeds on $base"
feed="$work/feed"
rm -rf "$feed"; mkdir -p "$feed/tampered"
tgz="Boa_${v_next}_macos-$arch.app.tar.gz"
cp "$work/next/$tgz" "$work/next/latest.json" "$feed/"
# Same signature, one byte of the tarball changed.
cp "$work/next/$tgz" "$feed/tampered/$tgz"
printf '\x00' | dd of="$feed/tampered/$tgz" bs=1 seek=4096 conv=notrunc 2>/dev/null
cmp -s "$feed/$tgz" "$feed/tampered/$tgz" && fail "tampering did not change the tarball"
sed "s#$base/$tgz#$base/tampered/$tgz#" "$work/next/latest.json" > "$feed/tampered/latest.json"
python3 -m http.server "$port" --bind 127.0.0.1 --directory "$feed" >"$work/http.log" 2>&1 &
pids+=($!)
for _ in $(seq 50); do curl -fsS "$base/latest.json" >/dev/null 2>&1 && break; sleep 0.1; done
curl -fsS "$base/latest.json" >/dev/null || fail "the feed server did not start"

install="$work/install"
app="$install/Boa.app"
rm -rf "$install"; mkdir -p "$install" "$work/config"
ditto "$work/n/Boa.app" "$app"
log="$work/update.log"
: > "$log"
plist_version() { /usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' "$app/Contents/Info.plist"; }

run_n() { # <endpoint>: run the installed app until it quits (60 s at most)
    BOA_CONFIG_DIR="$work/config" BOA_DEV_UPDATE=1 BOA_DEV_UPDATE_LOG="$log" \
        BOA_UPDATE_ENDPOINT="$1" BOA_LOG=info \
        "$app/Contents/MacOS/boa-desktop" >>"$work/app.log" 2>&1 &
    local pid=$!
    for _ in $(seq 600); do kill -0 "$pid" 2>/dev/null || return 0; sleep 0.1; done
    kill "$pid" 2>/dev/null || true
    fail "the app (pid $pid) did not quit"
}

step "tampered feed: N must refuse it"
run_n "$base/tampered/latest.json"
grep -q "download rejected: the update's signature is not valid" "$log" || fail "no rejection in $log"
[ "$(plist_version)" = "$v_n" ] || fail "the bundle changed after a rejected update"
codesign --verify --strict "$app" || fail "the bundle's signature broke"
echo "rejected; still $v_n"

step "good feed: N must update itself to N+1 and relaunch"
run_n "$base/latest.json"
grep -q "installed $v_next at $app" "$log" || fail "no install in $log"
[ "$(plist_version)" = "$v_next" ] || fail "Info.plist still says $(plist_version)"
codesign --verify --strict --verbose=1 "$app" || fail "the installed bundle's signature is broken"
# The relaunched instance (started by open -n once N quit) logs and quits.
for _ in $(seq 300); do grep -q "v$v_next\] up to date" "$log" && break; sleep 0.1; done
grep -q "v$v_next\] up to date" "$log" || fail "N+1 did not start (see $log)"
# ... on the rehearsal's config dir (BOA_* passed on through open --env), not the user's.
grep -q "v$v_next\] running $v_next from $app/Contents/MacOS/boa-desktop; config dir $work/config;" "$log" \
    || fail "N+1 did not run from the swapped bundle on $work/config"
leftovers=$(find "$install" -mindepth 1 -maxdepth 1 ! -name Boa.app)
[ -z "$leftovers" ] || fail "left next to the bundle: $leftovers"

step "log"
cat "$log"
echo
echo "REHEARSAL PASSED: $v_n rejected a tampered $v_next, then updated itself to $v_next and relaunched."
echo "Work folder: $work"
