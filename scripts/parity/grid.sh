#!/usr/bin/env bash
# The glyph-grid parity GATE (like assert.sh, unlike diff.sh — this exits
# non-zero on failure): for each markdown fixture under fixtures/, captures
# both sides through the existing capture.sh path (scenario 01-open-file,
# which brings both sides to the same two-pane chrome geometry — plan
# gotcha: Rust's left pane defaults hidden, Go's does not), then diffs the
# CENTER PANE's own content rows only (see grid_diff.py's module doc for
# why the comparison is scoped there and not to the whole screen).
#
# EXCLUDED_FIXTURES below records fixtures with a known, understood
# Go/Rust divergence in the region this gate compares — each is skipped
# here with its reason inlined, and the same divergence is written up in
# README.md's "Known divergences" section (plan WP1.S6). This is NOT
# papering over a difference: every excluded fixture was captured and
# diffed for real, its cause identified, and is skipped deliberately
# rather than asserted against — see the diffs this script's own dev
# session captured under .scratch/parity/out/grid-<fixture>.diff.
set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=lib.sh
source "$SCRIPT_DIR/lib.sh"

SCENARIO="01-open-file"

# Every markdown fixture under fixtures/ except sample.md (the pre-existing
# chrome-parity fixture, covered by assert.sh already).
FIXTURES=(
    headings.md
    emphasis.md
    lists.md
    tasks.md
    fences.md
    quotes.md
    tables.md
    tables-divergent.md
    tables-narrow.md
    frontmatter.md
    cjk.md
    emoji.md
    fences-code.md
)

# fixture -> reason, one real, verified cause each (README.md "Known
# divergences" has the full write-up for each). A plain case/esac rather
# than an associative array: the macOS-shipped /bin/bash (3.2) that `env
# bash` resolves to on this platform has no `declare -A` (added in bash
# 4.0) — see plan CLAUDE.md, "macOS (Apple Silicon) only".
excluded_reason() {
    case "$1" in
        headings.md)
            echo "Go's inline-emphasis concealment does not recurse into a heading's own text (bold/code inside a heading stay raw in Go, concealed in Rust)"
            ;;
        lists.md)
            echo "Go never conceals plain bullet/ordered list markers (vestigial in Go's own walker); Rust conceals them"
            ;;
        tasks.md)
            echo "same list-marker gap as lists.md (the checkbox-glyph defect this fixture also caught is fixed — see TODO.md)"
            ;;
        fences.md)
            echo "Go leaves a fenced code block's info string (e.g. 'rust') as visible text after stripping backticks; Rust conceals the whole fence delimiter line"
            ;;
        quotes.md)
            echo "Go's blockquote-marker concealment doesn't recurse into nested (depth >= 2) quotes or inline emphasis nested inside quoted text; Rust conceals both fully"
            ;;
        tables-divergent.md)
            echo "verified Go table defects: inverted alignment (goldmark's AlignLeft = iota + 1 cast into a renderer switch reading 0=left/1=center/2=right, so :--- renders centred and :---: renders left), strings.Split(line, \"|\") cell splitting (mishandles an escaped pipe and a ragged extra cell), container-prefix leakage inside a blockquote/list item, per-rune rather than per-grapheme emoji width, and the cjk.md TAB-padding defect"
            ;;
        frontmatter.md)
            echo "same list-marker gap as lists.md (the body's own bullet list)"
            ;;
        cjk.md)
            echo "same list-marker gap as lists.md, plus Go pads a long CJK-containing line's remaining width with literal TAB bytes instead of spaces (root cause identified — a vendored bubbletea renderer fork's hard-tab optimization, see TODO.md — not fixed, third-party dependency behavior)"
            ;;
        emoji.md)
            echo "same list-marker gap as lists.md (the ZWJ/skin-tone-modifier corruption this fixture also caught is fixed — see TODO.md)"
            ;;
        fences-code.md)
            echo "Go has no tree-sitter integration at all, so its fenced code blocks never gain token colours — there is nothing on the Go side to assert this fixture's Rust-only tree-sitter highlighting against (plan WP6.S4)"
            ;;
        *)
            return 1
            ;;
    esac
}

FAIL=0

check() {
    local desc="$1"
    local ok="$2"
    if [[ "$ok" == "0" ]]; then
        echo "PASS: $desc"
    else
        echo "FAIL: $desc"
        FAIL=1
    fi
}

mkdir -p "$OUT_DIR"

for fixture in "${FIXTURES[@]}"; do
    if reason="$(excluded_reason "$fixture")"; then
        echo "SKIP: $fixture — $reason"
        continue
    fi

    if ! "$SCRIPT_DIR/capture.sh" go "$SCENARIO" "$fixture"; then
        check "$fixture: go capture" 1
        continue
    fi
    if ! "$SCRIPT_DIR/capture.sh" rust "$SCENARIO" "$fixture"; then
        check "$fixture: rust capture" 1
        continue
    fi

    DIFF_OUT="$OUT_DIR/grid-$fixture.diff"
    rm -f "$DIFF_OUT"
    if python3 "$SCRIPT_DIR/grid_diff.py" "$OUT_DIR/go.txt" "$OUT_DIR/rust.txt" "$ROWS" "$COLS" "$fixture" "$DIFF_OUT"; then
        check "$fixture: editor content grid matches" 0
    else
        check "$fixture: editor content grid matches" 1
        echo "  see $DIFF_OUT"
    fi
done

exit $FAIL
