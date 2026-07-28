# Rune — Start Here

`rune` is a Bubble Tea (v2) TUI markdown editor in Go. Prime directive: **protect the user's words** — data safety beats performance, elegance, and features.

**Platform: macOS (Apple Silicon) only.** No other OS is supported or planned. Never write cross-platform code, portability stubs, or `!darwin` / `linux` / `windows` / `!unix` build tags — a non-Darwin build failing to compile is intended, not a bug to fix. Darwin-only syscalls (`golang.org/x/sys/unix`), cgo frameworks, and `/usr/bin` shell-outs are expected. Existing `darwin` / `unix` / `cgo` / `fuzzing` tags on real impls are fine; `vfs.ErrUnsupported` is a per-*filesystem* capability gap (e.g. an SMB/NFS mount lacking `renamex_np`), not an OS-portability seam.

**Designing a feature or touching persistence/UI code? Read `CONSTITUTION.md` first.** Every article is binding; code cites articles by frozen § number (e.g. §1.4.8, §5.4).

## Map

```
cmd/rune                        Entry point; bootstraps ONE vfs.FS + the workspace
pkg/ui                          Bubble Tea app: router (app.go), pages/, components/, help/
pkg/ui/pages/workspace          Main 3-pane page; owns file I/O, journal, undo/redo, layout
pkg/ui/components/textedit      Base editing component (buffer, cursors, viewport, cell render)
pkg/ui/components/markdownedit  textedit + markdown cell builder (parse, reveal, highlight, images, links)
pkg/ui/components/…             filetree, opentabs, footer, title, search, chat, dictation, …
pkg/ui/keymap, pkg/ui/styles    Leaf packages: all keybindings / shared styles
pkg/editor                      Domain primitives: buffer, cursor, coords, display, keybind
pkg/docstate                    SQLite recovery store: journal, snapshots, observations, blobs
pkg/vfs                         Injected filesystem (Disk/Mem); file identity, atomic publish (Exchange/RenameExcl), Trash
pkg/merge                       3-way merge for external-change resolution
pkg/dictation, whisper, microphone   Voice input
pkg/ai, imagekit, terminal, inputlang, command   Support packages
internal/fuzz                   Session fuzzer + property driver (make test-fuzz)
```

## Vocabulary

Say the left-hand term; the aliases in parentheses are ambiguous.

- **textedit** — the base editing component; no undo of its own (not "editor", "textarea").
- **markdownedit** — textedit + a markdown cell builder (`spanToCellsStyled`) + image/link integration (not "rich editor", "markdown editor", "MarkdownSync" — that name never existed in code).
- **SyncFunc** — the buffer→syntax/display-snapshot seam: pure `func(buf, sm, cursors, focused, width)`. `PlainSync` is the only implementation and the default for every textedit, markdownedit included — it already parses and conceals markdown (that lives in `display.SyntaxMap.Sync`, shared by all textedits, §12), it just doesn't style it.
- **CellBuilderFunc / ImageRowFunc** — the render-time seam markdownedit actually uses for markdown styling and image rows (`textedit.Model.RenderView`); `markdownedit.spanToCellsStyled` is what markdown syntax highlighting is (§12).
- **workspace** — the main page; owns file I/O, docstate persistence, and undo/redo routing.
- **journal / snapshot** — durable per-document edit stream / content-addressed full-content version (not "undo stack", "backup").
- **observation / probe** — a recorded disk fact (hash, size, mtime, inode) / the async re-read that classifies sync state `Clean | BufferAhead | DiskAhead | Diverged` (not "stat cache", "poll").
- **materialize** — the CAS write turning a buffer into the destination `.md`; ⌘S, evict, quit, rename all funnel through it (not "save", "flush" — autosave targets the recovery store).
- **draft** — untitled doc, recovery-backed, no file until named.
- **scrubber** — the time-travel undo UI; ⌘Z is the comfortable tier.
- **help document** — virtual read-only tab generated from the keymap; never dirty.

## Build & Test

`make build` · `make run` · `make test` · `make test-fuzz` (session fuzzer) · `make release-snapshot`

## The Unbreakables (digest — full articles in CONSTITUTION.md)

- Write the user's bytes verbatim — no normalized line endings / trailing newline / BOM / encoding. §1.4.5
- Write user content only through a durable temp write + atomic `vfs.Exchange`/`RenameExcl` publish; unsaved work goes to the recovery store, never the user's file. §1.4.1, §1.4.2
- Clamp every edit range to the live byte length; an empty async reset is not a deletion. §1.3
- Edit/cursor offsets are BYTES (`len`); display widths are RUNES (`utf8.RuneCountInString`). §1.5
- Halt with a surfaced error, never `panic` — a panic loses the unsaved buffer. §1.3
- Reach the filesystem only through the injected `vfs.FS`. §1.4.9
- Capture displaced bytes as a durable blob before they're ever discarded. §1.4.10
