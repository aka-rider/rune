# Plan — rich WYSIWYG editor MVP (Rust port)

## Goal

The Rust port at `rust/` gains tree-sitter syntax for ten non-markdown languages, a Catppuccin theme with a 256-colour fallback, a sequence-capable keymap engine with when-clauses, search & replace with an inline live preview, vim/Helix scroll patterns, mouse input, and structural navigation — verified by a glyph-grid parity gate against the Go implementation. Affects anyone editing in `rune`; the editor surface only.

Explicitly **not** in this plan: the editable title and rename guard (already implemented on worktree branch `worktree-kind-inventing-marshmallow`), links/wiki-links, tables, images, horizontal scroll, a line-number gutter, animated scrolling, user config files, full vim modal mode, 3-way merge, file watching, the time-travel scrubber, filetype icons.

## Context

**Inbound plan Rejected** because three load-bearing claims failed verification: it claimed no Go/Rust parity harness exists (`scripts/parity/` exists, wired into `Makefile:70-87`), it planned an editable title and rename guard (both already implemented on a sibling worktree), and it assumed a monolithic `keymap.rs` (already split into `binding.rs`/`global.rs`/`keystate.rs`). Verified this session: the `Vfs` trait surface, `rune-db`'s exports and clock threading, every `SyntaxSpan`/`SyntaxLine`/`RevealState`/`StyleId` consumer, `styles.rs`'s function inventory, `help.rs`'s hand-written section, `banner.rs`'s modal priority, `gc.rs`'s sweep rule, the fuzz `Action` enum, and `scripts/parity`.

**Repository state (branch `rr`, HEAD `74dc1ad`).** Seven crates: `rune-cli`, `rune-core`, `rune-db`, `rune-fuzz`, `rune-md`, `rune-tui`, `rune-vfs` (`rust/Cargo.toml`). `edition = "2024"`. Workspace lints deny `unwrap_used`, `expect_used`, `panic`.

**`rr` advanced during planning — re-verify before trusting any citation here.** This plan was drafted against `016f981`; `rr` is now `74dc1ad`, three commits later: `64fdad7` (held-space leader chords, `^D^D` quit, contextual footer), `84fd6b0` (TODO.md §1.6 bookkeeping, SSH-probe blocker resolved), then a fast-forward merge of `worktree-kind-inventing-marshmallow`. Consequences:

- The **title and rename work is now on `rr`**, not on a sibling branch: `rune-tui/src/rename.rs` (612 lines), `title.rs` with `TitleField` (376 lines), `Pane::Title`, `rune-db/src/rename.rs` (`rename_bind`/`rename_replace`), `GuardKind::RenameCollision`, `tests/rename.rs` (681 lines). Still do not re-implement any of it — but the earlier "do not edit those files" constraint is lifted; they are mainline now.
- **`keymap.rs` is already split.** `64fdad7` extracted `binding.rs`, `global.rs` and `keystate.rs` from a formerly monolithic 608-line `keymap.rs`, and added live leader chords plus a contextual footer. WP6 must **extend** that structure, not re-split it. New tests came with it: `tests/leader_chord.rs`, `tests/keystate_cost.rs`.
- `rune-md` and `rune-tui/src/render.rs` were **untouched** by the drift, so WP2's territory is unaffected.

**Architectural decisions this plan implements:**

- *One shared scope namespace.* Markdown roles get canonical scope names (`markup.heading.1`, `markup.raw.block`) in the same table as tree-sitter captures, resolved by longest-dotted-prefix at configure time — the rule Helix (`rfind('.')` loop), Zed (`BTreeMap` range search) and Neovim (`@comment.documentation` → `@comment`) converged on independently.
- *Span becomes an enum*: `Identical { range }` vs `Substituted { text, cell_map }` — Zed's factoring. Makes "identical text carrying a cell map" unrepresentable.
- *Raw tree-sitter, never `Tree::edit`.* `tree-sitter-highlight` is disqualified: it passes `old_tree: None` and hardcodes `0..usize::MAX`. Avoiding `InputEdit` removes the documented trigger for C asserts that `abort()` the process.
- *Keep the last good highlight, clamped.* Results are version-tagged; a late result for a superseded version is dropped.
- *Theme stores truecolor; one quantizer maps to `Color::Indexed`* when the terminal lacks truecolor. macOS Terminal.app is 256-colour only.
- *Derived preview buffer* for replace preview — `Buffer::apply_edits` already returns a new `Buffer` (`rune-core/src/buffer.rs:215`), so no rollback and no journal contamination.
- *Flat binding table + when-clauses + a derived prefix index*, rejecting prefix/standalone collisions at startup rather than shadowing them as VS Code does.
- *Document kind selects the producer.* A document is `Markdown`, `Code(lang)` or `Plain`, derived from its path extension. comrak runs **only** for `Markdown`; `Code` documents get an identity span list plus a tree-sitter overlay. Wrap stays on and there is no gutter for any kind — a code file is an ordinary `Document` with a different producer.

