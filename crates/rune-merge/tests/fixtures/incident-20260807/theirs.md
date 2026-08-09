# The Rune Constitution

This is how we do things here. Every article is binding, not advisory — when designing a feature or reviewing a change, check it against every article that applies.

Article numbers (§) are stable within this document, and `CLAUDE.md`'s Unbreakables digest cites them. Code never cites this file — an invariant is stated in the comment or the test that guards it, in its own words, not by § reference. When code and this document diverge, one of them is wrong; fix it in the same change that found it. First time in the repo? `CLAUDE.md` orients you; this file is the law.

**Contents:** §0 Prime Directive · §1 The Single I/O Seam · §2 Atomic Publish · §3 Save Lifecycle · §4 The Recovery Store · §5 Crash Recovery · §6 Byte Faithfulness & Coordinate Spaces · §7 Edit Safety · §8 Dirty State · §9 No Panic — Halt Discipline · §10 The Update Cycle · §11 Dispatch & Keymap · §12 State Residency & Reconciliation · §13 Layout · §14 The Display Pipeline · §15 Testing & Verification · §16 Pre-Merge Checklist

---

## §0 Prime Directive — Protect the User's Words

Never corrupt and never lose what the user wrote. When data safety conflicts with performance, elegance, or features — data safety wins.

### §0.1 The Harm Ladder

Rank every defect by the highest rung it can reach; always trade a failure DOWN the ladder:

1. **Catastrophic — silent corruption.** Wrong/garbled/reordered bytes, mangled UTF-8, a silent rewrite (line endings, trailing newline, BOM, encoding), a good file overwritten by a bad buffer. Never ships.
2. **Severe — losing more than a few seconds of work.** A crash, failed save, or botched recovery that discards unsaved edits.
3. **Tolerable — everything else.** Render glitch, wrong layout, dropped keypress, a clean halt that loses nothing.

Prefer a Tolerable halt — a surfaced error that keeps the buffer — over any higher rung. Halt with a visible error, never `panic` (a panic takes the unsaved buffer with it).

---

## §1 The Single I/O Seam

All access to user files funnels through the injected `Vfs` trait object; raw `std::fs` calls exist nowhere outside `rune-vfs`. Two implementations exist, deliberately no third: `Disk` (the real filesystem, atomic rename via `renamex_np`) and `Mem` (a fault-injecting test double). Exactly one `Vfs` is constructed in `main`, before anything else, and threaded by `Arc` into the app, the recovery store, and every spawned `Cmd`. Every path that will ever bind to a `Document` resolves through one chokepoint — `workspace::resolve` — so a symlink, a `..` traversal, and a duplicate spelling of the same file collapse to one identity before anything opens it.

Anchors: `Vfs` (`read`, `write_durable`, `exchange`, `rename_excl`, `remove`, `stat`, `resolve`, `mkdir_all`, `read_dir`), `Disk`, `Mem`, `workspace::resolve`.

