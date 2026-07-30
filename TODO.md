
- [ ] When editor is unfocused, cursor should become invisible.
- [ ] [Explorer visible on untitled launch](.claude/tickets/explorer-visible-on-untitled-launch.md) — Explorer should be collapsed on file launch, visible on pathless launch.
- [ ] Title component should allow to change file extension. So for that extension should be visible as a separate subcomponent or separate part of the title component, and to focus the extension user should explicitly press right arrow.
- [ ] Rust implementation should follow Golang implementation. in separation of rich text editor and verbatim editor. verbatim raw text editor should be used as a tile editor component so that selection, undo (w/o persistence), word movement would also work while editing the title.
- [ ] The file has been changed on disk guard with SaveDiscard escape. sense user in limbo becausecause this card works the same as escape.
- [ ] [Control W doesn't work (close tab) also ^1..9,0 (switch to tab) don't work.](.claude/tickets/close-tab-and-switch-tab-bindings.md)
- [ ] `crates/rune-tui/src/explorer.rs` is over the 500-line budget (§1.6) — it was already at 613 lines before WP3's `..` parent-row change, which pushed it to 650; the WP explicitly forbade splitting the file as part of that change, so the split is deferred here.