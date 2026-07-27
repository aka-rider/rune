#!/usr/bin/env bash
# Diffs the last captured go/rust screens and writes a report. Always exits
# 0 — this is a report, never a gate (parity-assert is the gate).
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=lib.sh
source "$SCRIPT_DIR/lib.sh"

mkdir -p "$OUT_DIR"
diff -u "$OUT_DIR/go.txt" "$OUT_DIR/rust.txt" > "$OUT_DIR/diff.txt" 2>&1 || true

LINES="$(wc -l < "$OUT_DIR/diff.txt" | tr -d ' ')"
echo "parity diff: $LINES line(s) — see $OUT_DIR/diff.txt"

exit 0