**Binding repo rules.** `CONSTITUTION.md` is law; articles are cited by frozen §. §1.6: no file past 500 LoC. §1.3: halt with a surfaced error, never `panic`. §1.5: buffer offsets in bytes, display widths in runes. §5.2: render is pure. §5.3: update must not block. §1.4.9: filesystem only through the injected `vfs::Vfs`. `CLAUDE.md` fixes the vocabulary — *textedit*, *materialize*, *journal*, *observation*.

**DO** run Rust commands from `rust/` via its `Makefile`. **DO** keep every new file under 500 lines. **DON'T** add `panic!`/`unwrap`/`expect` outside `#[cfg(test)]`.

## Gotchas

- **`make test` needs the sandbox disabled.** `GOCACHE` and `/usr/bin/trash` are blocked otherwise; a red suite there is the sandbox, not a regression.
- **`rune-fuzz` does not depend on `rune-db`.** Its `Cargo.toml` lists only `rune-core`, `rune-md`, `rune-tui`, `rune-vfs`, `base64`, `proptest`. Any invariant about journal coalescing is out of reach — do not plan one.
- **The undo-coalescing clock is already injectable.** `journal::append_edit(tx, session_id, now: SystemTime, …)` (`rune-db/src/journal.rs:72-76`) takes time as a parameter; `Store::append_edit` samples `self.clock` (`store.rs:357-359`); `Store::open_in_memory(clock, …)` (`store.rs:102-106`) accepts injection. Nothing to fix.
- **`ts_assert` is live in release builds.** `tree-sitter`'s `lib/src/ts_assert.h` compiles to a real `assert()` unless `NDEBUG` is defined, and neither its `build.rs` nor `cc` defines it. A failed C assert is `SIGABRT` — uncatchable, unaffected by the workspace lints. Hence: never construct an `InputEdit`.
- **`QueryMatches`/`QueryCaptures` are not `Iterator`.** Since 0.25 they implement `StreamingIterator` (re-exported from the `tree_sitter` crate root). `for m in cursor.matches(...)`, `.filter()`, `.collect()` will not compile.
- **`Point.column` is a byte offset within the line**, not a character count; `TSInputEdit`/`TSPoint` are `uint32_t` at the FFI boundary.
- **`slice_spans` is the hard part of WP2.** `rune-md/src/wrap.rs:336-380` branches on `s.state` and slices `cell_map` by **rune** offsets while slicing `text` by **byte** offsets. Preserve that distinction (§1.5).
- **`conceal_roundtrip.rs` is 1407 lines and asserts on span fields** via `line.spans` without importing the type name — see `:150`, `:402`, `:1355`, `:1367-1369`.
- **Nine test files re-implement their own `TestBackend` → `String` extractor**: `tests/{tui_render:59-85, banner:37-56, help:39-48, title_breadcrumb:44-57, chrome:34-57, opentabs:88-97, tui_edit:70-79}.rs`, plus `src/title.rs:51-65` and `src/breadcrumb.rs:215`. WP1 creates the shared one; do not add a tenth.
- **`diff.sh` always exits 0** (`scripts/parity/diff.sh:2-4,16`) — it is a report. `assert.sh` is the gate.
- **A held-space leader is partially landed and inert.** `global.rs:105` `LEADER_BINDINGS`, `keystate.rs:123` `leader_available()`; `TODO.md:101-121` records its WP4–WP6 as BLOCKED on an unverifiable `CGEventSourceKeyState`-over-SSH gate. Extend around it; do not delete or un-block it.
- **`styles.rs` has no chokepoint.** Sixteen chrome functions (`pane_title()` `:51` … `title_text()` `:130`) each build a `Style` from constants; only `markdown(id)` `:139` is a funnel. All seventeen must be rerouted.
- **Four files are already over §1.6's 500-line limit** and grow here: `commands/edit.rs` 802, `app.rs` 668, `commands/nav.rs` 530, `explorer.rs` 522. Each package that grows one of them splits it in the same package.
- **§1.6 is satisfied by splitting a file, never by deleting its doc comments.** A first attempt at WP2 landed `wrap.rs` at exactly 500 lines by cutting 39 doc lines — including the module-level explanation of why a `Rendered` span's `text` and `cell_map` are sliced at a wrap break while its buffer range is not, citing Go's `wrap_map.go:411-424`. That comment is what stops a later reader "fixing" the asymmetry. Trading it for a line count is a net loss; split the file instead.
- **A snapshot must not own a copy of the document.** The same attempt gave `WrapSnapshot` a private `String` copy of the buffer so `visual_col`/`byte_col_from_visual` could keep their signatures — an O(n) copy per wrap sync, and the exact ownership the `Identical { range }` variant exists to remove. Thread `content: &str` into the query methods that need it, as `segment_cells` already does.

## Assumptions

### A1. The parity corpus is authored fresh
- **Recommendation:** WP1 authors 10 `.md` fixtures under `scripts/parity/fixtures/`. Only two standalone `.md` fixtures exist today (`fixtures/sample.md`, 8 lines; `testdata/images.md`).
- **Pros:** targets exactly the constructs where wrap/conceal/width bugs live. **Cons:** hand-authored coverage.

