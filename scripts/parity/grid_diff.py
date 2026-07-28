#!/usr/bin/env python3
"""Glyph-grid diff helper for scripts/parity/grid.sh (plan WP1.S4).

Compares the CENTER PANE's own content rows (the editor viewport: not the
left Explorer/Open pane, not the title/breadcrumb/footer chrome rows)
between a captured Go screen and a captured Rust screen, for one fixture.
Scoped to the editor content on purpose: the left-pane shape, title text,
footer content, caret rendering, and breadcrumb path-relativization are
ALL already-documented, permanent chrome divergences (see README's "Known
divergences") that have nothing to do with markdown wrap/conceal/width —
the thing this gate exists to catch (plan: "Fixtures must exercise where
wrap/conceal/width bugs live"). Diffing the whole screen would fail every
fixture for the same already-known chrome reasons and never test anything
new.

Column cropping is done by character INDEX, not display width, but that
is safe here: the split-column search happens on row 0 (pure ASCII box-
drawing + an optional ASCII label — never a wide glyph), and the right
edge is simply "everything up to the row's own last character" (the
pane's own right border glyph, always a single, single-width character) —
so no wcwidth accounting is needed even though CJK/emoji fixtures put
wide glyphs inside the cropped content itself.

Usage: grid_diff.py <go.txt> <rust.txt> <rows> <cols> <fixture-name> <out-diff>
Exit 0 if the cropped content grids match, 1 otherwise (writing a unified
diff to <out-diff>).
"""

import re
import sys
import difflib

# Go marks a trailing empty line past the document's own content with a
# vi-style '~' in the editor's left margin; Rust leaves it blank (recorded
# in README's "Known divergences" — a cosmetic difference, unrelated to
# wrap/conceal/width, that would otherwise show up on nearly every
# fixture shorter than the viewport). Normalized away here before
# comparing rather than excluding every short fixture over it.
_TILDE_FILLER = re.compile(r"^~\s*$")


def normalize(line: str) -> str:
    return " " * len(line) if _TILDE_FILLER.match(line) else line


def load_rows(path: str, rows: int, cols: int) -> list[str]:
    with open(path, encoding="utf-8") as f:
        lines = f.read().splitlines()
    # Defensive right-pad (plan WP1.S4): tmux capture-pane trims a truly
    # blank tail, which should never happen here since every content row
    # ends in the pane's own border glyph — but pad anyway rather than
    # trust that invariant silently.
    lines = [line.ljust(cols) for line in lines]
    while len(lines) < rows:
        lines.append(" " * cols)
    return lines[:rows]


def split_column(row0: str) -> int:
    """Index of the 2nd top-left corner glyph in the chrome's top border
    row — the column where the CENTER pane begins. Row 0 is ASCII box-
    drawing plus an optional ASCII label (Rust's "Files"), so a plain
    character index equals a display column here (see module docstring).
    """
    first = row0.find("╭")  # '╭'
    second = row0.find("╭", first + 1)
    if first == -1 or second == -1:
        raise ValueError(f"expected two top-left corners in row 0, got: {row0!r}")
    return second


def content_region(lines: list[str], top: int, bottom: int, left: int) -> list[str]:
    """Rows `top..bottom` (exclusive), each cropped to the CENTER pane's
    own content — from just after its left border to just before its own
    right border (the row's last character, always the border glyph)."""
    return [normalize(line[left:-1]) for line in lines[top:bottom]]


def main() -> int:
    go_path, rust_path, rows_s, cols_s, fixture, out_diff = sys.argv[1:7]
    rows = int(rows_s)
    cols = int(cols_s)

    go_lines = load_rows(go_path, rows, cols)
    rust_lines = load_rows(rust_path, rows, cols)

    go_split = split_column(go_lines[0])
    rust_split = split_column(rust_lines[0])

    # Content rows: row 0 is the top border, row 1 the title, the last two
    # rows are the bottom border(+breadcrumb) and the footer — everything
    # else in between is the editor's own content (see grid.sh for the
    # ROWS-4 derivation shared with assert.sh's own chrome math).
    top, bottom = 2, rows - 2

    go_content = content_region(go_lines, top, bottom, go_split + 1)
    rust_content = content_region(rust_lines, top, bottom, rust_split + 1)

    if go_content == rust_content:
        return 0

    diff = difflib.unified_diff(
        [line + "\n" for line in go_content],
        [line + "\n" for line in rust_content],
        fromfile=f"go/{fixture}",
        tofile=f"rust/{fixture}",
    )
    with open(out_diff, "w", encoding="utf-8") as f:
        f.writelines(diff)
    return 1


if __name__ == "__main__":
    sys.exit(main())
