# Visual-parity harness (Go vs. Rust `rune`)

Runs the Go and Rust `rune` binaries on the same fixture file at the same
pinned terminal size, captures both screens through a private tmux server,
and mechanically diffs/asserts chrome invariants between them. It also
serves each pinned session over `ttyd` so a browser can screenshot it.

`parity-assert`'s scope is chrome geometry only (see the plan's "Out of
scope" section): markdown styling, the left-column shape, and breadcrumb
path-relativization are all expected to differ and are not asserted there.
`parity-grid` (plan WP1) asserts markdown *content* instead — the editor
viewport's own glyph grid, over a corpus of markdown fixtures — see
"Glyph-grid parity" below.

## Commands

```sh
make parity-capture   # builds both binaries, launches + drives both sessions, captures screens
make parity-diff      # unified diff of the two captures -> .scratch/parity/out/diff.txt (report, always exits 0)
make parity-assert    # chrome invariant gate (exits non-zero on failure)
make parity-grid      # glyph-grid content gate over the markdown fixture corpus (exits non-zero on failure)
make parity           # capture + diff + assert
make parity-serve     # serves both pinned sessions over ttyd (7681 go, 7682 rust)
make parity-clean     # kills ttyd + the private tmux server, removes the run directory
```

Or drive the scripts directly:

```sh
scripts/parity/capture.sh go   01-open-file
scripts/parity/capture.sh rust 01-open-file
scripts/parity/diff.sh
scripts/parity/assert.sh
scripts/parity/serve.sh go
scripts/parity/serve.sh rust
scripts/parity/clean.sh
```

## Env-var knobs

| Var | Default | Meaning |
|---|---|---|
| `PARITY_COLS` | `120` | Pinned terminal width |
| `PARITY_ROWS` | `34` | Pinned terminal height |
| `PARITY_SCENARIO` | `01-open-file` | Which `scenarios/<name>/` to drive |
| `PARITY_RUN` | `<repo>/.scratch/parity/run` | Scratch workspaces + tmux/ttyd state |
| `PARITY_OUT` | `<repo>/.scratch/parity/out` | Captured screens + diff/assert output |
| `PARITY_PORT_GO` | `7681` | ttyd port for the Go side |
| `PARITY_PORT_RUST` | `7682` | ttyd port for the Rust side |
| `PARITY_KEEP` | `1` | Set to `0` to kill the tmux session at the end of `capture.sh` |

All tmux state lives on a private server (`tmux -L rune-parity -f /dev/null`)
— it never touches your own tmux server or `~/.tmux.conf`.

## Adding a scenario

1. Create `scripts/parity/scenarios/<name>/go.keys` and `.../rust.keys`.
2. Each file is a newline-separated list of `tmux send-keys` key names (one
   per line; blank lines and `#`-prefixed lines are skipped). Use `tmux`'s
   own key-name syntax, e.g. `C-x`, `Down`, `Enter`.