### A2. Grid parity is asserted only over markdown documents
- **Recommendation:** the gate compares Go and Rust over markdown fixtures only — Go has no tree-sitter, no search preview, no editable title.
- **Pros:** the oracle stays valid as Rust grows past Go. **Cons:** new surfaces rely on Rust-side tests and the fuzzer.

### A3. `[R]eplace` blob attribution stays as the worktree implemented it
- **Recommendation:** do not re-litigate. `rune-db/src/rename.rs` files displaced bytes under the *renaming* document's `doc_id` (its test asserts `displaced.doc_id == f.ds.doc_id, "captured under OUR doc"`) and blanks any pre-existing row's path.
- **Pros:** shipped and tested, outside this plan. **Cons:** §12 makes observations the merge-ancestor source, so a foreign file's hash there is a latent wrong-ancestor risk — recorded in WP11 as a `TODO.md` entry.

## Risks

- **Merge conflict with the locked worktree.** WP5–WP9 land in `app.rs`, `keymap.rs`, `global.rs`, `explorer.rs` — all edited by `worktree-kind-inventing-marshmallow`. Mitigation: a dry-run merge is a Verification gate, and no package edits `title.rs`, `pane.rs` or `GuardKind`. If the dry run conflicts, land that branch first and rebase.
- **WP2 touches 21 files across 3 crates.** Mitigation: the producer side has one chokepoint — `push_span_split_by_line` (`rune-md/src/emit/mod.rs:216-224`) is the only constructor and `walk.rs` builds no literal. Consumers are three sites: `wrap.rs:336-380`, `render.rs:143-178`, `conceal_roundtrip.rs`.
- **Extracting `RevealState` pulls its siblings.** `RevealSm` (`element/mod.rs:28-59`), `RevealGrant`, `InheritCtx`, `ByteRange` are used together at ~15 sites. Mitigation: WP3 moves all five as a unit; the only external consumer is `render.rs:17,149`.
- **Grammar compile time.** `tree-sitter-cpp`'s `parser.c` is 25.8 MB and typescript's 8.7 MB, each one translation unit. Mitigation: the language set excludes both.
- **Uneven grammar query coverage.** `tree-sitter-typescript` exports no `INJECTIONS_QUERY`. Mitigation: a missing query registers as an empty string, never an error.
- **The 100 ms perf budget.** `full_pipeline_5k_under_100ms` (`rune-md/tests/perf_guard.rs:80`) bounds the pipeline `App::sync_view` drives. Note the real shape, verified against `runtime.rs:189,216`: `sync_view` runs in the runtime loop *after* `apply()` returns, and the loop first drains the channel into a batch (`runtime.rs:206-208`), so a burst of keystrokes costs **one** pipeline run and one draw — not one per message. The concern is therefore per-frame, not per-keystroke. WP8's preview adds a second pipeline run per frame while the replace field is non-empty. Mitigation: the preview reuses the real viewport and is gated on a non-empty replace field; `make perf-guard` gates WP8.
- **Line numbers in this plan are anchors, not addresses.** `sync_view` was read at `app.rs:293`, `:332` and `:347` at different points this session, in a 668-line file at one commit. Locate every cited symbol by name (`rg -n 'fn sync_view'`) and treat the line number as a hint only.
- **ACCEPTED:** nothing protects against a `SIGABRT` from linked C. Avoiding `InputEdit` removes the documented trigger, not the class.

## Work Packages

### WP1. A glyph-grid parity gate over a markdown corpus

**Steps:**
- WP1.S1. Create `rust/crates/rune-tui/src/testgrid.rs`, gated `#[cfg(any(test, feature = "testgrid"))]`, exporting `pub fn grid(app: &App, w: u16, h: u16) -> Vec<String>` and `pub fn row(app: &App, y: u16, w: u16) -> String`, built on `ratatui::backend::TestBackend` as `tests/tui_render.rs:59-85` does. Declare `mod testgrid;` in `rune-tui/src/lib.rs`.
- WP1.S2. Replace the private extractors in `rune-tui/tests/{tui_render,banner,help,title_breadcrumb,chrome,opentabs,tui_edit}.rs` and `src/breadcrumb.rs`'s test module with `testgrid` calls. Leave `src/title.rs` untouched (worktree conflict).
- WP1.S3. Add 10 fixtures under `scripts/parity/fixtures/`: `headings.md`, `emphasis.md`, `lists.md`, `tasks.md`, `fences.md`, `quotes.md`, `tables.md`, `frontmatter.md`, `cjk.md`, `emoji.md`; each ≤ 40 lines.
- WP1.S4. Add `scripts/parity/grid.sh`, modelled on `assert.sh`: per fixture, capture both sides through the existing `capture.sh` path, strip ANSI from the `.txt` captures, right-pad rows to the capture width, `diff` the grids, exit non-zero on any difference, and write failures to `.scratch/parity/out/grid-<fixture>.diff`.
- WP1.S5. Add `parity-grid` to the root `Makefile` beside `parity-assert` (`Makefile:79-80`) and to `.PHONY` (`Makefile:45`).
- WP1.S6. Record any fixture that does not yet pass under the existing "Known divergences" heading in `scripts/parity/README.md`, one line each.

