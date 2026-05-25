# WP14C — Markdown Advanced Preview

## Scope

Add advanced live-preview elements after WP14A and WP14B are stable:

- inline math and math blocks (styled raw fallback)
- frontmatter
- callouts
- embed references
- highlights (`==text==`)
- image tokens as semantic placeholders

## Dependencies

- WP14A
- WP14B
- WP17 only for terminal image rendering, not for image token detection

## Deliverables

### Advanced Tokens

- Inline math: per-token reveal, rendered fallback is semantic/styled raw text.
- Math blocks: per-block reveal, rendered fallback is semantic/styled raw text.
- Frontmatter: block reveal; default rendered mode is collapsed semantic property indicator unless configuration says source.
- Callouts: block reveal and semantic callout kind/title spans.
- Embed references: line-level reveal.
- Highlights: per-token reveal, delimiters hidden when cursor away.
- Images: `TokenImage` spans with alt/path metadata and text fallback. Actual terminal drawing belongs to WP17.

### Configuration

- Define frontmatter render mode with a safe default.
- Keep configuration on the editor/UI side when it affects presentation.

## Non-Goals

- Actual LaTeX rendering.
- Terminal graphics protocol output.
- Clipboard image copy/paste behavior.
- Network or filesystem access during display sync.

## Tests

- Render/reveal tests for every advanced element.
- Image token metadata extraction without reading files.
- Frontmatter config tests.
- Coordinate delta tests for highlight and inline math delimiters.
- Block reveal tests for frontmatter, callout, math block.

## QA Gates

| # | Gate | Harm Prevented |
|---|---|---|
| 1 | Advanced elements never perform I/O during `SyntaxMap.Sync` | Display pipeline blocks or fails from filesystem state |
| 2 | Image tokens render as semantic placeholders without graphics support | Unsupported terminals still show stable content |
| 3 | Frontmatter default is deterministic and configurable | Worker picks an arbitrary mode and tests become brittle |
| 4 | Highlight/math delimiter deltas preserve round-trips | Hidden delimiter math corrupts cursor positions |

## Verification

```bash
go test ./pkg/editor/display/ -run 'TestMarkdownAdvanced|TestImageToken|TestFrontmatter' -v
```
