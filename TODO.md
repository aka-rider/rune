
- [ ] When editor is unfocused, cursor should become invisible.
- [ ] [Explorer visible on untitled launch](.claude/tickets/explorer-visible-on-untitled-launch.md) — Explorer should be collapsed on file launch, visible on pathless launch.
- [ ] Title component should allow to change file extension. So for that extension should be visible as a separate subcomponent or separate part of the title component, and to focus the extension user should explicitly press right arrow.
- [ ] Rust implementation should follow Golang implementation. in separation of rich text editor and verbatim editor. verbatim raw text editor should be used as a tile editor component so that selection, undo (w/o persistence), word movement would also work while editing the title.
- [ ] The file has been changed on disk guard with SaveDiscard escape. sense user in limbo becausecause this card works the same as escape.
- [ ] Explorer pane is missing parent directory, so it's impossible to go up.
- [ ] [Control W doesn't work (close tab) also ^1..9,0 (switch to tab) don't work.](.claude/tickets/close-tab-and-switch-tab-bindings.md)
- [ ] (WP4) `crates/rune-tui/tests/opentabs.rs` is now 571 lines, over the §1.6 500-line budget — the WP4 ^w/^1-^0 global-binding tests were added to the existing WP5 file rather than splitting it out; decompose it (e.g. a separate `opentabs_global.rs` test file) next time it's touched.