**Done when:**
- `cd rust && make test` exits 0.
- `rg -c 'TestBackend::new' rust/crates/rune-tui/tests rust/crates/rune-tui/src/breadcrumb.rs` returns 0.
- `make parity-grid` exits 0.
- `ls scripts/parity/fixtures/*.md | wc -l` ≥ 11.

### WP2. `SyntaxSpan` becomes an enum

**Steps:**
- WP2.S1. In `rune-md/src/emit/syntax.rs`, replace the struct at `:17-27` with `pub enum SyntaxSpan { Identical { style: StyleId, range: Range<usize> }, Substituted { style: StyleId, text: String, range: Range<usize>, cell_map: CellMap } }`, plus accessors `style()`, `range()`, `is_rendered()`, and `text<'a>(&'a self, content: &'a str) -> &'a str`.
- WP2.S2. Update the sole constructor `push_span_split_by_line` (`rune-md/src/emit/mod.rs:216-276`) to emit `Substituted` when `state == RevealState::Rendered` (it already computes `cell_map` at `:266`) and `Identical` otherwise; update the literal at `:355-362` to `Identical`.
- WP2.S3. Rewrite `slice_spans` (`rune-md/src/wrap.rs:336-380`) to match the variants, preserving the rule exactly: `Identical` re-bases `range` by byte offsets; `Substituted` keeps the full original range and slices `cell_map` by **rune** counts and `text` by **bytes** (§1.5).
- WP2.S4. Rewrite `segment_cells` (`rune-tui/src/render.rs:143-178`) to match the variants, replacing the `(state, cell_map)` tuple match at `:148`. Delete the now-unreachable `debug_assert_eq!` at `:158-163`.
- WP2.S5. Update assertions in `rune-md/tests/conceal_roundtrip.rs` (`:150`, `:402`, `:1352-1370`, `:1402`) and `tests/wrap_roundtrip.rs:41-46`.

**Done when:**
- `cd rust && make test` exits 0 (includes `cargo test -p rune-md --features strict-invariants`).
- `rg 'span\.(text|state|cell_map|buffer_start|buffer_end)' rust/crates` returns no matches outside `rune-md/src/emit/syntax.rs`.
- `make parity-grid` exits 0.
- `cd rust && make perf-guard` exits 0.

### WP3. Extract the `rune-syntax` crate

**Steps:**
- WP3.S1. Create `rust/crates/rune-syntax/` with `Cargo.toml` depending on `rune-core`, `unicode-width`, `unicode-segmentation`; add it to `members` in `rust/Cargo.toml` explicitly, not by glob.
- WP3.S2. Move `rune-md/src/emit/syntax.rs`, `rune-md/src/wrap.rs`, and from `rune-md/src/element/mod.rs` the types `RevealState`, `RevealSm`, `RevealGrant`, `InheritCtx`, `ByteRange`, `CursorProbe` into `rune-syntax`. Leave `Block`, `Inline`, `DocMachine` in `rune-md`.
- WP3.S3. Move `StyleId` (`rune-md/src/emit/style.rs:11-37`) into `rune-syntax` unchanged; WP4 replaces it.
- WP3.S4. Add `rune-syntax` to `rune-md`'s and `rune-tui`'s `Cargo.toml`; update every import the move breaks.
- WP3.S5. Update `rune-fuzz/tests/invariants/support.rs:141` and `rune-tui/src/styles.rs:7` to import from `rune_syntax`.

**Done when:**
- `cd rust && make build && make test && make lint && make fmt` all exit 0.
- `rg '^use crate::element::' rust/crates/rune-md/src/wrap.rs` returns 0 matches.
- `rg 'rune-syntax' rust/Cargo.toml rust/crates/rune-md/Cargo.toml rust/crates/rune-tui/Cargo.toml` matches in each.
- `make parity-grid` exits 0.

### WP4. Scope namespace and the Catppuccin theme

