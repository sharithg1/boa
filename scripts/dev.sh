#!/usr/bin/env bash
# Dev loop: rebuild and relaunch Boa whenever a source file changes. A native
# GPUI app has no hot module reload, so this is the closest thing: save, and
# the app restarts on the new build a couple of seconds later.
#
#   scripts/dev.sh                         # debug build with dev-hooks
#   scripts/dev.sh --features screenshot   # extra `cargo run` arguments
#   BOA_CONFIG_DIR= scripts/dev.sh         # use your real config dir instead
#
# Needs watchexec (brew install watchexec). Edits to tests (tests.rs, tests/)
# don't restart the app. The window opens normally, so it takes the focus on
# each restart; add BOA_DEV_BACKGROUND=1 to keep it hidden.
set -euo pipefail

cd "$(dirname "$0")/.."
export PATH="$HOME/.cargo/bin:$PATH"

if ! command -v watchexec >/dev/null; then
    echo "scripts/dev.sh needs watchexec: brew install watchexec" >&2
    exit 1
fi

# A separate config dir by default, so dev builds never touch your saved
# connections or history; BOA_DEV_CONNECTION seeds "Local dev" (localhost
# boa/boa, boa_dev) into it the first time. Set either one to empty to opt out.
export BOA_CONFIG_DIR="${BOA_CONFIG_DIR-$PWD/target/dev-config}"
export BOA_DEV_CONNECTION="${BOA_DEV_CONNECTION-1}"
export BOA_NO_UPDATE_CHECK="${BOA_NO_UPDATE_CHECK-1}"

exec watchexec --restart --debounce 300ms \
    --watch crates --watch Cargo.toml --watch Cargo.lock \
    --ignore '**/tests.rs' --ignore '**/tests/**' \
    -- cargo run -p boa-app --features dev-hooks "$@"
