# SyncFunc as the extension mechanism for textedit

textedit.Model extends its display pipeline via a single function field (`SyncFunc`) injected at construction time, not via Go interfaces, wrapper overrides, or callback hooks. The SyncFunc receives buffer + cursors + focused + width, returns a fully composed SyncResult (snapshot + coordinate maps), and textedit's View renders cells generically from StyleHint on DisplaySpan.

Why not a pure wrapper (inner.Update then override snapshot): double-sync on every keystroke, scrollToCursor uses wrong (plain) snapshot from inner, must re-scroll after override — fragile and wasteful.

Why not a Go interface: violates the project's concrete-methods preference (§2.2), grows with every new rendering concern (images, links, highlighting), and requires boxing for tea.Model compatibility.

Why not multiple hooks (SyncHook + SpanStyler + ClickHandler): three seams instead of one, SpanStyler becomes a mini-View, every new feature requires a new hook.