**Steps:**
- WP4.S1. In `rune-syntax`, add `pub struct ScopeTable` owning the dotted-name → dense `ScopeId(u16)` mapping, with `register(&mut self, name: &str) -> ScopeId` and `resolve(&self, name: &str) -> Option<ScopeId>` implementing longest-dotted-prefix fallback (strip after the last `.` until a hit or no dots remain). **`rune-syntax` owns the table; the theme only maps `ScopeId → Style`.** Replace `StyleId` with `ScopeId` on `SyntaxSpan`.
- WP4.S2. Map each former `StyleId` variant to a canonical scope in `rune-md`'s emitter: `Text`→`text`, `H1`…`H6`→`markup.heading.1`…`.6`, `Bold`→`markup.strong`, `Italic`→`markup.italic`, `Strike`→`markup.strikethrough`, `Code`→`markup.raw.inline`, `CodeFence`→`markup.raw.block`, `Link`/`WikiLink`→`markup.link`, `Blockquote`→`markup.quote`, `ListMarker`→`markup.list`, `TaskMarker`→`markup.list.checked`, `Hr`→`punctuation.special`, `FrontmatterDim`→`comment`, `Verbatim`→`text`. Composite emphasis resolves to its strongest component with modifiers carried on the theme entry.
- WP4.S3. Create `rune-tui/src/theme/mod.rs` with `pub struct Theme { scopes: Vec<Style>, chrome: ChromeStyles }` — `scopes` indexed by `ScopeId`, built by walking the `ScopeTable` WP4.S1 populated — and `ChromeStyles` with one field per existing chrome function. Add `theme/catppuccin.rs` defining Mocha as truecolor `Color::Rgb` constants from the MIT `catppuccin` crate.
- WP4.S4. Add `theme/quantize.rs` with `pub fn to_ansi256(c: Color) -> Color`: the 6×6×6 cube (levels `0,95,135,175,215,255`, index `16 + 36r + 6g + b`) plus the 24-step grey ramp (`8..=238` step 10), choosing whichever candidate minimises Euclidean RGB distance.
- WP4.S5. Add `theme/probe.rs` with `pub fn supports_truecolor(term: &impl Terminal) -> bool`: emit a `termina::escape::csi` device-attributes query, read the typed `Csi` response, fall back to `COLORTERM in {"truecolor","24bit"}`. Apply the quantizer once at `Theme` construction — never per frame.
- WP4.S6. Delete the colour constants (`styles.rs:11-46`) and all 17 style functions (`:51-194`); replace with accessors reading the `Theme` held on `App`. Update every call site the compiler reports.

**Done when:**
- `cd rust && make build && make test && make lint` exit 0.
- `rg 'Color::Indexed|Color::Rgb' rust/crates/rune-tui/src --glob '!theme/*'` returns 0 matches.
- A test in `theme/quantize.rs` asserts `to_ansi256(Color::Rgb(0x1e,0x1e,0x2e))` and two other Mocha values map to fixed asserted indices.
- A test in `rune-syntax` asserts `resolve("markup.heading.marker")` falls back to the entry registered for `markup.heading`.
- `make parity-grid` exits 0, or every new difference is recorded in `scripts/parity/README.md`.

### WP5. `rune-ts` and document-kind producer selection

**Steps:**
- WP5.S1. Create `rust/crates/rune-ts/` depending on `tree-sitter = "0.26"`, `rune-syntax`, and grammar crates `tree-sitter-rust`, `-json`, `-toml-ng`, `-yaml`, `-bash`, `-python`, `-javascript`, `-go`, `-html`, `-css`. Add to `rust/Cargo.toml` members.
- WP5.S2. Add `pub struct LanguageRegistry` mapping a lowercased name or extension to `(tree_sitter::Language, &'static str /* highlights query */)`, with aliases `rs`→rust, `sh`/`zsh`→bash, `yml`→yaml, `js`/`mjs`→javascript, `py`→python. A grammar with no `INJECTIONS_QUERY` registers an empty query, never an error.
- WP5.S3. Add `pub fn highlight(lang, source: &str, viewport: Range<usize>, deadline: Instant) -> Option<Vec<(Range<usize>, ScopeId)>>`: one `Parser`, `parse_with_options` with a `progress_callback` returning `ControlFlow::Break(())` past `deadline`, then the highlights `Query` through a `QueryCursor` with `set_byte_range(viewport)`. **Never call `Tree::edit`; never construct an `InputEdit`.** Iterate with `StreamingIterator`.
- WP5.S4. Add `pub enum DocumentKind { Markdown, Code(&'static str), Plain }` on `Document` (`rune-tui/src/document.rs`), derived from `file_path`'s extension via `LanguageRegistry`: `.md` → `Markdown`, a registered extension → `Code(lang)`, anything else → `Plain`. In `Document::sync` (`document.rs:238`), run `DocMachine`'s comrak pipeline **only** for `Markdown`; `Code` and `Plain` produce one `Identical` span per line at the `text` scope, then wrap as usual. A pathless draft and the Help document are `Markdown`, preserving today's behaviour.
- WP5.S5. Add `HighlightState { version: u64, spans: Vec<(Range<usize>, ScopeId)> }` on `Document`. Add `Msg::Highlighted { doc: DocumentId, version: u64, spans }` to `runtime.rs`'s `Msg` and a `CmdKind` running `rune_ts::highlight` on the existing worker-thread mechanism, dispatched for `Code` documents on open and on buffer version change.
- WP5.S6. On receipt, drop the message when `version != doc.buffer.version()`. When rendering, clamp every stored range to the live byte length (§1.3) and apply it over the `Identical` spans from WP5.S4.
- WP5.S7. Wire code fences: where `rune-md` knows a fence's info string and byte range, resolve it through `LanguageRegistry` and request a highlight for that sub-range, merging into the same span list.
- WP5.S8. Extract `handle_key`/`handle_editor_key`/`handle_db_event` from `app.rs` (668 lines) into `rune-tui/src/dispatch.rs`, the split `TODO.md:83` has deferred five times, so this package's additions do not push it further over §1.6.

