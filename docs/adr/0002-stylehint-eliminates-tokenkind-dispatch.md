# StyleHint on DisplaySpan eliminates TokenKind dispatch in View

display.DisplaySpan gains a `StyleHint lipgloss.Style` field. The SyncFunc pre-computes each span's visual style; textedit.View() renders cells generically using StyleHint with zero TokenKind dispatch.

The current `spanToCellsStyled` has ~20 branches dispatching on TokenKind (heading, bold, italic, code fence, link, table, etc.). Moving style computation into the SyncFunc means the View is completely content-agnostic — it knows cells, overlays, and ViewportState. This is the correct scope for a bubbles-level primitive.

Code fence Chroma highlighting moves from View-time to Sync-time: the MarkdownSync splits code spans into per-token sub-spans with per-token StyleHint. Table styling is baked into spans. Link styling is baked in.

The alternative (keeping TokenKind dispatch in View) would mean every textedit consumer that adds a new span kind must modify the shared View — violating the extension boundary.