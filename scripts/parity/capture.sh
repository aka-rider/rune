#!/usr/bin/env bash
# Launches one side (go|rust) of the parity harness in a pinned, private
# tmux session, drives the scenario's keys, and captures the resulting
# screen (plain text and ANSI) into $OUT_DIR.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=lib.sh
source "$SCRIPT_DIR/lib.sh"

SIDE="${1:?usage: capture.sh <go|rust> [scenario]}"
SCENARIO="${2:-01-open-file}"

case "$SIDE" in
    go|rust) ;;
    *) echo "capture.sh: SIDE must be 'go' or 'rust', got '$SIDE'" >&2; exit 1 ;;
esac

SCENARIO_DIR="$SCRIPT_DIR/scenarios/$SCENARIO"
KEYS_FILE="$SCENARIO_DIR/$SIDE.keys"
if [[ ! -f "$KEYS_FILE" ]]; then
    echo "capture.sh: no keys file at $KEYS_FILE" >&2
    exit 1
fi

S="$(session_name "$SIDE")"

# Kill any prior session for this side and wipe its workspace.
$TM kill-session -t "$S" 2>/dev/null || true
WS_ROOT="$RUN_ROOT/$SIDE"
rm -rf "$WS_ROOT"
WS="$WS_ROOT/parityws"
mkdir -p "$WS"
cp "$SCRIPT_DIR/fixtures/sample.md" "$WS/sample.md"

# Go writes .rune/ into its workspace on launch; pre-create an identical one
# in both workspaces so the file trees list the same entries (gotcha 8).
mkdir -p "$WS/.rune"
printf '*\n' > "$WS/.rune/.gitignore"

if [[ "$SIDE" == "rust" ]]; then
    # Rust's HOME must be outside the captured workspace dir, or its
    # Library/... tree shows up in the file tree (gotcha 7).
    mkdir -p "$RUN_ROOT/rust/home"
fi

case "$SIDE" in
    go)
        BIN="$REPO_ROOT/rune"
        if [[ ! -x "$BIN" ]]; then
            echo "capture.sh: Go binary missing at $BIN — run 'make build' first" >&2
            exit 1
        fi
        CMD=(env TERM=xterm-256color "$BIN" -w "$WS" "$WS/sample.md")
        ;;
    rust)
        BIN="$REPO_ROOT/rust/target/debug/rune"
        if [[ ! -x "$BIN" ]]; then
            echo "capture.sh: Rust binary missing at $BIN — run 'make rust-build' first" >&2
            exit 1
        fi
        CMD=(env TERM=xterm-256color HOME="$RUN_ROOT/rust/home" "$BIN" "$WS/sample.md")
        ;;
esac

$TM new-session -d -s "$S" -x "$COLS" -y "$ROWS" "${CMD[@]}"
$TM set-option -t "$S" -w window-size manual
$TM resize-window -t "$S" -x "$COLS" -y "$ROWS"

wait_for_pane "$S" 'sample.md' 20

while IFS= read -r line || [[ -n "$line" ]]; do
    [[ -z "$line" ]] && continue
    [[ "$line" == \#* ]] && continue
    $TM send-keys -t "$S" "$line"
done < "$KEYS_FILE"

wait_for_pane "$S" 'parityws' 20

mkdir -p "$OUT_DIR"
$TM capture-pane -p -t "$S" > "$OUT_DIR/$SIDE.txt"
$TM capture-pane -p -e -t "$S" > "$OUT_DIR/$SIDE.ansi"

if [[ "${PARITY_KEEP:-1}" == "0" ]]; then
    $TM kill-session -t "$S" 2>/dev/null || true
fi
