#!/usr/bin/env bash
# Serves one side's already-captured, pinned tmux session over ttyd so a
# browser (the Playwright MCP tools, running in Docker) can screenshot it.
# Requires `make parity-capture` (or capture.sh) to have run first — the
# session must already exist.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=lib.sh
source "$SCRIPT_DIR/lib.sh"

SIDE="${1:?usage: serve.sh <go|rust>}"

case "$SIDE" in
    go)   PORT="${PARITY_PORT_GO:-7681}" ;;
    rust) PORT="${PARITY_PORT_RUST:-7682}" ;;
    *) echo "serve.sh: SIDE must be 'go' or 'rust', got '$SIDE'" >&2; exit 1 ;;
esac

S="$(session_name "$SIDE")"

if ! $TM has-session -t "$S" 2>/dev/null; then
    echo "serve.sh: no tmux session '$S' — run 'make parity-capture' first" >&2
    exit 1
fi

mkdir -p "$RUN_ROOT"
PIDFILE="$RUN_ROOT/ttyd-$SIDE.pid"
LOGFILE="$RUN_ROOT/ttyd-$SIDE.log"

# ttyd is deliberately left running after this script exits (WP2: it keeps
# serving until clean.sh kills it) — its stdout/stderr must NOT inherit this
# script's own fds, or any caller that pipes serve.sh's output (`make
# parity-serve | tail`, a CI log capture, ...) blocks forever waiting for
# EOF on that pipe, since the still-running ttyd process holds the write
# end open indefinitely. Redirect to a log file instead.
# shellcheck disable=SC2086
ttyd -W -p "$PORT" -t fontSize=14 $TM attach -r -t "$S" >"$LOGFILE" 2>&1 &
ttyd_pid=$!
echo "$ttyd_pid" > "$PIDFILE"

start=$SECONDS
until curl -sf -o /dev/null "http://127.0.0.1:$PORT/"; do
    if (( SECONDS - start > 20 )); then
        echo "serve.sh: ttyd never came up on port $PORT after 20s" >&2
        exit 1
    fi
    sleep 0.05
done

echo "serving '$SIDE' on:"
echo "  http://127.0.0.1:$PORT/            (human)"
echo "  http://host.docker.internal:$PORT/ (containerised browser)"
