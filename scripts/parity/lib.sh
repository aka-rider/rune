#!/usr/bin/env bash
# Shared helpers for the Go-vs-Rust visual-parity harness. Sourced, never
# executed directly.
#
# Every tmux invocation in this toolchain MUST go through $TM: a bare `tmux`
# would touch the user's live server and ~/.tmux.conf.
#
# This file deliberately sets NO shell options. `source` runs in the caller's
# shell, so a `set -e` here silently overrides the policy the sourcing script
# declared on its own first lines — and assert.sh/clean.sh switch `-e` OFF on
# purpose (they must survive a failing check to report it). When lib.sh set
# `-euo pipefail`, a `grep -o | wc -l` count that legitimately found zero
# matches aborted assert.sh mid-run: no FAIL line, and six of its eight gates
# silently skipped. Every script here declares its own `set` line; leave that
# decision to them.

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

RUN_ROOT="${PARITY_RUN:-$REPO_ROOT/.scratch/parity/run}"
OUT_DIR="${PARITY_OUT:-$REPO_ROOT/.scratch/parity/out}"

COLS="${PARITY_COLS:-120}"
ROWS="${PARITY_ROWS:-34}"

TM="tmux -L rune-parity -f /dev/null"

session_name() {
    echo "rune-parity-$1"
}

# wait_for_pane <session> <pattern> <timeout_seconds>
#
# Polls `tmux capture-pane` for <session> until <pattern> (a plain grep
# pattern, not necessarily a regex-escaped literal) appears, or the bounded
# deadline (computed from bash's own $SECONDS builtin) elapses. The poll
# interval (0.05s) is not a "let it settle" sleep — it is a bounded-deadline
# predicate poll, the only waiting mechanism this harness ever uses.
wait_for_pane() {
    local session="$1"
    local pattern="$2"
    local timeout="$3"
    local start=$SECONDS
    local captured=""
    while (( SECONDS - start < timeout )); do
        captured="$($TM capture-pane -p -t "$session" 2>/dev/null || true)"
        if grep -q -- "$pattern" <<<"$captured"; then
            return 0
        fi
        sleep 0.05
    done
    echo "wait_for_pane: timed out waiting for '$pattern' on session '$session' after ${timeout}s" >&2
    echo "--- last captured pane ---" >&2
    echo "$captured" >&2
    echo "--------------------------" >&2
    exit 1
}