3. If a `<side>.keys` sends any key, `capture.sh` requires a matching
   `<side>.settle` — a one-line `grep -q --` pattern (BRE, not necessarily a
   regex-escaped literal) that appears ONLY after those keys have taken
   effect, so the capture cannot race the repaint. Its text may contain the
   literal token `{{FIXTURE}}`, which `capture.sh` expands to the actual
   fixture filename (the 3rd `capture.sh` argument, `sample.md` by default)
   before waiting. `wait_for_pane`'s pattern is a BRE, so ` *` means "zero or
   more spaces" — mirroring `assert.sh`'s own `sample\.md +──╯` — a fixed
   single-space literal must not be used if a settle file ever anchors on
   that row.
   - `rust.settle` itself waits for the literal `Enter open` — a fragment of
     the Explorer-focused footer, present only once `C-b` has switched focus
     and rendered, absent from the initial single-pane view. It deliberately
     does **not** wait for the bottom-border breadcrumb (`sample.md ──╯`,
     what `assert.sh` itself checks): with the Explorer pane open, Rust's
     un-relativized absolute-path breadcrumb (see "Known divergences" below)
     overflows the narrower centre-pane width for any workspace path this
     harness's own nesting produces (`<repo>/.scratch/parity/run/rust/
     parityws/<fixture>`) and renders **nothing at all** on that row — not a
     slow repaint, a permanent blank border. No settle predicate can wait for
     text that structurally never appears; see this file's own "Known
     divergences" section (breadcrumb relativization) and root `TODO.md` for
     the resulting `assert.sh` gate impact.
4. Both sides start from `scripts/parity/fixtures/sample.md` by default,
   copied into a scratch workspace. `capture.sh` takes the fixture as an
   optional 3rd argument (`capture.sh <go|rust> [scenario] [fixture]`) —
   add a new file under `fixtures/` and pass its name to use it instead
   (`grid.sh`, below, does exactly this for each corpus fixture).
5. Run `PARITY_SCENARIO=<name> make parity-capture`.

## Glyph-grid parity (plan WP1)

`grid.sh` (wired as `make parity-grid`) captures both sides — via the same
`capture.sh`/`01-open-file` path `parity-assert` uses, so both sides reach
the same two-pane chrome geometry — over each markdown fixture under
`fixtures/` (`headings.md`, `emphasis.md`, `lists.md`, `tasks.md`,
`fences.md`, `quotes.md`, `tables.md`, `tables-divergent.md`,
`tables-narrow.md`, `frontmatter.md`, `cjk.md`, `emoji.md`; `sample.md`
stays `parity-assert`'s own fixture). For each one
it crops BOTH captures down to the center pane's own content rows — not
the left Explorer/Open pane, not the title/breadcrumb/footer chrome, which
are already covered (or explicitly out of scope) via `parity-assert` — and
diffs that cropped grid. A fixture whose cropped grid doesn't yet match is
excluded from the gate in `grid.sh` (see `excluded_reason`), with the
reason duplicated below.

Column/row cropping and a small trailing-tilde normalization (Go marks an
empty line past the document's own content with a vi-style `~`; Rust
leaves it blank) live in `grid_diff.py` — see its module docstring for why
character-index cropping is safe even over the CJK/emoji fixtures'
wide glyphs.

## Screenshots (agent procedure)

The mechanical gate (`parity-assert`) is what actually blocks a change; the
screenshot is for a human/agent to *look* at the two chrome layouts
side-by-side. Procedure, run from the agent side:

1. `make parity-capture` then `make parity-serve` (or `scripts/parity/serve.sh go` / `rust` individually).
2. Load the Playwright MCP tool schemas (they're deferred until requested):
   `ToolSearch` with query
   `select:mcp__MCP_DOCKER__browser_navigate,mcp__MCP_DOCKER__browser_take_screenshot,mcp__MCP_DOCKER__browser_resize,mcp__MCP_DOCKER__browser_close`.
3. `browser_resize` to a fixed size (use the SAME size for both sides, e.g. 1280x800).
4. `browser_navigate` to `http://host.docker.internal:7681/` (Go), then `browser_take_screenshot`.
5. `browser_navigate` to `http://host.docker.internal:7682/` (Rust), then `browser_take_screenshot`.
6. `browser_close`.

**`localhost:<port>` is refused from inside the Playwright container** — the
browser runs in Docker and can only reach the host via
`http://host.docker.internal:<port>/`. `ttyd` itself still also listens on
`127.0.0.1:<port>` for a human driving a real browser on the host directly.

The screenshot itself is not written anywhere durable (Assumption A1 in the
plan) — it's viewed by the agent in the moment, not archived as a file.

## Known divergences

These are expected and not asserted by `parity-assert` — they are either
explicitly out of scope for the chrome-parity fix, or an inherent property
of how each side renders:

- **Caret rendering.** The Rust caret is a drawn, reverse-video cell (the
  real terminal cursor is hidden for the whole session, `term.rs`); Go's
  caret may be the real terminal cursor, which `tmux capture-pane` does not
  record at all. Expect a caret-cell diff between the two captures purely
  from this — it is not a bug.
- **Left-column shape.** Go's left column is one bordered box with a
  `── Open ──────`-style text divider sized by tab count. Rust's left
  column is two separately bordered blocks (`Files` / `Open`), 50/50 split.
  This stays as-is (user decision) — out of scope for the chrome-parity fix.
- **Markdown styling, images, tables, links.** Not addressed by this
  harness or the chrome-parity fix at all.