**Done when:**
- `cd rust && make build && make test && make lint` exit 0.
- A test asserts `highlight` on `fn main() {}` yields ≥1 span whose `ScopeId` resolves to a `keyword`-prefixed scope.
- A test asserts a result tagged with a stale version is discarded and previous spans survive.
- A test asserts `highlight` with an already-elapsed deadline returns `None` rather than blocking.
- An end-to-end test opens a `.rs` file through `App`, drives one `Msg::Highlighted`, renders via `testgrid::grid`, and asserts the buffer text appears with `DocumentKind::Code("rust")` and **no** markdown conceal applied.
- `rg 'Tree::edit|InputEdit' rust/crates` returns 0 matches.
- `wc -l rust/crates/rune-tui/src/app.rs` reports < 500.

### WP6. Sequence-capable keymap with when-clauses

**Steps:**
- WP6.S1. In `rune-tui/src/binding.rs`, extend `Binding<C>` to carry `keys: &'static [KeyPattern]` and `when: &'static str`. Keep `KeyPattern`, `label()`, `KeyOutcome`.
- WP6.S2. Add `rune-tui/src/when.rs`: a recursive-descent parser for `ident`, `ident == "value"`, `!`, `&&`, `||`, `()`, evaluated against `pub struct Context { focus: FocusTarget, search_open: bool, has_selection: bool, has_multi_cursor: bool, read_only: bool, modal_open: bool, language: Option<&'static str> }`.
- WP6.S3. Add `FocusTarget` in a new `rune-tui/src/focus.rs` — **not** in `pane.rs` (worktree conflict) — with variants `Explorer`, `Tabs`, `Editor`, `SearchField`, `ReplaceField`, derived from the existing `Pane` plus search state.
- WP6.S4. Add `rune-tui/src/keymap/index.rs` building a prefix map from the binding tables at startup; in the same pass return `Err` naming both bindings when one key sequence is a strict prefix of another **within the same binding set**. Validation is per-set, so a vim set may reuse keys the default set binds.
- WP6.S5. Add `Resolution { None, Pending(&'static [Binding<C>]), Matched(C) }` and `KeymapState { pending: Vec<KeyPattern> }` on `App`. `Esc` clears pending and returns the consumed keys so a text surface can re-insert them.
- WP6.S6. Add `pub fn on_next_key(&mut self, f: NextKeyFn)` on `KeymapState` — the out-of-band single-key hook the binding table cannot express and vim requires.
- WP6.S7. Replace `help.rs`'s hand-written `push_editor_section` (`:57-99`) with a reflection pass over the editor binding table so all four sections generate; delete the recorded exception at `help.rs:10-14`.
- WP6.S8. Add a vim binding set (`h`/`j`/`k`/`l`/`i`/`Esc`) plus a `mode` context key, selected by a field on `App` defaulting to the VS Code set.

**Done when:**
- `cd rust && make build && make test && make lint` exit 0.
- A test asserts the index builder returns `Err` for a table containing both `["ctrl+k"]` and `["ctrl+k","ctrl+c"]`.
- A test asserts the same two sequences in *different* binding sets validate successfully.
- A test asserts `when.rs` parses `focus == "Editor" && !read_only` and evaluates it both ways.
- `rg 'push_editor_section' rust/crates/rune-tui/src` returns 0 matches, and a test asserts `help_markdown()` contains a `## Editor` section whose row count equals the editor table's length.

### WP7. Scroll patterns and mouse

**Steps:**
- WP7.S1. Add `scrolloff: u16` (default 5) and `mode: ScrollMode` to `Viewport` (`document.rs:35-39`). Replace `scroll_to_row` (`:59`) with `reconcile(&mut self, cursor_row: usize) -> Option<usize>`: it moves the viewport to honour scrolloff and, when the viewport moved independently, returns the boundary row the cursor must snap to — vim's rule, the cursor is never left off-screen.
- WP7.S2. Add commands `scroll_line_up`/`down`, `scroll_half_page_up`/`down`, `centre_cursor`, `cursor_to_top`, `cursor_to_bottom` in `commands/nav.rs`, all routed through `reconcile`.
- WP7.S3. Enable mouse in `rune-tui/src/term.rs` by writing `CSI ?1002;1006h` on enter and `CSI ?1002;1006l` on exit via `termina::escape::csi` — `termina` exposes no helper.
- WP7.S4. Extend `runtime.rs::translate_event` to map `termina::event::MouseEvent` into a new `Msg::Mouse`; it currently drops every non-`Key`/`Paste`/`Resize` event.
- WP7.S5. Add `PointerState { last_click: Option<(Instant, u16, u16)>, click_count: u8, drag_anchor: Option<usize> }` on `App`, with the clock injected as a field. Multi-click threshold: 500 ms **and** Chebyshev distance ≤ 1 cell.
- WP7.S6. Implement gestures: click positions the caret; alt-click adds a cursor; shift-click extends; double-click selects the word; triple-click selects the whole **logical** line including wrapped rows; wheel scrolls 3 rows.
- WP7.S7. Bind every command from WP7.S2 in the editor binding table, and split `commands/nav.rs` (530 lines) into `nav.rs` plus `nav_scroll.rs` per §1.6 and `TODO.md:88`.

