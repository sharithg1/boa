#!/usr/bin/env bash
# Write latest.json, the in-app update feed, to stdout, in the shape the Tauri
# app's release workflow wrote it (and tauri-plugin-updater 2.x / the GPUI
# updater read it):
#
#   {"version", "notes", "pub_date", "platforms": {"darwin-aarch64": {url, signature}, "darwin-x86_64": {…}}}
#
#   scripts/latest-json.sh DIR VERSION BASE_URL [NOTES_URL]
#
# DIR holds Boa_<VERSION>_macos-<arch>.app.tar.gz and its .sig for aarch64 and
# x86_64, or one universal tarball that then serves both. Each url is
# BASE_URL/<file name>. ARCHS (default "aarch64 x86_64") lists the platforms
# that must be present; a release needs both.
set -euo pipefail

[ $# -ge 3 ] || { sed -n '2,13p' "$0" | sed 's/^# \{0,1\}//'; exit 1; }
dir=$1 version=$2 base=${3%/} notes=${4:-$3}

platforms='{}'
for arch in ${ARCHS:-aarch64 x86_64}; do
    found=""
    for candidate in "$arch" universal; do
        tgz="Boa_${version}_macos-$candidate.app.tar.gz"
        if [ -f "$dir/$tgz" ] && [ -f "$dir/$tgz.sig" ]; then
            found=$tgz
            break
        fi
    done
    [ -n "$found" ] || { echo "error: no signed update tarball for $arch in $dir" >&2; exit 1; }
    platforms=$(jq --arg k "darwin-$arch" --arg url "$base/$found" --rawfile sig "$dir/$found.sig" \
        '. + {($k): {url: $url, signature: $sig}}' <<<"$platforms")
done
jq -n --arg version "$version" --arg date "$(date -u +%Y-%m-%dT%H:%M:%SZ)" \
    --arg notes "$notes" --argjson platforms "$platforms" \
    '{version: $version, notes: $notes, pub_date: $date, platforms: $platforms}'