Documented exceptions, exhaustive: launch bootstrap's `current_dir`/`$HOME` read; the SQLite recovery-store's own file (opened by `rune-db`, which is `rune-vfs`'s sibling, never its caller, and which owns its own separate `std::fs` use for its database file and garbage collection — a correct exception, named here rather than left implicit); test and fuzz tooling; OS resources that are not file content (`/usr/bin/open`, `pbpaste`/`pbcopy`).

## §2 Atomic Publish

User content reaches disk only through a durable temp write — create-new, write, fsync data, `O_CLOEXEC` — followed by exactly one atomic publish: `Vfs::exchange` to overwrite (`RENAME_SWAP`) or `Vfs::rename_excl` to create (`RENAME_EXCL`), each a single kernel rename, then a parent-directory fsync. Neither the temp nor the destination is ever unlinked as a separate step — the rename itself is the publish.

A durability failure that happens *after* the rename already took effect is a distinct typed state, `published_not_durable`: the swap succeeded, the temp file is the sole surviving copy of whatever it just displaced, and it must never be removed on this path. Callers treat `published_not_durable` as physical success, not failure.

Anchors: `Vfs::exchange`, `Vfs::rename_excl`, `published_not_durable`.

Honest scope: `Vfs::save_atomic` is a compatibility convenience, not the rule — it overwrites without capturing what it displaces, so it cannot satisfy §3.4 (capture before discard). It is reachable exactly once: the no-store fallback save, where the alternative is refusing to save at all. State this plainly rather than claiming capture-before-discard is universal.

## §3 Save Lifecycle

**§3.1 Refusal ladder.** `trigger_save` returns a typed `SaveStart` (`InFlight | NotDirty | NeedsName | Refused`); every rung refuses with a stated reason before a single byte moves, in load-bearing order: image document, preview document, save-already-in-flight, rename-in-flight, an active conflict resolver with unresolved blocks on the target document (a half-resolved conflict-marker working form must never be published over the user's file), not-dirty (re-derived, §8), pathless draft (focuses the title field, `NeedsName`), document unbound to the recovery store (falls to `save_atomic`, §2, so the user can always save), degraded store (arms the confirm gate below).

**§3.2 Degraded confirm gate.** On a degraded store, the first ⌘S arms a document-tagged confirm; the same document's second ⌘S within `SAVE_CONFIRM_TIMEOUT` (2s) proceeds. The message names the document because the pending slot is a single global — a second document's ⌘S must not silently confirm the first's save.

**§3.3 CAS conflict guard.** Before an overwrite, the live target is read and hashed; a mismatch against the expected baseline is `Conflict` (no write happens); a vanished target is `Missing` (never silently recreated); a bound-path disagreement is `PathDisagreement` (a caller bug — degrades the store rather than corrupting the file). On create, a racing `rename_excl` surfaces `AlreadyExists`: the winner's bytes are read back, the conflict is recorded, and the draft stays untitled rather than silently adopting the winner's name.

**§3.4 Capture before discard, mechanically.** `exchange` swaps first; the temp path is then re-read and re-hashed; a mismatch against the CAS expectation is captured as a durable blob before the temp is ever removed — the temp is the only place the user's just-displaced bytes still physically exist, so a genuine I/O failure on this path must never remove it. The blob write and the observation that references it commit as one database transaction, closing the cross-process GC race. Capture is never gated on UTF-8 validity — blobs are `&[u8]`. The identical shape protects a destructive rename.

**§3.5 Capture once, never re-derive.** Content, path, CAS baseline, and journal position are captured synchronously at the moment a save is triggered (`Document::begin_save`); only those exact captured bytes are ever promoted to `saved_content` — never a later re-read of the buffer.

**§3.6 A lost ack never loses a save.** Once the physical write completes, the save is reported successful even if the store's bookkeeping enqueue then fails; the buffer and journal are never rolled back on that failure. The recovery store degrades — sticky, visible — but the save itself already happened.

Anchors: `SaveStart`, `trigger_save`, `Raced`, `Document::begin_save`, `saved_content`.

## §4 The Recovery Store

The SQLite store is an observer beside the user's file, never inside it — losing the database must never damage the `.md`. Four roles: a durable per-edit journal (coalesced ≤300ms, redo-truncating), content-addressed snapshots (pure replay anchors), observations (every disk sighting ever recorded), and content-addressed zstd blobs (SHA-256-verified on read).

**§4.1 Session scoping.** Journal position, CAS baseline, events, and snapshots are all scoped to `(doc_id, session_id)`; a second window can never see, coalesce, or truncate another session's edits. Session identity is `(pid, proc_started_at)`.

**§4.2 Ancestor eligibility vs "theirs".** Merge-ancestor eligibility is session-scoped (only this session's own load/save/resolve observations, at or before its own journal position, qualify); the newest disk observation used as "theirs" is deliberately session-UNSCOPED — any session's disk fact is everyone's. Undoing past a merge or discard re-exposes the divergence, because the ancestor is derived fresh, never stored.

**§4.3 Decisions read inside the deciding transaction.** Every value a save/materialize decision depends on is re-read inside the same `BEGIN IMMEDIATE` transaction that then writes the outcome, on the single writer connection; no database transaction is ever held across a `Vfs` call. A separate read-only reader thread serves only display and stale-tolerant reads — a decision read must never be added to its request enum, no matter how convenient.

**§4.4 Versioned by filename, never migrated in place.** A schema change bumps `SCHEMA_VERSION` and opens a new file (`rune-v{N}.db`); the previous file is left untouched. Every `rune-v*.db` must satisfy one frozen liveness query over `sessions(pid, proc_started_at)`; GC of an old file requires an exact filename match, an mtime at least an hour old, a successful read-only liveness query, and every recorded session confirmed dead — on any error, the file is left alone.

**§4.5 Open ladder never hard-fails.** File-backed store, then a retried `mkdir`, then a private in-memory store with `degraded = true`. Degraded is never confusable with a deliberate in-memory session; the footer surfaces a warning that history is unavailable.

**§4.6 Reaper and blob GC are best-effort, never blocking.** The reaper deletes only a confirmed-dead session's own rows, and never a session still the most-recent session for some document it touched; blob GC sweeps only genuinely unreferenced blobs, batched, once per store open, after the reaper runs.

**§4.7 Liveness fails toward "alive".** An ambiguous liveness check treats the session as still running; wrongly refusing to inherit its rows is tolerable, corrupting a live session's journal is not.

**§4.8 A non-UTF-8 path loses recovery, never fidelity.** A workspace path that doesn't round-trip UTF-8 is rejected from the recovery store loudly (never `to_string_lossy`); the document itself still opens, edits, and saves byte-exact through `Vfs`.

Anchors: `ReaderRequestKind`, `SCHEMA_VERSION`, session identity, reaper, blob GC.

## §5 Crash Recovery

Hydration runs at bootstrap, before the TUI exists, and never fails fatally: any failure prints to stderr and the editor still opens, with a banner — zero-crash-protection is never silent.

Recovered content is reconstructed as editable history, never written to the file: the buffer holds the draft, dirty, with one synthetic bridge step so ⌘Z can still reach the on-disk content; the destination changes only on an explicit save. Hydration is deliberately not gated on read-only mode — refusing it to honor a view setting would discard already-typed bytes.

A fabricated CAS baseline is worse than none: if `Load` finds no saved observation to adopt, the document degrades rather than inventing a baseline of zero.

Draft recovery reconstructs every recoverable scratch row left by a dead session (skipping empty/whitespace-only rows and any row still owned by a live session); each draft adopts its own row, never a fresh row copying its text — a fresh row would be re-offered forever. Cross-session inheritance completes before this session writes anything of its own; a diverged inheritance re-anchors on the dead session's own baseline so the newer disk content is never silently discarded.

`Load` records the disk sighting unconditionally, then requires valid text: the blob and hash of the raw bytes are recorded before any UTF-8 decode is attempted.

Anchors: `Document::hydrate`, `db_bootstrap`, `Hydration::Refused`.

## §6 Byte Faithfulness & Coordinate Spaces

Load → edit → save is byte-identical except exactly where the user edited — no normalization anywhere on the save path: line endings, trailing newline, BOM, and encoding all pass through verbatim. Invalid UTF-8 is refused at load, never repaired.

Edit, cursor, journal, and CAS offsets are BYTE offsets into UTF-8. Display width is TERMINAL CELLS over whole grapheme clusters via `unicode-width` — never a byte count, never a `char`/rune count. `BufferOffset` and `VisualCol` are distinct newtypes; the mix-up is unrepresentable.

Exactly two measurement chokepoints exist, and nothing may re-derive the numbers independently: `rune_syntax::wrap::grapheme_width` (+ `TAB_STOP`) is the source of truth; `rune_tui::width::display_width` (the chrome/footer chokepoint) sums the first, so the first stays authoritative. One documented exception: a lone zero-width cluster clamps to 1 cell so the caret can still reach it. Rune's width for a symbol must equal what ratatui derives for that same symbol — enforced by a test that depends on both.

Refuse, don't guess, at the buffer boundary; clamp only at the caller's boundary. `Buffer::apply_edits` validates a range against the live byte length and char boundaries and REFUSES (`OutOfBounds`/`SplitsRune`) rather than clamping or panicking. Callers — chrome fields, highlight queries, merge resync — clamp before the call, each at their own boundary.

Anchors: `BufferOffset`, `VisualCol`, `grapheme_width`, `display_width`, `Buffer::apply_edits`.

## §7 Edit Safety

Exactly four edit chokepoints exist: document text (`edit_core::apply_edit_batch_with_cursors` — the sole buffer-mutating primitive: one call, one journal push, one undo step, read-only guard first); chrome fields (`Field::edit`, refusal is `KeyOutcome::Ignored`, never silent corruption); recovery adoption (`Document::hydrate`); merge install (`install_whole_range`, routed back through the text chokepoint so an installed merge still replicates to the store).

A destructive async replacement is suspect until proven: adoption that empties or shrinks a non-empty buffer by more than half is refused with a surfaced reason, buffer untouched — an empty async reset is never a user deletion.

An async reply never clobbers newer keystrokes: recovery adopts only when the buffer's version still equals the version the request was issued against.

Collapsed post-edit cursor positions are refused at the one chokepoint the write path and the read-back path share (`DuplicateEditStart`); one merge rule for touching deletes is defined once and shared by undo inversion and the batch builder.

The undo journal is peek-then-commit: `undo_peek`/`redo_peek` are read-only; the position only moves after the buffer edit actually succeeds. Replay surfaces corruption rather than silently clamping or skipping it.

Anchors: `edit_core::apply_edit_batch_with_cursors`, `Field::edit`, `install_whole_range`, `DuplicateEditStart`, `coalesce_touching_deletes`.

## §8 Dirty State

Dirty is a CONTENT comparison — the live buffer against the exact bytes the last successful save persisted — never a version-number proxy; a version comparison alone leaves an edit-then-undo document dirty forever.

The dirty flag has exactly one writer (`finish_save_ok` sets the baseline) and one cache, and the cache is render-only: every transition that matters — save trigger, close, evict, switch, quit — recomputes via `is_dirty_now` before it decides anything; only rendering reads the cache.

The journal-position-derived sync state (`Clean | BufferAhead | DiskAhead | Diverged`) is a *separate* fact from the dirty dot: it drives the merge machinery and footer hints, not whether the tab shows a dot.

Anchors: `is_dirty`, `is_dirty_now`, `finish_save_ok`.

## §9 No Panic — Halt Discipline

`panic`, `unwrap_used`, and `expect_used` are workspace-denied lints in every crate; `indexing_slicing` warns. The only allows live under `#[cfg(test)]`. A "can't happen" check is never a bare `assert!`/`debug_assert!` in production — both evade the lint set — it routes through a crate-local `assert_invariant`, gated on `STRICT_INVARIANTS = cfg!(any(test, feature = "strict-invariants"))`, never on `cfg!(debug_assertions)`: an ordinary shipped build, optimized or not, degrades gracefully on a detected violation instead of panicking. The feature is never on by default.

A refusal is never silent to the caller — it is a typed return value (`SaveStart`, `Hydration::Refused`, `KeyOutcome::Ignored`, `Commit::Refused`), not a swallowed `Result`.

The footer's own status ladder is one mutually exclusive `Mode`, never concatenated: Modal error outranks a dirty-quit guard, which outranks an unacknowledged save error, which outranks a pending chord, which outranks a degraded-store warning, which outranks an ordinary status message, which outranks a disk-changed hint, which outranks the default key hints. Exactly one wins per frame.

The error ladder is one raise chokepoint and one clear chokepoint: `report_error` is the sole path every error-reporting call site routes through; a status message carries provenance, and only its own subsystem may clear it — an unrelated cancel must never cost the user their unacknowledged save-failure banner.

Prefer totality to `unwrap`: a value that looks fallible but structurally cannot fail gets a real infallible accessor instead of an `unwrap` at every call site (`App::active_doc` is infallible by construction, not by convention); an ID space that must never collide or wrap uses saturating arithmetic at the mint site rather than a checked panic later.

Halt path: on unwind, `Drop` restores the terminal before `catch_unwind` traps the panic in `main`; `panic = "abort"` is forbidden precisely so that restore can run. The process exits `EX_SOFTWARE`.

**A crash in linked C is not a Rust panic, and no Rust lint can see it.** `tree-sitter`'s `ts_assert` compiles to a live `assert()` in release builds; a failed C assert calls `abort()` → `SIGABRT`, which `catch_unwind` cannot catch and which `panic = "abort"` vs `"unwind"` is irrelevant to. The mitigation here is structural, not a lint: no `InputEdit` is ever constructed and `Tree::edit` is never called — every parse is a full parse from source, which removes the documented trigger for the class of crash tree-sitter's own issue tracker attributes to malformed incremental edits and ABI/grammar mismatches. This is a §0.1 Severe-rung hazard (an uncatchable abort loses the unsaved buffer) that the lint set cannot see and code review must remember by hand.

Anchors: `assert_invariant`, `report_error`, `StatusSource`, `ParsedTree::source`.

## §10 The Update Cycle

`update` is the sole writer of synchronous state; a `Cmd` exists only for work that leaves the thread. `Effects` accumulates I/O for the runtime to perform after the whole message batch is applied. Honest exception: the recovery store's enqueue calls are plain non-blocking channel sends, not I/O, so they run inline from `update` rather than through a `Cmd`.

Render is a pure function of `&App` — `render::draw(app: &App, ..)` never takes `&mut`; every mutation a frame needs happens in the settle step (`App::sync_view`) before `draw` runs.

Terminal bytes leave through exactly one path, `Effects.raw`, on the main thread; a `Cmd` never touches the terminal — the one `Terminal` handle is single-owner by construction.

Every message is applied through one chokepoint, `runtime::apply`, which discharges the `Effects` it produced in a fixed order (raw bytes, then Cmds, then force-redraw, then resize-only graphics redetection); the loop batches with a drain-then-draw-once shape.

Every spawned `Cmd` is tagged with a `CmdKind` naming the physical resource it touches, so a consumer that must not execute certain effects — the headless fuzzer — can decide by inspection rather than by inferring intent from field diffs. Every spawned `Cmd` thread sends something back — a result, `None`, or a caught panic — the loop is never left blocking forever.

There is no cancellation. A superseded async reply is killed by a generation or version echo carried on the original request (`RenameDone.generation`, `DirLoaded.generation`, `Highlighted.version`), never by resolving live state on arrival. Documented exception: `FileOpened` carries no staleness echo because opening a file mutates no shared single-slot state.

A timeout is a message carrying a generation, produced by a dedicated thread — never a sleep inside `update`. The snapshot debounce is the one exception worth naming precisely: it is a single long-lived rearmable timer that parks on a condition variable until the earliest pending document's deadline, not a fresh thread spawned per keystroke; arming it is a plain, pure state update.

Every wall-clock read the model depends on is an injected seam (`Box<dyn Clock>`, §15) — production installs the real clock, nothing in `App` calls `Instant::now()`/`SystemTime::now()` directly.

Anchors: `App::update`, `Cmd`, `Effects`, `runtime::apply`, `CmdKind`, `SnapshotTimer::arm`, `Clock`.

## §11 Dispatch & Keymap

Keys resolve through a fixed four-stage pipeline and no fifth stage: modal capture, then the global chord table, then the focused pane's own keymap, then `Ignored`. Modal capture is total — while a modal is open, every key is consumed there, quit chords included.

Content-bearing messages are deliberately not modal-gated: a paste lands in the journaled, undoable buffer even under a modal, because that is the safer failure mode than losing it.

A mode that owns the working form (merge resolution) intercepts before every hardcoded fast path and consumes every key with visible feedback, never silently.

Bindings are `const` data tables resolved by one stateless function that consults no state and no `when` clause; whole-modifier equality, first-match-wins. Every printable global chord requires `ctrl` or `sup` — "every printable keystroke is text" is structural, verified by a startup test, not a convention.

Every binding table is validated at startup for prefix collisions, duplicate sequences, and malformed `when`; a registry test walks every registered table so a forgotten one is conspicuous, not silently unchecked.

Help is generated by reflection over the real binding tables — a hand-maintained key list may not exist, so Help and each pane's own handling cannot drift apart.

Chord/confirm state lives on `App` as a typed `Option` carrying a generation; the footer renders it, the timer never renders. The same chord twice quits; a different chord while one is pending re-arms with a fresh generation.

**Honest scope**, stated plainly rather than implied:

- Universal per-key feedback is enforced only where a mode captures the keyboard (merge resolution); pane key handlers may still return `Ignored` silently, and `dispatch::handle_key` discards that verdict. The rule as actually enforced: *any mode that captures the keyboard must consume every key with feedback* — it is not yet a structural guarantee for every pane.
- `when` clauses are validated at startup but never consulted by the resolver — today they partition rows within a pane table as documentation, not as dispatch input.
- There is no cross-table keymap-union guard: a global chord can silently shadow a pane binding with nothing structural to catch it. The rule this codebase actually ships: **every new global chord must add its own cross-table claimant test against `KeyPattern::matches`** — a handful of such tests are the whole defense today, and a new global row without one is a gap, not an oversight the architecture prevents.

Anchors: `dispatch::handle_key`, `banner::handle_key`, `GLOBAL_BINDINGS`, `keymap::resolve`, `index::validate`, `help::generate`.

## §12 State Residency & Reconciliation

Per-editing-pane state lives on `Document`; only genuinely app-wide state lives on `App`. Litmus for a doc-tagged exception (`pending_save_confirm`, `pending_quit`): it must be app-wide by nature, not merely convenient to reach.

Focus is a `Pane` enum discriminant, no trait objects; a pane the frame doesn't paint cannot be focused, enforced by the type system — `App::set_focus` takes a `VisiblePane` token, not a bare `Pane`, so "focus lands on a pane nobody can see" does not compile.

Every structural change that can un-paint the focused pane runs the one focus reconciler; resize is a caller of it, not a special case beside it.

No shadow state: a value has exactly one writer, and derived state is re-derived from the thing it describes, never cached and read back as if it were a second source of truth. Dirty (§8) is the canonical example; two independent height computations for the same chrome is the forbidden pattern.

Disk facts update only from an operation's own result — an open, a switch, or a save-time CAS check — never from a watcher, never from a poll. No file-change watcher exists today; external-change detection is exactly the save-time CAS guard (§3.3) plus an explicit disk probe on switch-onto. If a watcher is ever added, its events must flow through this same law: a disk fact updates only from an operation's own recorded result, and a clean-buffer adoption must be journaled and undoable — never a silent buffer reset.

The `App`-to-writer-thread seam is a bridge with a bootstrap buffer: nothing posted during the startup window is lost, and the runtime never exposes its raw message sender to anything that could bypass `update`.

Anchors: `Document`, `Pane`, `VisiblePane`, focus reconciler, `is_dirty_now`.

## §13 Layout

`layout::geometry` is the one geometry chokepoint — a pure function from `(area, &App)` to every rect the frame is built from; no consumer reverse-engineers its own idea of a rect.

Visibility is decided once, in `layout::resolve`; the layout mode is produced at the same moment as the rects and is never re-derived from them afterward.

The layout owner pushes sizes into each pane's viewport during the settle step; a pane never sizes itself. Every dimension clamps to at least 1 in both axes.

Every geometry invariant is a `STRICT_INVARIANTS`-gated assert (§9): fatal in test and opt-in builds, a graceful degrade in a shipped one.

Chrome text (footer, breadcrumb, tab titles) is measured and truncated through the one chrome-width chokepoint (§6) — never re-measured ad hoc.

Anchors: `layout::geometry`, `layout::resolve`, `LayoutMode`.

## §14 The Display Pipeline

`DocMachine::rebuild` is the one chokepoint that produces a `ViewSnapshot` — syntax spans, wrap segments, and display rows, in that fixed order (emit → wrap sync → display snapshot). No consumer re-derives any of them; a single `dirty` flag memoizes the whole pass, and `snapshot()` is the only public door. Rendering, the viewport, and mouse hit-testing all read display-space geometry, never wrap-space directly.

`DocumentKind` selects the producer and there is no second plain-text producer: `Markdown` parses with comrak; `Code`/`Plain` reuse the same emitter with an empty block list (its gap-fill pass yields one verbatim span per line); only `Image` synthesizes rows directly. Emit and wrap still run for an image document so its coordinate maps stay valid — only the display snapshot diverges.

`rune-syntax` owns the emitted vocabulary (`SyntaxSpan`, `SyntaxLine`, `SyntaxSnapshot`, `ScopeId`) and depends on no producer; a producer's only way to the screen is emitting these types. Every buffer byte is accounted as a visible-span byte or a recorded hidden-range byte — gap-fill guarantees no byte is silently dropped, because a dropped byte is a hazard the caret could no longer reach. Every line's spans are sorted into byte order at one place, where every producer's output converges — a producer never hands out spans in its own internal order and trusts a downstream consumer to fix it.

Tree-sitter highlighting is a render-layer overlay, never an emitted `SyntaxSpan` — it patches `Cell::style` keyed by buffer offset, and never reflows a row. Nested captures paint outer-first through one comparator (`start ASC, end DESC, capture order ASC`) into a byte-indexed window sized to the visible bytes; the innermost, latest-painted span wins.

The parse is whole-region and retained; the query is viewport-scoped and stateless — no viewport cache exists to invalidate on scroll. One `ParsedTree` per code region, valid exactly when its retained source still equals the reconstructed buffer region; every frame re-queries each intersecting tree over the visible byte range and stores nothing. Every parse is a full parse (§9's linked-C mitigation) — `Tree::edit`/`InputEdit` is never attempted.

No pass budget is ever divided by region count: a lone expensive region gets the whole per-region cap, and `PassBudget` has no constructor that omits a total, so an unbounded pass is unrepresentable. At most one highlight runs per document; a request that arrives mid-flight only arms a `pending` flag rather than spawning a second one. Both levels of `None` in a reply mean "carry forward, never clear" — a slow document degrades to stale colours, never to no colours.

Reveal (source vs rendered markdown) is cursor-driven and inherits downward: `RevealSm::transition` is the single writer of reveal state, an unfocused document never reveals, and reveal transitions never bump the content version — reveal and content are deliberately disjoint axes.

Four coordinate spaces exist — Buffer, Syntax, Wrap, Display — each its own newtype; Buffer and Syntax columns are bytes, `VisualCol` is cells (§6). Display can exceed Wrap after table/image expansion, so a caret converts wrap-space to display-space through one named function rather than assuming the two ever coincide. The wrap pass itself is greedy, cluster by cluster, against the content width, breaking at the last space when one exists; a `Substituted` span's rendered text and cell map may be sliced by a line break, but its underlying buffer range never is — the cell map alone stays authoritative for that span from the break onward.

Decoration (icons, bullets, quote bars, rules) is metadata riding alongside the display types, never a text mutation: it becomes cells only at render time, always at a sentinel buffer offset that the caret, selection, and click hit-testing can never resolve to. The icon tier (Unicode vs a terminal's native icon font) is chosen once, from environment values read at bootstrap and passed in as data — never re-read per frame, never a source of render-loop nondeterminism.

Images: reserved-row model (synthetic rows carry an `ImageRowRef`, never real span text); Kitty protocol only, gated on true-color capability; deterministic per-path IDs so a respawned pane's placeholders stay stable; teardown always emits a delete so no orphaned image survives the process.

Anchors: `DocMachine::rebuild`, `DocumentKind`, `SyntaxSpan`, `ScopeId`, `ParsedTree`, `PassBudget`, `RevealSm`, `ImageRowRef`, `VisualCol`, `wrap_to_display`.

## §15 Testing & Verification

Behavior tests drive the real update seam — `App::update` with a real `Msg`, most often `Msg::Key` — never poke the state a behavior is supposed to produce. Fixtures may set PRECONDITIONS directly; they never set the asserted OUTCOME. A `Cmd` is never executed inline by a test — assert on the `Effects` it produced, then hand-deliver the reply message yourself.

Time is a field, not a call: `App::pointer_clock: Box<dyn Clock>` — production installs the real clock, tests install a manual one that only advances on command. A debounce or timeout is tested by injecting its due message with the matching generation, never by sleeping. No wall-clock sleeps order events anywhere in the suite; real concurrency that must be awaited rendezvouses on an ordered channel or spins with a deadline, never a fixed sleep.

Ten recurring invariant classes, each with both a unit-test form and a fuzz-checked form: render purity/idempotence; layout and resize bounds; scroll stability; async I/O success AND failure; key routing and focus gating; byte-verbatim round-trips; conceal/reveal round-trips; keymap collision guards; screen assertions (via `TestBackend` through the real render path — no committed frame blobs, one exception for image byte-goldens compared structurally); and architecture guards written as tests (a dependency-graph walk, a single-writer grep gate, a single-construction-site inventory).

**No Vacuous Gate**: every gate must be able to fail — a Makefile target names one test in one binary (`--test X --exact`), a tripwire asserts a minimum step count and a changed final state, a repro-glob check fails on zero matches, a keymap sweep asserts a non-trivial floor of checked rows.

**Self-Checking Claims**: a comment asserting an invariant is not proof — it is replaced by a test that checks it (the single-writer grep gate, the single-construction-site inventory, the dependency guard). If a claim can't be checked mechanically, treat the comment as unverified.

The headless session fuzzer (`rune-fuzz`) drives production code — `App::update`, the real `Cmd`/`Effects` machinery, `Mem` — with no terminal, no clock, no subprocess: same input, same result, always. Every invariant has a stable, greppable ID (`SAVE-VERBATIM`, `PANE-NO-BLEED`, `LAYOUT-FITS`, …), a one-line meaning, and lives in one domain file. A caught bug becomes a permanent replayed-forever repro script, landed only in the same commit as its fix.

Performance budgets are named constants with a wall-clock guard, run only from their own Make target, never as part of the default test run — a soak or timing test is `#[ignore]`d with the reason inline and reachable only that way, so an ordinary `make test` stays fast and deterministic.

File size: keep a source file under 500 lines. An overrun is legal only when recorded in `TODO.md` with a reason and a named split candidate; the list is re-measured wholesale, never accreted from deltas — a file quietly growing past 500 with no entry is itself the defect.

Anchors: `App::update`, `Clock`, `TestBackend`, `assert_invariant`, fuzz invariant IDs.

---

## §16 Pre-Merge Checklist

Verify mechanically before completing a change:

- [ ] User content still reaches disk only via durable temp write + `exchange`/`rename_excl`; unsaved work still lands in the recovery store, never a debounced write to the destination. (§1, §2)
- [ ] The refusal ladder order is intact, and every new refusal rung is a typed `SaveStart` variant, not a swallowed early return. (§3.1)
- [ ] A new write path that can displace existing bytes captures them as a durable blob before removing the temp that holds them. (§3.4)
- [ ] A new database read that a save/materialize decision depends on happens inside the deciding transaction, never split across a read-then-later-write. (§4.3)
- [ ] Bytes stay verbatim on the save path — no normalized line endings, trailing newline, BOM, or encoding. (§6)
- [ ] Edit/cursor offsets are BYTES; every new width/column computation routes through the one grapheme-width chokepoint, not a bare `len()` or `char` count. (§6)
- [ ] A new edit path goes through one of the four edit chokepoints — it does not open a fifth. (§7)
- [ ] Dirty is re-derived on every new destructive transition, never read from the render cache. (§8)
- [ ] No new `panic!`/`unwrap()`/`expect()` in production code; a new "can't happen" check routes through `assert_invariant`, gated on `STRICT_INVARIANTS`, never `cfg!(debug_assertions)`. (§9)
- [ ] A new refusal is a typed return value the caller must handle, not a discarded `Result`. (§9)
- [ ] Any new synchronous state write happens inside `update`, not inside a `Cmd` closure or inside `render::draw`. (§10)
- [ ] A new async reply that can arrive stale carries a generation or version echo and is dropped on mismatch — it does not resolve against live state on arrival. (§10)
- [ ] A new global chord requires `ctrl` or `sup`, and ships its own cross-table claimant test against `KeyPattern::matches`. (§11)
- [ ] A new mode that captures the keyboard consumes every key with visible feedback — it does not return `Ignored` silently. (§11)
- [ ] New per-editing-pane state lives on `Document`, not on `App`, unless it is genuinely app-wide by nature. (§12)
- [ ] A new structural change that can un-paint the focused pane runs the one focus reconciler. (§12)
- [ ] A new source file stays under 500 lines, or its overrun is recorded in `TODO.md` with a reason and a split candidate. (§15)
- [ ] A new invariant claim ships as a test, not only as a comment. (§15)