**Done when:**
- `cd rust && make build && make test && make lint` exit 0.
- A test drives `reconcile` with the viewport scrolled away and asserts the cursor snapped to the scrolloff boundary, never off-screen.
- A test asserts two clicks 400 ms apart on one cell produce a double-click and 600 ms apart produce two single clicks, using the injected clock.
- A test asserts a triple-click on a wrapped line selects the full logical line.
- `wc -l rust/crates/rune-tui/src/commands/nav.rs` reports < 500.
- `cd rust && make test-fuzz` exits 0.

### WP8. Search and replace with an inline preview

**Steps:**
- WP8.S1. Add `rune-tui/src/search/engine.rs`: `pub fn find_all(hay: &str, query: &Query) -> Vec<Range<usize>>`, where `Query` carries `literal | regex`, `smart_case` (case-insensitive unless the query contains uppercase) and `whole_word`. Add `regex` as a direct dependency of `rune-tui` at the version the lockfile already resolves for `tree-sitter`.
- WP8.S2. Add `SearchState { query: Query, replacement: String, matches: Vec<Range<usize>>, active: usize, computed_for_version: u64 }` on `Document`, recomputed when `computed_for_version != buffer.version()`.
- WP8.S3. Add the search and replace fields as `FocusTarget` variants with their own bindings, rendered through `render::segment_cells` — do not hand-roll rows.
- WP8.S4. Add a `Mode::Search` arm to `footer.rs`'s precedence ladder rendering `i/N matches` from `SearchState`, so the counter has one owner.
- WP8.S5. Build the preview: when `replacement` is non-empty, apply all matches to a **derived** `Buffer` via `Buffer::apply_edits` (`rune-core/src/buffer.rs:215` — returns a new buffer, never mutates) and a fresh `DocMachine::new()`, seeded with **the real document's viewport width and height**, and render the frame from that. Never route it through `commands::edit::commit_edit_batch`.
- WP8.S6. Implement `replace_one` and `replace_all`, each committing through `commit_edit_batch` as a **single** edit batch, hence one undo step.

**Done when:**
- `cd rust && make build && make test && make lint` exit 0.
- A test asserts `find_all` with query `Foo` is case-sensitive and with `foo` is case-insensitive.
- A test asserts a regex replacement with `$1` expands the capture.
- A test asserts that rendering with a non-empty preview leaves `doc.buffer.version()`, `doc.journal.pos()` and `doc.is_dirty()` unchanged.
- A test asserts the preview wraps at the real document's width, not `Viewport::default()`'s 80.
- A test asserts `replace_all` over 3 matches pushes exactly one journal step and one ⌘Z restores the original bytes.
- `cd rust && make perf-guard` exits 0.

### WP9. Editing commands and structural navigation

**Steps:**
- WP9.S1. Replace `char_class` (`commands/nav.rs:52`) with a Unicode-aware classifier using `unicode-segmentation` word boundaries, fixing Go's ASCII-only classes; record the divergence in `TODO.md`.
- WP9.S2. Add to `commands/edit.rs`: `move_line_up`/`down`, `clone_line_up`/`down`, `delete_word_left`/`right`, `delete_line`, and newline auto-reindent from the previous line's leading whitespace — each one `commit_edit_batch` call, hence one undo step.
- WP9.S3. Add multicursor `add_cursor_above`/`below` using `CursorSet::add` and `merge` (`rune-core/src/cursor.rs:194`).
- WP9.S4. Add `commands/structural.rs`: `match_bracket` and `select_to_match`, scanning for the delimiter under or adjacent to the caret and skipping any position whose span resolves to a `markup.raw` or `comment` scope.
- WP9.S5. Add `expand_selection`/`shrink_selection`, driven by `rune-md`'s `Block`/`Inline` tree for `DocumentKind::Markdown` and by `rune_ts`'s tree for `Code`, with a shrink stack on `Document`.
- WP9.S6. Bind every command added in this package in the editor binding table, and split `commands/edit.rs` (802 lines) into `edit.rs` plus `edit_lines.rs` per §1.6 and `TODO.md:86`.

**Done when:**
- `cd rust && make build && make test && make lint` exit 0.
- A test asserts `word_left` over `привіт світ` stops at the word boundary, not at every non-ASCII character.
- A test asserts `move_line_down` then one ⌘Z restores the buffer byte-for-byte.
- A test asserts `match_bracket` inside a fenced code block ignores a bracket in surrounding prose.
- A test asserts `expand_selection` grows word → paragraph in markdown and to the enclosing node in Rust.
- `wc -l rust/crates/rune-tui/src/commands/edit.rs` reports < 500.

### WP10. Extend the session fuzzer

