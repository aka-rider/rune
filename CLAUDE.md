# Rune — Start Here

`rune` is a ratatui TUI markdown editor in Rust. Prime directive: **protect the user's words** — data safety beats performance, elegance, and features.

**Platform**: macOS (Apple Silicon with ANE). Potentially Linux but out of scope for now. No Windows is supported or planned.

**Designing a feature or touching persistence/UI code? Read `CONSTITUTION.md` first.** Every article is binding.

## Map

```
crates/rune-cli      Entry point; constructs the one Vfs + store and starts the runtime
crates/rune-core     UI-free kernel: buffer, coordinate spaces, cursor set, in-memory undo journal
crates/rune-vfs      The single chokepoint for real-disk I/O (Disk/Mem); Exchange/RenameExcl publish
crates/rune-db       Multiprocess-safe SQLite recovery store: journal, snapshots, observations, blobs, materialize
crates/rune-syntax   Producer-agnostic syntax layer: reveal vocabulary, SyntaxSpan model, scopes, wrap pass
crates/rune-md       Markdown pipeline over comrak: parse -> emit -> wrap -> snapshot. Terminal-free
crates/rune-tui      Elm-style runtime, terminal lifecycle, keymap resolver, panes, editor UI
crates/rune-fuzz     Headless session fuzzer: drives the real update loop, checks named invariants
crates/rune-ts       Terminal-free tree-sitter layer: 22 grammars, compile-free language lookup, whole-document highlight
crates/rune-merge    Editor<->disk hunk resolver: three-way diff, conflict markers, merge-mode dispatch intercept
crates/rune-nav      Link/target resolution: bare-then-.md retry, one classifier shared by links and images
crates/rune-image    Terminal-free image decode + Kitty graphics transmit: byte-parity framing, deterministic IDs
```

## Vocabulary

Say the left-hand term; the aliases in parentheses are ambiguous.

- **materialize** — the write turning a buffer into the destination `.md`; ⌘S, evict, quit, rename all funnel through it (not "save", "flush" — autosave targets the recovery store).
- **journal / snapshot** — durable per-document edit stream / content-addressed full-content version (not "undo stack", "backup").
- **observation / probe** — a recorded disk fact (hash, size, mtime, inode) / the async re-read that classifies sync state (not "stat cache", "poll").
- **draft** — untitled doc, recovery-backed, no file until named.
- **pane** — a focusable region of the workspace (Editor, Explorer, Tabs); focus routing keys off it.
- **snapshot (display)** — the `SyntaxSnapshot` a buffer parses to; distinct from a *document* snapshot in the recovery store. Say which one you mean.
- **highlight overlay** — a `(Range<usize>, ScopeId)` list from `rune-ts` painted onto cell styles at render time, never emitted as a `SyntaxSpan`; distinct from *snapshot (display)* (the emitted syntax model) and from a *document* snapshot (the recovery store).
- **help document** — virtual read-only tab generated from the keymap; never dirty.

## Build & Test

`make build` · `make test` · `make lint` · `make fmt` · `make bench` · `make perf-guard` · `make test-fuzz` (session fuzzer; `RC=` cases, `RS=` pinned seed) · `make test-grammars` (22 tree-sitter grammars)

## House Rules

- **User-centric**: every user action must have feedback; every interaction must be pleasant. Design the architecture so that silent input swallowing is architecturally unsound. Pay attention to application performance.
- **GUI-first**: take a step to design the UI, validate the solution from a UX standpoint — are there better alternatives?
- **Who does it better**: in doubt? `/research` the best-in-class solutions from Zed, Helix, Neovim, Visual Studio Code, Emacs, etc.
- **Never cite a `path:line` or even `path` location in a code comment.** `foo/bar.rs:210` rots the moment either file moves. A bare filename or module path (`cluster.rs`, `driver::checks`) is tolerated but discouraged — replace it with a description of the invariant when you touch the comment. Say what the invariant is or why the code is shaped this way, and let the reader grep. The same goes for doc comments.
- **Code never cites `CONSTITUTION.md`.** State the invariant in the comment itself, in its own words — a `§N`-style reference rots the moment an article is renumbered or split, and hides the actual rule behind a lookup.
- Keep a source file **under 500** lines. When you push one over, record it in `TODO.md` with the reason and a named split candidate.

## The Unbreakables (digest — full articles in CONSTITUTION.md)

- Write the user's bytes verbatim — no normalized line endings / trailing newline / BOM / encoding. §6
- Write user content only through a durable temp write + atomic `exchange`/`rename_excl` publish; unsaved work goes to the recovery store, never the user's file. §2, §4
- Refuse, don't guess, at the buffer boundary; clamp only at the caller's boundary. An empty async reset is never a user deletion. §6, §7
- Edit/cursor offsets are BYTES; display widths are TERMINAL CELLS over whole grapheme clusters (`unicode-width`), never bytes and never `char`s. §6
- Halt with a surfaced error, never `panic`/`unwrap`/`expect` — a panic loses the unsaved buffer. The workspace denies those lints; do not `allow` them in production code. §9
- Reach the filesystem only through the injected `Vfs`. §1
- Capture displaced bytes as a durable blob before they're ever discarded. §3
- A crash in linked tree-sitter C is not a Rust panic and no lint can see it — never construct an `InputEdit`, every parse is a full parse. §9
- `update` is the sole writer of synchronous state; a `Cmd` exists only for work that leaves the thread. §10
- A superseded async reply is killed by a generation/version echo, never by resolving live state on arrival. §10
