#!/usr/bin/env bash
# Tears down everything the parity harness started: any ttyd processes
# recorded by serve.sh, the private tmux server, and the run directory.
set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=lib.sh
source "$SCRIPT_DIR/lib.sh"

# Kill recorded ttyd PIDs first (WP2.S2) — they attach as tmux clients, so
# stopping them before the server avoids a client racing a dying session.
if [[ -d "$RUN_ROOT" ]]; then
    for pidfile in "$RUN_ROOT"/ttyd-*.pid; do
        [[ -f "$pidfile" ]] || continue
        pid="$(cat "$pidfile" 2>/dev/null || true)"
        if [[ -n "$pid" ]] && kill -0 "$pid" 2>/dev/null; then
            kill "$pid" 2>/dev/null || true
        fi
        rm -f "$pidfile"
    done
fi

$TM kill-server 2>/dev/null || true

rm -rf "$RUN_ROOT"

exit 0