**Steps:**
- WP10.S1. Add to `rune-fuzz/src/action.rs`'s `Action`: `Mouse(MouseKind, u16, u16)`, `SearchQuery(String)`, `ReplaceText(String)`, `Focus(FocusTarget)`; extend `generate.rs`'s generators and `script.rs`'s codec for each.
- WP10.S2. Add `rune-fuzz/src/invariant/preview.rs` with `PREVIEW-NO-MUTATE`: after any frame rendered with a preview active, `buffer.version()`, journal position and dirty flag equal their pre-render values.
- WP10.S3. Add `STALE-HIGHLIGHT-CLAMPED` in `invariant/render.rs`: every highlight range used in a frame is within the live buffer length and on a char boundary.
- WP10.S4. Add `invariant/keymap.rs` with `CHORD-NO-SWALLOW`: after a pending chord is cancelled, printable characters delivered to the buffer plus those returned by the cancellation equal those typed.
- WP10.S5. Teach `CELL-OFFSET` (`invariant/render.rs:65-85`) which buffer a frame was rendered from, by carrying that buffer's length on the snapshot rather than reading the live document.
- WP10.S6. Do **not** add a `rune-db` dependency to `rune-fuzz`; record that boundary in `rune-fuzz/src/lib.rs`'s module doc.
- WP10.S7. Split `rune-fuzz/src/generate.rs` (523 lines) and `driver.rs` (551 lines) as this package grows them, per §1.6 and `TODO.md:97,99`.

**Done when:**
- `cd rust && make test-fuzz` exits 0 with `RC=512`.
- `cd rust && RS=1 make test-fuzz` exits 0, and a second run with the same `RS` produces an identical report.
- `rg 'rune-db' rust/crates/rune-fuzz/Cargo.toml` returns 0 matches.
- `rg 'PREVIEW-NO-MUTATE|STALE-HIGHLIGHT-CLAMPED|CHORD-NO-SWALLOW' rust/crates/rune-fuzz/src` returns ≥1 match each.
- `wc -l rust/crates/rune-fuzz/src/generate.rs rust/crates/rune-fuzz/src/driver.rs` both report < 500.

### WP11. Amend the Constitution and clear stale bookkeeping

**Steps:**
- WP11.S1. In `CONSTITUTION.md`, rewrite §1 (`Go Fundamentals`), §2.2 (the `tea.Model` contract) and §5 (`The Elm Cycle`) to state the principle first with Go and Rust specifics as sub-points. **Never renumber or delete an article.**
- WP11.S2. Add to §1.3 that linked C can `abort()` outside every Rust lint, citing `tree-sitter`'s `ts_assert.h`, and that the port's mitigation is never constructing an `InputEdit`.
- WP11.S3. Add to §12: the shared scope namespace, the `Identical`/`Substituted` split, the no-`InputEdit` rule, the flat binding table with startup collision rejection, `DocumentKind` producer selection, the derived preview buffer, and the glyph-grid oracle. Amend the `CellBuilderFunc` bullet to record that the Rust port needs no such seam because one namespace made styling uniform.
- WP11.S4. Strike the resolved VFS entry at `TODO.md:63-67` — it asks for a split into `exchange`/`rename_excl` that shipped (`rune-vfs/src/lib.rs:83,89`, `disk.rs:114-120`) — using the `~~…~~` convention already used at `TODO.md:84`.
- WP11.S5. Add a `TODO.md` entry recording A3: displaced-file bytes are filed under the renaming document's `doc_id`, and §12 makes that stream the merge-ancestor source.
- WP11.S6. Add a `TODO.md` entry recording that `full_pipeline_5k_under_100ms` permits a 100 ms keystroke on a 5,000-line document inside a synchronous `sync_view`.

**Done when:**
- `rg '^## §13' CONSTITUTION.md` returns 0 matches (amended in place, no addendum).
- `rg -c '^### §' CONSTITUTION.md` returns the same count as before this package.
- `rg '~~.*save_atomic' TODO.md` returns a match.
- `rg 'displaced|100 ?ms' TODO.md` returns ≥2 matches.

## Verification

Run from the repository root unless noted:

```
cd rust && make build
cd rust && make fmt
cd rust && make lint
cd rust && make test
cd rust && make perf-guard
cd rust && make test-fuzz
cd rust && RS=1 make test-fuzz
make parity-capture
make parity-assert
make parity-grid
git merge --no-commit --no-ff worktree-kind-inventing-marshmallow && git merge --abort
```

The final command is a dry run proving these packages still integrate with the unmerged title/rename branch; any conflict means landing that branch first and rebasing.

`make test` and `make test-fuzz` need the sandbox disabled — `GOCACHE` and `/usr/bin/trash` are blocked otherwise, and a red suite there is the sandbox, not a regression.

Manual confirmation via `scripts/parity/serve.sh`: open a `.rs` file and confirm tree-sitter highlighting, bracket matching and expand-selection; open a markdown file containing a Rust fence and confirm both vocabularies style within one document; run a regex replace-all with a `$1` capture and confirm the inline preview renders before commit and one ⌘Z reverts the whole replacement. Check both colour depths — iTerm2 or Ghostty for truecolor, Terminal.app for the quantized path.
