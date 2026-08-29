# Rune — Start Here

`rune` is a ratatui TUI markdown editor in Rust. Prime directive: **protect the user's words** — data safety beats performance, elegance, and features.

**Platform**: macOS (Apple Silicon with ANE). Potentially Linux but out of scope for now. No Windows is supported or planned.

**Read `CONSTITUTION.md` before contributing anything.** Every rule in it is binding.

## Map

```
crates/rune-cli      Entry point; constructs the one Vfs + store and starts the runtime
crates/rune-core     UI-free kernel: buffer, coordinate spaces, cursor set, in-memory undo journal
crates/rune-vfs      The single chokepoint for real-disk I/O on user documents (Disk/Mem); Exchange/RenameExcl publish; rune-db sits below it and manages its own sidecar file directly
crates/rune-db       Multiprocess-safe SQLite recovery store: journal, snapshots, observations, blobs, materialize
crates/rune-syntax   Producer-agnostic syntax layer: reveal vocabulary, SyntaxSpan model, scopes, wrap pass
crates/rune-md       Markdown pipeline over comrak: parse -> emit -> wrap -> snapshot. Terminal-free
crates/rune-tui      Elm-style runtime, terminal lifecycle, keymap resolver, panes, editor UI
crates/rune-fuzz     Headless session fuzzer: drives the real update loop, checks named invariants
crates/rune-ts       Terminal-free tree-sitter layer: 32 grammars, compile-free language lookup, whole-document highlight
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

`make build` · `make test` · `make lint` · `make fmt` · `make bench` · `make perf-guard` · `make test-fuzz` (session fuzzer; `RC=` cases, `RS=` pinned seed) · `make test-grammars` (32 tree-sitter grammars)

## House Rules

- **User-centric**: every user action must have feedback; every interaction must be pleasant. Design the architecture so that silent input swallowing is architecturally unsound. Pay attention to application performance.
- **GUI-first**: take a step to design the UI, validate the solution from a UX standpoint — are there better alternatives?
- **Who does it better**: in doubt? `/research` the best-in-class solutions from Zed, Helix, Neovim, Visual Studio Code, Emacs, etc.
- **Comment discipline is governed by `CONSTITUTION.md`** (comments article) — that includes never citing a `path:line` or a `§`-style reference; code never cites this file or that one.
- Keep a source file **under 500** lines. When you push one over, record it in `TODO.md` with the reason and a named split candidate.
