# WP14A — Markdown Inline & Line Preview

## Scope

Upgrade `pkg/editor/display.SyntaxMap` from pass-through for parser selection plus the safest live-preview subset:

- headings
- bold, italic, strikethrough
- inline code
- links as rendered link text only
- blockquotes
- task list markers
- horizontal rules

This workpackage must preserve all WP7 coordinate round-trip invariants while adding delimiter folding and reveal behavior.

## Dependencies

- WP7 (display pipeline)
- WP8 (editor component integration)
- WP09 (cursor movement for reveal transitions)

## Inputs

Read `editor-spec.md` rendering rules, `qa-instructions.md`, `qa-implementation-specs.md`, WP14 parent, and the current `pkg/editor/display` implementation.

## Deliverables

### Parser Spike

- Add a focused parser proof test before committing to a parser dependency.
- Prove byte-accurate source ranges for every element in this WP.
- If the parser cannot provide ranges for one element, stop and document the blocker rather than guessing ranges with ad hoc string scans.

### SyntaxMap Elements

- Headings: hide leading `#` markers and following space when cursor is away from the heading line.
- Bold/italic/strikethrough: hide delimiters when cursor is outside the token range; reveal raw syntax when cursor is inside `[start,end)`.
- Inline code: hide backticks when cursor is outside token range.
- Links: show link text only when rendered; reveal full `[text](url)` when cursor is in token range.
- Blockquotes: hide `>` marker when cursor is away from the line.
- Task lists: show checkbox glyph/state while preserving list marker semantics.
- Horizontal rules: render as a semantic divider span, not terminal drawing in the domain package.

### Coordinate Deltas

- Build monotonic `OffsetDelta` entries for every hidden delimiter range.
- `BufferToSyntax` clamps positions inside hidden delimiters to cursor-legal visible positions per `editor-spec.md`.
- `SyntaxToBuffer(BufferToSyntax(bp)) == bp` for cursor-legal positions only.

## Non-Goals

- Code fences, tables, math, frontmatter, callouts, embeds, image rendering, and terminal graphics.
- Lipgloss styling in `pkg/editor/display`.
- Incremental parsing optimization.

## Tests

- Parser source-range proof tests for each supported element.
- Rendered/revealed table tests for heading, bold, italic, inline code, link, blockquote, task list, horizontal rule.
- Coordinate round-trips for cursor-legal positions.
- Hidden-delimiter clamp tests.
- Reveal transition test: cursor enters and exits `**bold**` without changing buffer offset.
- Scroll stability integration test in editor component if line width changes after reveal.

## QA Gates

| # | Gate | Harm Prevented |
|---|---|---|
| 1 | Parser source ranges are byte-accurate for every WP14A element | Guessed ranges make cursor/display mapping lie |
| 2 | Offset deltas are sorted and monotonic | Binary search conversion returns wrong positions |
| 3 | Cursor inside token reveals only that token, not siblings | Markdown preview flickers or over-reveals unrelated text |
| 4 | Line-level reveal reveals only the cursor line | Moving through one heading changes unrelated lines |
| 5 | Domain package emits semantic spans only, no lipgloss | UI/domain boundary stays clean |

## Verification

```bash
go test ./pkg/editor/display/ -run 'TestSyntaxMap|TestMarkdownInline|TestMarkdownLine' -v
go test ./pkg/ui/components/editor/ -run TestMarkdownReveal -v
```
