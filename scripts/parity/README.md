# Visual-parity harness (Go vs. Rust `rune`)

Runs the Go and Rust `rune` binaries on the same fixture file at the same
pinned terminal size, captures both screens through a private tmux server,
and mechanically diffs/asserts chrome invariants between them. It also
serves each pinned session over `ttyd` so a browser can screenshot it.

Scope is chrome geometry only (see the plan's "Out of scope" section):
markdown styling, the left-column shape, and breadcrumb path-relativization
are all expected to differ and are not asserted here.

## Commands

```sh
make parity-capture   # builds both binaries, launches + drives both sessions, captures screens
make parity-diff      # unified diff of the two captures -> .scratch/parity/out/diff.txt (report, always exits 0)
make parity-assert    # chrome invariant gate (exits non-zero on failure)
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
3. Both sides start from `scripts/parity/fixtures/sample.md` copied into a
   scratch workspace. To use a different fixture, add it under `fixtures/`
   and adjust `capture.sh` (currently hardcoded to `sample.md`).
4. Run `PARITY_SCENARIO=<name> make parity-capture`.

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
  root `TODO.md`.
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
