#!/usr/bin/env bash
# The parity harness's GATE (unlike diff.sh, a report): mechanical chrome
# invariants checked against the last captured screens. Exits non-zero if
# any check fails, printing PASS/FAIL with the offending line for each.
#
# Go-side gates (plan WP1) plus the Rust-side gates (plan WP4.S8) proving
# the border + spliced breadcrumb fixes landed.
set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=lib.sh
source "$SCRIPT_DIR/lib.sh"

FAIL=0

check() {
    local desc="$1"
    local ok="$2"
    local detail="${3:-}"
    if [[ "$ok" == "0" ]]; then
        echo "PASS: $desc"
    else
        echo "FAIL: $desc${detail:+ — $detail}"
        FAIL=1
    fi
}

GO_TXT="$OUT_DIR/go.txt"
RUST_TXT="$OUT_DIR/rust.txt"

if [[ -s "$GO_TXT" ]]; then
    check "go.txt is non-empty" 0
else
    check "go.txt is non-empty" 1 "$GO_TXT missing or empty"
fi

if [[ -s "$RUST_TXT" ]]; then
    check "rust.txt is non-empty" 0
else
    check "rust.txt is non-empty" 1 "$RUST_TXT missing or empty"
fi

# Go top row has both the left-pane and the center-pane top-left corners.
if [[ -s "$GO_TXT" ]]; then
    GO_TOP_CORNERS="$(head -1 "$GO_TXT" | grep -o '╭' | wc -l | tr -d ' ')"
    if [[ "$GO_TOP_CORNERS" == "2" ]]; then
        check "go top row has 2 '╭' corners (left pane + center pane)" 0
    else
        check "go top row has 2 '╭' corners (left pane + center pane)" 1 \
            "found $GO_TOP_CORNERS: $(head -1 "$GO_TXT")"
    fi
fi

# Go's bottom content row ends in the breadcrumb's right corner.
BOTTOM_LINE_NO=$((ROWS - 1))
if [[ -s "$GO_TXT" ]]; then
    GO_BOTTOM="$(sed -n "${BOTTOM_LINE_NO}p" "$GO_TXT")"
    if grep -qE 'sample\.md +──╯[[:space:]]*$' <<<"$GO_BOTTOM"; then
        check "go bottom content row (line $BOTTOM_LINE_NO) ends 'sample.md ──╯'" 0
    else
        check "go bottom content row (line $BOTTOM_LINE_NO) ends 'sample.md ──╯'" 1 "$GO_BOTTOM"
    fi
fi

# Rust top row now has the same 2 top-left corners as Go (left pane +
# center pane) — plan WP4.S8, proving Bug 1 (missing border) is fixed.
if [[ -s "$RUST_TXT" ]]; then
    RUST_TOP_CORNERS_L="$(head -1 "$RUST_TXT" | grep -o '╭' | wc -l | tr -d ' ')"
    if [[ "$RUST_TOP_CORNERS_L" == "2" ]]; then
        check "rust top row has 2 '╭' corners (left pane + center pane)" 0
    else
        check "rust top row has 2 '╭' corners (left pane + center pane)" 1 \
            "found $RUST_TOP_CORNERS_L: $(head -1 "$RUST_TXT")"
    fi

    RUST_TOP_CORNERS_R="$(head -1 "$RUST_TXT" | grep -o '╮' | wc -l | tr -d ' ')"
    if [[ "$RUST_TOP_CORNERS_R" == "2" ]]; then
        check "rust top row has 2 '╮' corners (left pane + center pane)" 0
    else
        check "rust top row has 2 '╮' corners (left pane + center pane)" 1 \
            "found $RUST_TOP_CORNERS_R: $(head -1 "$RUST_TXT")"
    fi
fi

# Rust's bottom content row ends in the breadcrumb's right corner — same
# shape as Go's, proving Bug 2 (breadcrumb spliced onto the border) is
# fixed.
if [[ -s "$RUST_TXT" ]]; then
    RUST_BOTTOM="$(sed -n "${BOTTOM_LINE_NO}p" "$RUST_TXT")"
    if grep -qE 'sample\.md +──╯[[:space:]]*$' <<<"$RUST_BOTTOM"; then
        check "rust bottom content row (line $BOTTOM_LINE_NO) ends 'sample.md ──╯'" 0
    else
        check "rust bottom content row (line $BOTTOM_LINE_NO) ends 'sample.md ──╯'" 1 "$RUST_BOTTOM"
    fi
fi

# The title row (now inside the border, row 2) still shows the file name.
if [[ -s "$RUST_TXT" ]]; then
    RUST_TITLE="$(sed -n '2p' "$RUST_TXT")"
    if grep -q 'sample.md' <<<"$RUST_TITLE"; then
        check "rust title row (line 2) contains 'sample.md'" 0
    else
        check "rust title row (line 2) contains 'sample.md'" 1 "$RUST_TITLE"
    fi
fi

exit $FAIL
