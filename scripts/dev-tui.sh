#!/usr/bin/env bash
# Hot-reload dev loop for the TUI: tsc --watch recompiles all packages in the
# background while node --watch restarts the TUI whenever dist/ changes.
# tsc output goes to a log file so it never smears the alt-screen UI.
set -euo pipefail
cd "$(dirname "$0")/.."

log="${TMPDIR:-/tmp}/queohoh-dev-tui-tsc.log"
pnpm -r --parallel exec tsc --watch --preserveWatchOutput >"$log" 2>&1 &
tsc_pid=$!
trap 'kill "$tsc_pid" 2>/dev/null || true' EXIT

echo "tsc --watch running (pid $tsc_pid, log: $log)"
node --watch packages/tui/dist/cli.js
