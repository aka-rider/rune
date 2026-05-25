# WP14D — Code Fence Syntax Highlighting

## Scope

Implement UI-side syntax highlighting for rendered markdown code fences using the semantic code-fence spans and language metadata produced by WP14B.

This package owns the `editor-spec.md` requirement that rendered code fences appear as syntax-highlighted blocks with a background and language label. It must not move styling concerns into `pkg/editor/display`.

## Dependencies

- WP14B (semantic code-fence spans with language metadata)
- WP8B (editor renderer and style mapping)

## Deliverables

### Renderer Integration

Define a testable highlighter adapter at the UI layer:

```go
type HighlightSpan struct {
	Text  string
	Class string // e.g. "keyword", "string", "comment", "plain"
}

type CodeHighlighter func(language, source string) ([]HighlightSpan, error)
```

Production wiring uses `github.com/charmbracelet/ultraviolet` behind this adapter. Tests inject a deterministic adapter or inspect the adapter's structured spans before terminal styling is rendered.

- In `pkg/ui/components/editor`, detect rendered `TokenCodeFence` display spans and apply UI-side code block rendering.
- Show a language label when the fence declares a language.
- Apply a code-block background style through `styles.Styles`.
- Apply syntax highlighting in the UI renderer using `github.com/charmbracelet/ultraviolet` unless a deliberate replacement is documented in the same workpackage.
- Revealed code fences must show raw source with fence markers and should not apply token-level syntax coloring over the raw markdown delimiters.

### Fallbacks

- Unknown language: render with code-block background and no token coloring.
- Highlighter unavailable or errors: render plain code-block text with background; do not fail display.
- No ANSI-byte golden tests. Golden tests strip or normalize ANSI and assert semantic content when possible.

## Non-Goals

- Markdown parsing or code-fence source-range detection.
- Terminal image rendering.
- Moving lipgloss or syntax-highlighter types into `pkg/editor/display`.

## Tests

- Preflight fixture: consume grouped `TokenCodeFence` display spans produced by WP14B from a real multi-line markdown fenced block, including `Language`, line-local `Text`, `State`, line-local source byte ranges, and shared `BlockID`/`BlockStart`/`BlockEnd` fields.
- Go code fence renders with language label and proof of token-level highlighting through structured `HighlightSpan` output. Golden tests may normalize ANSI, but unit tests assert that known Go tokens (for example `func`, string literals, and comments) receive distinct classes/styles before rendering.
- Unknown language falls back to plain code block.
- Cursor inside fence reveals raw fence and disables rendered syntax highlighting.
- Golden tests compare normalized/semantic output, not raw ANSI escape bytes.

## QA Gates

| # | Gate | Harm Prevented |
|---|---|---|
| 1 | Rendered Go code fence has background, language label, and distinct highlighted token spans for keyword/string/comment samples | Spec-required code fence preview is incomplete |
| 2 | Unknown language falls back without error | User opens uncommon fenced code and editor breaks |
| 3 | Revealed fence shows raw delimiters without conflicting highlighting | User cannot edit code fence syntax accurately |
| 4 | Domain display package remains free of UI/highlighter imports | Architecture boundary is preserved |

## Verification

```bash
go test ./pkg/ui/components/editor/ -run 'TestCodeFenceHighlight|TestGolden_CodeFence' -v
```
