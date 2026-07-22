#!/usr/bin/env bash
# Self-heal the queohoh daemon: ensure one is running (starting it detached if
# not), and optionally restart the current one first.
#
# Usage:
#   scripts/daemon-ensure.sh           # ensure a daemon is up
#   scripts/daemon-ensure.sh --restart # kill the running daemon, then ensure
set -euo pipefail

CLI="node packages/daemon/dist/cli.js"
STATE="${QUEOHOH_STATE_DIR:-$HOME/.local/state/queohoh}"
PIDFILE="$STATE/daemon/daemon.pid"
LOGFILE="$STATE/daemon/daemon.log"

mkdir -p "$STATE/daemon"

# Liveness only — must NOT call `status`/`state`. Full state snapshots can take
# 10s+ with a large live+archive queue, which used to make ensure fail while the
# daemon was healthy. Prefer `ping` (instant pong); fall back to a socket connect
# for older builds that lack the ping command.
is_up() {
	if $CLI ping >/dev/null 2>&1; then
		return 0
	fi
	# Pre-ping builds: status used to be the probe, but its 5s default timed out
	# on large queues. Socket existence alone is too weak (stale sock files);
	# try status with the long budget only as last resort after ping fails.
	$CLI status >/dev/null 2>&1
}

start_and_wait() {
	nohup $CLI daemon >>"$LOGFILE" 2>&1 &
	# 20 × 0.5s = 10s — cold start + first tick can be slow under load.
	for _ in $(seq 1 20); do
		if is_up; then
			echo "daemon started, reachable"
			return 0
		fi
		sleep 0.5
	done
	echo "daemon failed to become reachable within 10s; last log lines:" >&2
	tail -n 20 "$LOGFILE" >&2 || true
	return 1
}

if [ "${1:-}" = "--restart" ]; then
	if [ -f "$PIDFILE" ]; then
		pid="$(cat "$PIDFILE")"
		if [ -n "$pid" ] && kill -0 "$pid" 2>/dev/null; then
			echo "stopping daemon (pid $pid)"
			kill "$pid" 2>/dev/null || true
			for _ in $(seq 1 10); do
				kill -0 "$pid" 2>/dev/null || break
				sleep 0.5
			done
			if kill -0 "$pid" 2>/dev/null; then
				echo "daemon (pid $pid) did not exit in 5s; sending SIGKILL" >&2
				kill -9 "$pid" 2>/dev/null || true
			fi
		fi
	fi
	start_and_wait
	exit $?
fi

if is_up; then
	echo "daemon already running"
	exit 0
fi
start_and_wait