- **Breadcrumb path relativization.** Go relativizes the breadcrumb against
  the workspace root; Rust has no workspace-root concept on `App` yet
  (`explorer.root` is empty until the first `^x`), so it renders every
  `Normal` component of the absolute path instead. Tracked in the repo's
  root `TODO.md`. **Consequence proven empirically (glyph-grid parity plan
  WP0):** with the Explorer pane open (this scenario's own `C-b`) and the
  parity harness's own workspace path (`<repo>/.scratch/parity/run/rust/
  parityws/<fixture>`, 10 `Normal` components deep), Rust's un-relativized
  crumb doesn't just render longer than Go's — it exceeds `overlay`'s own
  `bc + 7 > block.width` fit check even at its most truncated (one leaf part
  plus the ellipsis) and renders **no crumb text at all**, a bare border, on
  every capture, not intermittently. `assert.sh`'s "rust bottom content row
  ends 'sample.md ──╯'" check therefore fails deterministically — see root
  `TODO.md`. This is orthogonal to `parity-grid`'s own gate, which crops to
  rows 2..(ROWS-2) and never looks at the bottom border row at all.
- **Title text.** Observed via the WP5 screenshot comparison: Go's title
  row shows the file name WITHOUT its extension (`sample`, distinctly
  colored); Rust's shows the full file name (`sample.md`, plain). Title
  *content* was never in this plan's scope (only the border + breadcrumb-
  splice geometry) — recorded here as an observed divergence, not a gate.
- **Left-pane listing.** Go's file list includes a `../` parent-navigation
  entry above `.rune/`/`sample.md`; Rust's does not. Consistent with the
  left-column shape being out of scope (see above) — the two sides'
  Explorer/file-list implementations are independent, not just re-skinned.
- **Footer content.** Go's footer advertises a chat pane and a dictation
  mic icon (`^r chat`, `🎤`); Rust's advertises tabs, save, and quit chords
  (`^t tabs`, `⌘s save`, `^c`/`^⌥d` quit) instead. Expected — chat/dictation
  and mouse support are both out of scope for the Rust port at this stage
  (plan Out of scope), and the two keymaps were never meant to match.
- **Heading concealment inside a heading's own inline spans
  (`headings.md`, excluded from `parity-grid`).** Go's inline-emphasis
  concealment does not recurse into a heading line's own text: `## A
  heading with **bold** and \`code\`` stays raw in Go, while Rust conceals
  the `**`/`` ` `` inside the heading like anywhere else. Elsewhere (plain
  cursor-off-line headings with no nested inline spans) the two sides
  agree.
- **Plain list markers are never concealed in Go (`lists.md`, `tasks.md`,
  `frontmatter.md`, `cjk.md`, `emoji.md`, excluded from `parity-grid`).**
  Go's markdown walker only emits a concealable span for GFM task-list
  checkboxes (`- [ ]`/`- [x]` → `☐`/`☑`); plain bullet (`-`) and ordered
  (`1.`) markers fall through with no span at all and are never concealed,
  for any cursor position. Rust conceals all of them. Vestigial on the Go
  side, not a cursor/focus condition — confirmed by reading Go's
  `walkTaskList` (`pkg/editor/display/markdown_walk.go`).
- **Fenced code block info string stays visible in Go (`fences.md`,
  excluded from `parity-grid`).** Go strips a fence's backticks but leaves
  the language tag (e.g. `rust`, `text`) as a plain visible line; Rust
  conceals the entire fence delimiter line, info string included.
- **Nested blockquote/inline concealment gaps in Go (`quotes.md`,
  excluded from `parity-grid`).** Go's blockquote-marker concealment
  covers only the outermost `>` — a nested (depth ≥ 2) quote's own `>`
  stays raw — and, like headings, does not recurse into inline emphasis
  nested inside quoted text. Rust conceals both fully.
- **Table rendering (`tables-divergent.md`, excluded from `parity-grid`;
  `tables.md`/`tables-narrow.md` ARE gated and pass).** Both sides render
  an actual box-drawn table widget; the constructs collected in
  `tables-divergent.md` each have a verified Go-side defect instead of a
  Rust one:
  - **Inverted alignment.** goldmark's alignment enum is `AlignLeft = iota
    + 1` (Left=1, Right=2, Center=3, None=4), and Go casts that value into
    a renderer switch reading `0=left/1=center/2=right` — so `:---`
    renders centred and `:---:` renders left in Go. Rust reads the
    alignment correctly.
  - **Escaped-pipe and ragged-row cell splitting.** Go splits a table row
    on `strings.Split(line, "|")`, with no concept of `\|` escaping and no
    tolerance for a row with more cells than the header — an escaped pipe
    misaligns the row's columns, and an extra cell is kept instead of
    truncated to the header's column count. comrak gives Rust real cell
    sourcepos, so both come out correct.
  - **Pipe inside inline code.** GFM requires `\|` even inside a code
    span, so comrak counts an unescaped pipe there as a column separator
    — in a header row that disagrees with the delimiter row's column
    count and the whole construct degrades to a plain paragraph; in a
    body row the row instead truncates to the established column count,
    dropping the last cell. This is GFM-spec-conformant: Rust follows the
    spec, Go does something else.
  - **Container-prefix leakage.** A table inside a blockquote or list item
    corrupts Go's own line-splitting, which doesn't account for the `> `
    or list-indent prefix before looking for `|`.
  - **Per-rune emoji width.** Go measures a cell's width per rune, not per
    grapheme cluster, so a multi-codepoint ZWJ sequence is counted as
    several cells wide instead of one.
  - **CJK TAB-padding.** The same vendored-renderer defect that already
    excludes `cjk.md` from this gate.
- **Go pads a long CJK line's remainder with literal tab bytes (`cjk.md`,
  excluded from `parity-grid`).** Reproducible: a paragraph whose only
  wide (CJK, width-2) run is followed by trailing blank width in a single
  visual row renders with real `\t` (0x09) characters filling that
  trailing width in Go's own `tmux capture-pane -p` output, instead of
  spaces. Root cause identified (2026-07-28, see `TODO.md`): a hard-tab
  cursor-movement optimization in the vendored `github.com/aka-rider/
  ultraviolet` fork bubbletea's renderer uses (`replace` in `go.mod`),
  triggered by a CJK-row-specific width-accounting mismatch — a
  third-party dependency's behavior, not a bug in this repo's own `pkg/`/
  `cmd/` code, so left unfixed rather than forcing a speculative patch
  into either the dependency or the renderer.
