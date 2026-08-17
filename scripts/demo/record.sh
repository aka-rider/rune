#!/usr/bin/env bash
set -euo pipefail

export TERM="${TERM:-xterm-256color}"

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
OUT="${DEMO_OUT:-$HOME/artifacts/rune}"
SOCKET=runedemo
BINARY="$REPO_ROOT/target/release/rune"
ENTRY_FILE=welcome.md
READY_TEXT="Welcome to Rune"
WORKSPACE=""
ASG=asg

usage() {
  echo "usage: $(basename "$0") --check | <feature>"
  echo
  echo "Records a scripted rune screencast into \$DEMO_OUT (default ~/artifacts/rune)"
  echo "as <feature>.cast, then exports <feature>.svg and <feature>.gif."
  echo
  echo "features:"
  local script name
  for script in "$SCRIPT_DIR"/*.sh; do
    name="$(basename "$script" .sh)"
    if [ "$name" != "record" ]; then
      echo "  $name"
    fi
  done
}

report_tool() {
  if command -v "$1" >/dev/null 2>&1; then
    echo "ok: $1"
  else
    echo "missing: $1"
    return 1
  fi
}

check_tools() {
  local failures=0
  report_tool asciinema || failures=1
  report_tool tmux || failures=1
  report_tool agg || failures=1
  report_tool cargo || failures=1
  if command -v asg >/dev/null 2>&1; then
    echo "ok: asg"
  elif command -v cargo >/dev/null 2>&1; then
    echo "ok: asg (installable via cargo)"
  else
    echo "missing: asg (and no cargo to install it)"
    failures=1
  fi
  return "$failures"
}

ensure_binary() {
  if [ ! -x "$BINARY" ]; then
    (cd "$REPO_ROOT" && cargo build --release)
  fi
}

ensure_asg() {
  if command -v asg >/dev/null 2>&1; then
    ASG=asg
    return 0
  fi
  cargo install asg --version 2.0.2 --locked
  if command -v asg >/dev/null 2>&1; then
    ASG=asg
  else
    ASG="$HOME/.cargo/bin/asg"
  fi
}

keys() {
  local pace="$1"
  shift
  if [ "${1:-}" = "-l" ]; then
    shift
    tmux -L "$SOCKET" send-keys -l -- "$@"
  else
    tmux -L "$SOCKET" send-keys -- "$@"
  fi
  sleep "$pace"
}

capture_pane() {
  tmux -L "$SOCKET" capture-pane -p 2>/dev/null || true
}

recorder_alive_or_die() {
  local recorder_pid="$1" log="$2"
  if ! kill -0 "$recorder_pid" 2>/dev/null; then
    echo "error: asciinema exited early; last lines of $log:" >&2
    tail -n 20 "$log" >&2 || true
    return 1
  fi
}

wait_for_session() {
  local recorder_pid="$1" log="$2" attempt
  for attempt in $(seq 1 50); do
    recorder_alive_or_die "$recorder_pid" "$log"
    if tmux -L "$SOCKET" has-session 2>/dev/null; then
      return 0
    fi
    sleep 0.2
  done
  echo "error: no tmux session on socket $SOCKET after $attempt attempts" >&2
  return 1
}

wait_for_ready() {
  local recorder_pid="$1" log="$2" attempt
  for attempt in $(seq 1 50); do
    recorder_alive_or_die "$recorder_pid" "$log"
    if capture_pane | grep -qF "$READY_TEXT"; then
      return 0
    fi
    sleep 0.2
  done
  echo "error: '$READY_TEXT' not on screen after $attempt attempts" >&2
  return 1
}

wait_for_input() {
  local recorder_pid="$1" log="$2" attempt
  for attempt in $(seq 1 50); do
    recorder_alive_or_die "$recorder_pid" "$log"
    if [ "$(tmux -L "$SOCKET" display-message -p '#{pane_in_mode}' 2>/dev/null)" = "1" ]; then
      tmux -L "$SOCKET" send-keys -X cancel 2>/dev/null || true
    fi
    timeout 2 tmux -L "$SOCKET" send-keys -- Down 2>/dev/null || true
    sleep 0.2
    if capture_pane | grep -qF "Ln " && ! capture_pane | grep -qF "Ln 1,"; then
      rewind_to_top
      return 0
    fi
  done
  echo "error: editor ignored input after $attempt attempts" >&2
  return 1
}

rewind_to_top() {
  local attempt
  for attempt in $(seq 1 50); do
    if capture_pane | grep -qF "Ln 1,"; then
      return 0
    fi
    timeout 2 tmux -L "$SOCKET" send-keys -- Up 2>/dev/null || true
    sleep 0.2
  done
  echo "error: caret did not return to line 1 after $attempt attempts" >&2
  return 1
}

end_session() {
  local recorder_pid="$1"
  kill -TERM "$recorder_pid" 2>/dev/null || true
  local attempt
  for attempt in $(seq 1 25); do
    if ! kill -0 "$recorder_pid" 2>/dev/null; then
      break
    fi
    sleep 0.2
  done
  tmux -L "$SOCKET" kill-server 2>/dev/null || true
}

cleanup() {
  tmux -L "$SOCKET" kill-server 2>/dev/null || true
  if [ -n "$WORKSPACE" ]; then
    rm -rf "$WORKSPACE"
  fi
}

record() {
  local feature="$1"
  local feature_script="$SCRIPT_DIR/$feature.sh"
  if [ ! -f "$feature_script" ]; then
    echo "error: unknown feature '$feature' ($feature_script not found)" >&2
    usage >&2
    exit 1
  fi

  check_tools
  ensure_binary
  ensure_asg

  mkdir -p "$OUT"
  WORKSPACE="$(mktemp -d)"
  trap cleanup EXIT INT TERM
  cp "$SCRIPT_DIR"/fixtures/*.md "$WORKSPACE/"

  local cast="$OUT/$feature.cast"
  local rec_log="$OUT/$feature.rec.log"
  local rec_cmd
  printf -v rec_cmd 'tmux -f %q -L %q new-session -x 100 -y 30 -- %q %q' \
    "$SCRIPT_DIR/tmux.conf" \
    "$SOCKET" "$BINARY" "$WORKSPACE/$ENTRY_FILE"
  asciinema rec --headless --overwrite --window-size 100x30 --idle-time-limit 2 \
    --command "$rec_cmd" "$cast" 2>"$rec_log" &
  local recorder_pid=$!

  wait_for_session "$recorder_pid" "$rec_log"
  wait_for_ready "$recorder_pid" "$rec_log"
  wait_for_input "$recorder_pid" "$rec_log"

  # shellcheck source=/dev/null
  source "$feature_script"

  end_session "$recorder_pid"
  wait "$recorder_pid" || true

  "$ASG" "$cast" "$OUT/$feature.svg" --window --fps 15
  agg "$cast" "$OUT/$feature.gif"

  echo "artifacts:"
  ls -1 "$OUT/$feature".cast "$OUT/$feature".svg "$OUT/$feature".gif
}

case "${1:-}" in
  --check)
    check_tools
    ;;
  -h | --help | "")
    usage
    ;;
  *)
    record "$1"
    ;;
esac
