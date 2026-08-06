# Feature gaps

Recorded 2026-08-05 from a full feature audit of `rr` HEAD. Each entry is a missing or
half-wired user-facing capability; voice dictation, microphone capture, and LLM chat are
**strictly out of scope** and deliberately absent from this list. Entries are ordered by
user impact.

## In-file search

No search inside the open document exists. The recovery store already has an unused
`search_history` table (`crates/rune-db/src/schema.rs`) waiting for it.

Target shape:
- A one-row search bar inside the center pane; one chord toggles it open/closed
  (closing clears highlights).
- Live highlight-as-you-type with an `i/N` match readout; `no matches` / `N matches`
  states in the bar.
- Enter = next match, Shift+Enter = previous; both persist the query to durable history.
- Match navigation with the bar closed via a next/prev chord pair.
- History browsing with ↑/↓, fuzzy-filtered by the current draft; case-insensitive
  matching; wraparound; matches inside concealed markup are skipped as non-navigable.
- Find-and-replace can follow later; the bar and history come first.

## File watching and auto-reload

Disk changes are currently detected only at open, on tab-switch probes, and at save time
(CAS). There is no watcher, so an external edit to the visible document goes unnoticed
until one of those events fires. `rune_db::SyncKind::DiskAhead` is documented as "safe
to adopt", yet no code path adopts it — the footer only shows `⇄ disk changed — [^M]erge`.

Target shape:
- Watch the active document's directory through an injected watcher seam (production
  fsevents/fsnotify implementation, noop in tests and the fuzzer) so no real watcher
  thread exists outside production.
- Directory events trigger an explorer reload plus an async probe of the open document;
  in-place writes trigger a probe. The directory event is the umbrella that catches
  atomic external saves (temp + rename).
- A probe classifying `DiskAhead` on a clean buffer auto-adopts disk content as a
  journaled adoption (undoable, never a silent buffer reset); every other divergence
  keeps today's guard/merge flow.
- Detect external renames via file identity (inode+device) and tell the user
  (`Renamed: a.md → b.md`).

## Create-new-file keybinding

`workspace::new_untitled_document` exists but is reachable only as the automatic
replacement when the last tab closes. No key creates a document at runtime. Target:
one chord creates a durable untitled draft and focuses the title for naming.

## Tab cap, eviction, and pinning

Open tabs are unbounded; digit chords reach only tabs 1–10, so tab 11+ is
keyboard-inaccessible and the list grows without limit. The theme already carries an
unread `tab_pinned` style token.

Target shape:
- Cap open tabs (10 matches the digit chords) with least-recently-active eviction:
  the victim must be non-active, non-pinned, file-bound; prefer clean over dirty; a
  dirty victim gets the standard dirty-close guard; no eligible victim surfaces
  `Tab limit reached — close or unpin a tab`.
- A pin toggle with a visible marker in the tab row (pin key needs choosing — ^P is
  taken by reading view).

## Hardlink-fork warning (wiring only)

The data-safety plumbing is complete — `Stat.nlink` flows through observations and
`LoadResult.nlink` is documented as "saving through this path forks the document from
its other names on disk", with `Mem::set_nlink` built for the test path — but no UI
consumer reads it. Surface `⚠ hardlinked file — saving breaks the link` on load and
on save.

## Image paste

Paste is text-only (strict UTF-8; non-text clipboard bytes are refused). Target:
pasting image bytes writes a content-addressed file (hash-prefixed name) into the
document's `assets/` directory (workspace root for untitled), published atomically via
temp + no-clobber rename, treats an identical existing asset as success, and inserts
the embed link at the caret.

## Animated GIF playback

GIFs render first-frame only. `rune-image::anim` already owns the timing math
(50 ms minimum frame delay). Missing: frame compositing honoring disposal methods and
loop count, a tick source re-armed on scroll, and per-frame retransmission.

## iTerm2 inline-image protocol

Image rendering is Kitty-graphics-only (Kitty/Ghostty + truecolor). iTerm2 and WezTerm
users get the info-card fallback. Target: OSC 1337 inline images as a second protocol
behind the existing capability probe (truecolor not required), with erase-before-place
escape batching.

## Markdown extras

Recognized today: wikilinks, embeds, autolinked bare URLs, setext headings. Missing as
styled constructs (all currently render as plain text or fixed verbatim):
- Callouts: `> [!note]`-style blockquote kinds with distinct styling.
- `==highlight==` inline marks.
- `#tags` as a styled inline token.
- Math: `$$` blocks and inline `$x$` as styled math scopes (no math extension is
  enabled in the parser today).
- Frontmatter display modes (collapsed / source / hidden) — currently one fixed
  pinned-revealed dim treatment.

## Word-count readout

The footer shows `Ln N, Col N` only. Add a live word count beside it.

## Footer link hint

The caret sitting on a link gives no feedback; following it is blind. Show the resolved
target in the footer (`→ target  ⏎ open`) while the caret is on a link.

## Release packaging

A `cargo-dist`-driven pipeline now exists (`dist-workspace.toml`,
`.github/workflows/release.yml`, `RELEASING.md`): a pushed `v*` tag builds an
`aarch64-apple-darwin` binary, publishes a GitHub Release, and pushes the `rune`
Homebrew formula to `aka-rider/homebrew-tap`. What remains open is the signing
story: releases still ship with an ad-hoc signature, which changes on every
rebuild and resets TCC permission grants. Target shape:
- A stable code-signing identity (a real Developer ID certificate) wired into
  the release build, replacing the ad-hoc signature.
- Notarization, once a signing identity exists.

## Port the embed-target fix from branch `img-embedfix`

Live bug on `rr`: `rune_md::catalogue::image_target` resolves an extension-less
wiki-form image embed (`![[x.png]]`-style with the extension elided) as a `Name`
target, appending a bogus `.md`. The unmerged branch `img-embedfix` carries the fix
(force `Target::Path` for image embeds regardless of wikilink form) plus regression
tests for subdirectory, spaced, and extension-less embed targets; its dedupe-ordering
half already landed separately. Port the fix into `catalogue::image_target` and adopt
the tests.
