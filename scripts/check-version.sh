#!/usr/bin/env bash
# Print the release version ([workspace.package] version in the root
# Cargo.toml), and optionally check that it matches a tag.
#
#   scripts/check-version.sh            # prints the version
#   scripts/check-version.sh v0.7.0     # ... and fails unless it is 0.7.0 (leading "v" optional)
#
# The GPUI app has one version source (the Tauri app also had
# tauri.conf.json); Info.plist gets it from scripts/bundle.sh.
set -euo pipefail

cd "$(dirname "$0")/.."

version=$(awk '
    /^\[/ { in_pkg = ($0 == "[workspace.package]") }
    in_pkg && /^version[ \t]*=/ {
        v = $0; sub(/^[^"]*"/, "", v); sub(/".*$/, "", v); print v; exit
    }
' Cargo.toml)
[ -n "$version" ] || { echo "error: no version in Cargo.toml [workspace.package]" >&2; exit 1; }
if [ $# -gt 0 ] && [ "$version" != "${1#v}" ]; then
    echo "error: tag $1 does not match Cargo.toml ($version)" >&2
    exit 1
fi
echo "$version"
