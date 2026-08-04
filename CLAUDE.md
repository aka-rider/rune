# Rune — Start Here

`rune` is a ratatui TUI markdown editor in Rust. Prime directive: **protect the user's words** — data safety beats performance, elegance, and features.

**Platform**: privary: macOS (Apple Silicon with ANE); potentially Linux but out of scope for now.
No Windows is supported or planned.

**Designing a feature or touching persistence/UI code? Read `CONSTITUTION.md` first.** Every article is binding; code cites articles by frozen § number (e.g. §1.4.8, §5.4).

The **Go reference implementation** lives in `golang/` with its own `CLAUDE.md` and its own Makefile. It is where behaviour is looked up when the Rust port has to match something; it is not where new features go.

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

`make build` · `make test` · `make lint` · `make fmt` · `make bench` · `make perf-guard` · `make test-fuzz` (session fuzzer; `RC=` cases, `RS=` pinned seed)

Cross-implementation screen parity: `make parity` (builds both sides, captures the same scenario, diffs). Go alone: `cd golang && make test`.

## House Rules

- **User-centric**: EVERY user's action MUST have a feedback; Every interaction must be pleasant; 
Design the architecture so that silent input swallowing is architecturally unsound;
Pay attention to the application performance;
- **GUI-first**: Take a step to **design UI** validate your solution from **UX** standpoint: are there better alternatives;
- **Who does it better**: in doubt? /research the best-in-class solutions from: Zed, Helix, Neovim, Visual Studio Code, Emacs, etc.
- **Never cite a `path:line` or even `path` location in a code comment.** `foo/bar.rs:210` rots the moment either file moves. A bare filename or module path (`cluster.rs`, `driver::checks`) is tolerated but discouraged — replace it with a description of the invariant when you touch the comment. Say what the invariant is or why the code is shaped this way, and let the reader grep. The same goes for doc comments. (Frozen `CONSTITUTION.md` § numbers are the deliberate exception — they are guaranteed stable.)
- Keep a source file **under 500** lines (§1.6). When you push one over, record it in `TODO.md` with the reason.
- **User-centric**: EVERY user's action MUST have a feedback; Every interaction must be pleasant; 
Design the architecture so that silent input swallowing is architecturally unsound;
Pay attention to the application performance;
**GUI-first**: Take a step to **design UI** validate your solution from **UX** standpoint: are there better alternatives;
**Who does it better**

## The Unbreakables (digest — full articles in CONSTITUTION.md)

- Write the user's bytes verbatim — no normalized line endings / trailing newline / BOM / encoding. §1.4.5
- Write user content only through a durable temp write + atomic `exchange`/`rename_excl` publish; unsaved work goes to the recovery store, never the user's file. §1.4.1, §1.4.2
- Clamp every edit range to the live byte length; an empty async reset is not a deletion. §1.3
- Edit/cursor offsets are BYTES; display widths are in terminal cells (`unicode-width`), not bytes and not `char`s. §1.5
- Halt with a surfaced error, never `panic`/`unwrap`/`expect` — a panic loses the unsaved buffer. The workspace denies those lints; do not `allow` them in production code. §1.3
- Reach the filesystem only through the injected `Vfs`. §1.4.9
- Capture displaced bytes as a durable blob before they're ever discarded. §1.4.10
