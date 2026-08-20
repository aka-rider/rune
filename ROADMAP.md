# Roadmap

The shipped baseline is README's feature list; this is the delta on top of it,
forward-looking and prioritized by user impact. Full detail on each item lives
in `TODO/feature-gaps.md`.

- [x] **In-file search** — a toggleable search bar in the editor pane, live
  highlight-as-you-type with a match readout, next/prev navigation, and
  durable search history.
- [ ] **File watching + auto-adoption** — watch the open document's directory
  and auto-adopt a clean buffer's external changes; keep today's guard/merge
  flow for every divergent case.
- [x] **Trash** — delete a file from the explorer or editor via the macOS
  Trash, guarded by a confirm prompt and refused while the document is dirty.
- [x] **New-file chord** — a key that creates a durable untitled draft and
  focuses the title for naming, without waiting for the last tab to close.
- [x] **Tab cap, eviction, and pinning** — bound open tabs to the ten
  digit-addressable slots, evict least-recently-active non-pinned tabs, and
  let the user pin a tab against eviction.
- [x] **Hardlink-fork warning** — surface a warning when saving a hardlinked
  file would fork it from its other names on disk; the underlying plumbing
  already tracks link count.
- [ ] **Image paste** — write pasted image bytes to a content-addressed file
  in the document's assets directory and insert the embed link at the caret.
- [ ] **Animated GIF playback** — composite and retransmit GIF frames instead
  of rendering a still first frame.
- [ ] **iTerm2 inline-image protocol** — a second image protocol behind the
  existing capability probe, for terminals without Kitty graphics support.
- [ ] **Markdown extras** — callouts, `==highlight==` marks, `#tags`, math
  (`$$`/`$...$`), and selectable frontmatter display modes.
- [ ] **Word-count readout** — a live word count beside the footer's
  line/column display.
- [ ] **Footer link hint** — show the resolved target in the footer when the
  caret sits on a link.
- [ ] **Release signing** — a stable code-signing identity in place of the
  ad-hoc signature (which resets TCC grants on every rebuild), and eventual
  notarization. The build/archive/Homebrew-formula pipeline itself now ships
  via `cargo-dist` (see `RELEASING.md`).
