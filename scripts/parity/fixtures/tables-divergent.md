Alignment markers — goldmark's alignment enum is `AlignLeft = iota + 1`
(Left=1, Right=2, Center=3, None=4), and Go casts it into a renderer switch
that reads 0=left/1=center/2=right, so `:---` renders centred and `:---:`
renders left in Go. Rust reads the alignment correctly.

| Left | Center | Right |
| :--- | :----: | ----: |
| a    |   b    |     c |

An escaped pipe inside a cell — comrak unescapes `\|` to a literal pipe
that stays part of the cell's own content; Go's naive
`strings.Split(line, "|")` cell-splitting has no concept of escaping and
splits on it anyway, misaligning the row's columns.

| a \| b | c |
| ------ | - |
| x      | y |

A pipe inside inline code in a header row — GFM requires `\|` even inside
a code span, so comrak counts the raw, unescaped pipe as a column
separator. That disagrees with the delimiter row's column count, so the
whole construct is rejected and degrades to a plain paragraph. This is
GFM-spec-conformant: Rust follows the spec, Go does something else (Go
still renders some kind of table here).

| `a|b` | c |
| ----- | - |
| x     | y |

The same construct in a body row instead of the header — the table
survives, but the row truncates to the header's established column
count, cutting the code span across two cells and silently dropping the
last column.

| a | b |
| - | - |
| `a|b` | c |

A ragged row with an extra cell — comrak truncates a body row to the
column count the header established; Go's own `strings.Split` cell
splitting keeps the extra cell instead.

| a | b |
| - | - |
| x | y | z |

A table inside a blockquote — Go's container-prefix (`> `) leaks into the
line-splitting Go's table renderer does, corrupting the first cell.

> | a | b |
> | - | - |
> | x | y |

A table inside a list item — same container-prefix leakage as the
blockquote case above, from the list marker's own indentation.

- | a | b |
  | - | - |
  | x | y |

A cell containing a ZWJ family emoji — Go measures cell width per rune,
not per grapheme cluster, so a 7-codepoint ZWJ sequence is counted as 7
cells wide instead of 1.

| Emoji | Label |
| ----- | ----- |
| 👨‍👩‍👧‍👦 | family |

A CJK-heavy table — Go pads a long CJK-containing rendered line's
remaining width with literal TAB bytes instead of spaces, the same
vendored-renderer defect that already excludes `cjk.md` from this gate.

| 名前 | 年齢 |
| ---- | ---- |
| 世界 | 三十 |
