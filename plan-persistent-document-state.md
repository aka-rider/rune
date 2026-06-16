# Plan: Persistent Document State (`docstate`)

Persistent, crash-recoverable document history with **time-travel undo/redo** and **autosave**, backed by SQLite. Owned by the workspace; textedit stays a storage-free editing primitive.

This plan is the implementation spec. The load-bearing decisions are recorded as ADRs — `docs/adr/0004`–`0007` — and the vocabulary (journal / snapshot / scrubber / draft) is in `CONTEXT.md`. Editor test architecture is governed by `qa-instructions.md`.

---

## 1. Goal & shape

Replace textedit's in-memory `history.UndoStack` and the workspace's `origContent`/dirty/quit-guard with a `docstate` layer that gives:

- **Comfortable ⌘Z / ⌘⇧Z** — edit-granular undo across one global, whole-workspace timeline, restoring cursor/selection/focus context.
- **A scrubber** (Phase 2) — event-by-event time travel over the same timeline.
- **Autosave** — a titled document is written to disk continuously; there is no "unsaved changes" state.
- **Crash recovery** — reopen the last snapshot; ≤~2s of loss.
- **Document-aware guard** (Phase 2) — external/agent edits reconciled via the existing `pkg/merge` (ancestor = the last snapshot).

### Non-goals
- No keystroke-level undo *across restarts* (the journal is session-only; cross-restart history is snapshot-granular).
- No branch-aware undo, no LCA-for-undo, no DAG traversal for undo (undo is the journal, not the permanent graph).
- No vault-local database; no per-keystroke disk or permanent-DB writes.

---

## 2. Mental model — four layers of state

| Layer | Lives in | Lifetime | Role |
|-------|----------|----------|------|
| **Live buffer** | textedit (Go) | now | the editing surface |
| **Journal** (`events`) | `:memory:` SQLite, workspace-owned | session | undo/redo + scrubber timeline |
| **Snapshots** (`blobs`+`snapshots`) | permanent SQLite | forever | history, crash recovery, scrubber anchors |
| **Disk file** | filesystem | forever | the user's actual file (autosaved) |

The journal is written **continuously** (in memory, ~free). Snapshots and the disk file are written together on the **freeze-at-flush** cadence. Undo is the journal; the snapshots are only a coarse fallback and the scrubber's reconstruction anchors.

**Ownership (ADR-0004):** the **workspace** owns the `*docstate.Store`, the journal, and the flush/autosave timers. **textedit** owns the buffer and cursors and has *no undo of its own* — the workspace drives undo/redo through textedit primitives. The journal spans focus changes between panes, so it is inherently workspace-level.

---

## 3. Architecture decisions

| Decision | Choice | ADR |
|----------|--------|-----|
| Persistence owner | workspace — textedit has no undo | 0004 |
| Stores | in-memory **journal** + permanent **snapshots-only** DB | 0005 |
| Driver | `mattn/go-sqlite3` (the app already requires CGO) | 0005 |
| DB location | global `~/.local/share/rune/rune.db`; open-ladder → `:memory:`+warn → hard-fail | 0005 |
| Journal | one global linear timeline: edits + cursor + selection + focus | 0006 |
| Permanent store | content-addressed zstd snapshots + merge nodes only | 0005 |
| Undo | two-tier: global ⌘Z (edit-granular, focus-follows) + scrubber | 0006 |
| Abandoned futures | truncate (orphaned snapshots GC-eligible) | 0006 |
| Write timing | continuous journaling; freeze-at-flush snapshots + autosave | 0005 |
| Save model | autosave-to-disk; dirty flag + quit-guard retired | 0007 |
| ⌘S | force-flush + name-untitled (no longer "persist") | 0007 |
| Untitled | ephemeral recoverable **drafts** (synthetic id, disk on naming) | 0007 |
| Crash recovery | reopen last snapshot (no pending-ops replay) | 0005 |
| Identity | Phase 1 path-based; Phase 3 inode+device (global, survives rename) | 0005 |
| External detection | per-file watch; **self-writes filtered** (Phase 2 — document-aware guard) | 0007 |
| Merge | CGO `pkg/merge` (char-level 3-way); Phase 1 removes the old wiring, Phase 2 re-wires it document-aware (ancestor = last snapshot) | — |
| Migration | big-bang; remove `history.UndoStack` from textedit | 0004 |

---

## 4. SQLite schema

Two databases, both opened and owned by `docstate.Store`. Journal offsets are **byte** offsets (internal — `len()` is correct per CLAUDE.md §4.5); display code elsewhere still uses `utf8.RuneCountInString`.

### 4.1 Permanent — `~/.local/share/rune/rune.db` (snapshots only)

```sql
-- File identity. path='' for unsaved drafts; inode/device NULL until Phase 3.
CREATE TABLE documents (
  id           INTEGER PRIMARY KEY,
  path         TEXT NOT NULL DEFAULT '',
  inode        INTEGER,            -- NULL until Phase 3
  device       INTEGER,            -- NULL until Phase 3
  created_at   TEXT NOT NULL,
  last_seen_at TEXT NOT NULL
);
CREATE INDEX idx_documents_path ON documents(path) WHERE path != '';

-- Content-addressed blobs (dedup identical content across snapshots).
CREATE TABLE blobs (
  hash    TEXT PRIMARY KEY,        -- SHA-256 of uncompressed content
  content BLOB NOT NULL            -- zstd-compressed full document text
);

-- Snapshots: linear per document; parent_ids has >1 element ONLY for merges
-- (external/agent concurrency). NOT traversed for undo — that's the journal.
CREATE TABLE snapshots (
  id         INTEGER PRIMARY KEY,
  doc_id     INTEGER NOT NULL REFERENCES documents(id),
  blob_hash  TEXT NOT NULL REFERENCES blobs(hash),
  parent_ids TEXT,                 -- JSON array of snapshot ids (NULL for root)
  source     TEXT NOT NULL,        -- 'local' | 'external' | 'agent:<name>' | 'merge'
  created_at TEXT NOT NULL
);
CREATE INDEX idx_snapshots_doc ON snapshots(doc_id, id);

-- In-progress prompts persisted across restart (the chat draft).
CREATE TABLE drafts (
  surface    TEXT PRIMARY KEY,     -- 'chat' (extensible)
  content    TEXT NOT NULL,
  updated_at TEXT NOT NULL
);
```

### 4.2 In-memory journal — `:memory:` (the timeline)

```sql
-- One global, ordered timeline of every interaction across all surfaces.
-- Ephemeral: built at launch, dies with the session.
CREATE TABLE events (
  seq                INTEGER PRIMARY KEY AUTOINCREMENT,  -- global monotonic order
  surface            TEXT NOT NULL,   -- 'main' | 'title' | 'chat'
  kind               TEXT NOT NULL,   -- 'edit' | 'cursor' | 'selection' | 'focus'
  edits              BLOB,            -- kind='edit': JSON [{start,end,deleted,insert}]
  cursors_before     BLOB,            -- JSON [{position,anchor,desired_col,id}]
  cursors_after      BLOB,
  focus_before       TEXT,            -- kind='focus'
  focus_after        TEXT,
  is_undo_stop       INTEGER NOT NULL DEFAULT 0,  -- 1 = a ⌘Z lands here (edit boundary)
  anchor_snapshot_id INTEGER,         -- nearest permanent snapshot, for reconstruction
  at                 TEXT NOT NULL
);
CREATE INDEX idx_events_undo ON events(seq) WHERE is_undo_stop = 1;
```

### 4.3 Open ladder (`store.go`)

```
1. open ~/.local/share/rune/rune.db                  (existing)
2. mkdir -p ~/.local/share/rune  then create the db   (first run / missing dir)
3. open :memory:  + emit a non-blocking warning       ("history disabled — storage unavailable")
4. hard fail: "internal storage failure"
```
Per CLAUDE.md §1.3, rung 3 MUST surface the warning — never degrade silently. The `:memory:` journal DB is always opened regardless (it is ephemeral by design).

---

## 5. Core data flow

### 5.1 Normal editing
User types → buffer mutates in textedit → after `m.editor.Update(msg)` the workspace **drains** the applied edits + current cursors/selection and appends `events` rows (pull seam, §6). No permanent-DB or disk writes on the keystroke path. Coalescing sets `is_undo_stop`: an `edit` event starts a new stop unless it can merge with the previous one — **same `kind` · ≤300ms · same `surface` · prior insert non-whitespace** (ported from `history.ShouldCoalesce`). `cursor`/`selection`/`focus` events are never undo-stops.

### 5.2 Autosave + snapshot (freeze-at-flush)
Triggers: **~2s idle · focus-blur · tab-switch · close · quit · ~10s safety-net during unbroken typing.** On each:
```sql
BEGIN;
INSERT INTO blobs(hash, content) VALUES(?, ?) ON CONFLICT DO NOTHING;
INSERT INTO snapshots(doc_id, blob_hash, parent_ids, source, created_at)
  VALUES(?, ?, ?, 'local', ?);
COMMIT;
```
Then stamp recent events' `anchor_snapshot_id`, and **if the doc has a path, write the file to disk**. (Recording the written content hash for self-write filtering is a **Phase 2** concern — it exists only to serve the document-aware guard's watcher, §5.5; Phase 1 has no watcher, so autosave just writes.) Untitled drafts snapshot but skip the disk write; the chat prompt upserts its `drafts` row.

### 5.3 Undo / redo (⌘Z / ⌘⇧Z) — workspace-global
Handled at the workspace (routed before keys reach a surface). ⌘Z selects the last `is_undo_stop` event, inverts its `edits` on that event's `surface` (via textedit primitives), restores `cursors_before`/`focus_before`, and **pulls focus there**. ⌘⇧Z mirrors forward. A new edit after undo deletes events with higher `seq` (truncate — abandoned futures gone; their orphaned snapshots become GC-eligible).

### 5.4 Scrubber (time travel) — Phase 2
Reconstruct any event K: load `anchor_snapshot_id`'s blob, then replay `edits` for events in `(anchor, K]` on that surface. The scrubber is a new component rendering the journal as a linear timeline with snapshot markers.

### 5.5 The document-aware guard (external change + merge) — Phase 2
This whole path is **introduced in Phase 2** — Phase 1 removes the legacy footer merge guard, the per-file watcher, and the `merge.Merge` wiring (it cannot coexist with autosave; §6.3). Per-file watcher Write event → **filter self-writes** (compare to the last-written hash from §5.2) → read disk content → if it differs from the last snapshot, create a `source='external'` snapshot, then 3-way merge via `pkg/merge` using **the last snapshot as the ancestor** (no transient in-memory byte field). A clean merge writes a `source='merge'` snapshot (2 parents); conflicts produce conflict regions + an overlay. Whether the *incoming external content* enters the ⌘Z journal is **deferred** (§12) — but the local application of an accepted merge result is a normal journaled edit, undoable by ⌘Z (§6.3).

### 5.6 Crash recovery
The journal died with the session — nothing to replay. On open, reopen the last `snapshots` row per document (decompress its blob) and restore the chat `drafts` row. Loss is bounded by the unflushed live region (≤~2s idle, ≤~10s mid-burst), consistent with the `:memory:` degradation rung.

---

## 6. The seam — textedit ↔ workspace

There is no `editor` package. The journaled surfaces are **`markdownedit`** (the `main` editor, embeds `textedit.Model`), **`title`** (wraps a `textedit.Model` in `field`), and **`chat`** (wraps a `textedit.Model` in `prompt`; its read-only `display` markdownedit is **not** journaled). Today the workspace forwards messages to each and detects mutation by comparing `m.editor.Revision()` (in `recomputeDirty`); the journal seam **replaces** that revision poll with an explicit pull of the applied edits. No new hot-path messages (§5.4 of CLAUDE.md).

### 6.1 textedit — loses undo, gains a drain/drive seam

textedit becomes storage-free. Two classes of change:

**Removals** (see §7 for exact lines):
- `history history.UndoStack` field, `applyUndo`/`applyRedo`, `editKindFromCommand`, **and** `UndoForTest` (no test-only methods — undo is exercised through the real workspace ⌘Z path).
- The direct `key.Matches(msg, m.keys.Undo/Redo)` branches in `updateKeys` (228–243) and the `OperationHistory` arm of `applyOperation` (744–753).
- The `history.undo`/`history.redo` commands: delete `commands_history.go`, drop the `registerHistoryCommands` call in `commands_registry.go` (27–30), and drop the resolver bindings `add(b.Undo, "history.undo", …)` / `add(b.Redo, …)` in `keymap.go` `CommandBindings` (302–303). **Keep** the `Undo`/`Redo` key *definitions* in `Bindings` and their entries in `AllPhysicalKeys` (209–210) — the workspace now matches `m.keys.Undo`/`m.keys.Redo`.

**Additions** — a new accumulator field plus a thin seam:

```go
// New field on textedit.Model: applied edits not yet pulled by the workspace.
pendingEdits []buffer.AppliedEdit

// Observation (pull): return + CLEAR the accumulated edits since the last drain.
func (m Model) DrainEdits() (Model, []buffer.AppliedEdit)   // Model first, matches dict.TakePendingEdit
func (m Model) Cursors() []cursor.Cursor                    // current cursor/selection state

// Driving (workspace-owned undo/redo and scrubber) — NONE of these journal or
// accumulate into pendingEdits; they apply, bump rev, and syncDisplay:
func (m Model) ApplyInverse(edits []buffer.AppliedEdit) Model  // undo: invert + apply (math from old applyUndo)
func (m Model) Reapply(edits []buffer.AppliedEdit) Model       // redo: forward-apply (math from old applyRedo)
func (m Model) SetCursors(cs []cursor.Cursor) Model            // restore cursors/selection, then ScrollToCursor
func (m Model) SetContent(s string) Model                      // exists; snapshot jumps + file load; clears pendingEdits
```

**Accumulation rule (the pull seam's source):** every buffer-mutating path appends its `applied []buffer.AppliedEdit` to `pendingEdits` — `applyOperation` (after a successful `ApplyEdits`, replacing the `history.Push`/`MergeIntoLast`) and `ReplaceRange`/`AppendText`. `ApplyInverse`/`Reapply`/`SetContent` do **not** accumulate (undo/redo and loads are not new edits). `DrainEdits` returns and nils the slice.

`buffer.AppliedEdit{Start, End int; Deleted, Insert string}` and `cursor.Cursor{Position, Anchor, DesiredCol, ID int}` are reused unchanged. Inverse of an applied edit is `{Start, Start+len(Insert), Insert: Deleted}` (byte offsets — internal, `len()` correct per CLAUDE.md §4.5). The coalescing/`is_undo_stop` decision (ported from `history.ShouldCoalesce`) lives in `docstate/journal.go`, **not** textedit.

### 6.2 markdownedit / title / chat — forward the seam

textedit no longer self-undoes, so **every** journaled surface must forward the seam to its embedded/wrapped `textedit.Model`:

- **markdownedit** (embeds `textedit.Model`): `Cursors()` promotes through the embed unchanged. `DrainEdits`/`ApplyInverse`/`Reapply`/`SetCursors`/`SetContent` must be **shadowed** to return `markdownedit.Model` (the embedded versions return `textedit.Model`, the wrong type for `m.editor markdownedit.Model`). The buffer-mutating shadows (`ApplyInverse`, `Reapply`) must also run `afterContentChange` (image re-expansion) and therefore return `(Model, tea.Cmd)`, exactly like the existing `ReplaceRange` shadow (`markdownedit.go:204`). **Remove** the `UndoForTest` shadow (90–95). `ApplyMergeResult`/`ReplaceRange` already delegate to `m.Model.ReplaceRange`, so merge/programmatic edits flow into `pendingEdits` automatically — no extra wiring.
- **title** (`field textedit.Model`): add forwarders `DrainEdits`/`Cursors`/`ApplyInverse`/`Reapply`/`SetCursors` to `m.field`, returning `title.Model`. Title edits journal as `surface='title'`; the existing "modifiers reach textedit unmodified (undo, copy)" comment in `handleKey` is updated — title undo is now workspace-driven.
- **chat** (`prompt textedit.Model`): add the same five forwarders to `m.prompt` (`surface='chat'`), **plus** `PromptContent() string` and `SetPromptContent(string) Model` so the workspace can persist/restore the chat **draft** (§5.6). Only `prompt` is drained; the read-only `display` is never journaled.

### 6.3 workspace — owns the store, journal, timers

- **Store lifecycle.** Holds `store *docstate.Store` (a pointer — it owns the two `*sql.DB` connections, permitted by CLAUDE.md §1.1). `New()` stays pure; `Init()` returns the open-ladder Cmd (§4.3, I/O) which yields `StoreReadyMsg{Store}` or a degradation warning. Until the store is ready, journaling is a no-op (graceful). Tests inject a real store directly (§9).
- **Journal append (synchronous, on the main goroutine).** After **every** site where a journaled buffer mutates, the workspace pulls and appends: `m.<surface>, edits = m.<surface>.DrainEdits()`; read `Cursors()`; `m.store.AppendEvents(surface, edits, cursorsBefore, cursorsAfter, focus)`. The drain sites are: the key path after `editor`/`title`/`chat` `Update`; the dictation path after `editor.ReplaceRange`/`chat.ApplyToPrompt` (592/596); and the merge path after `editor.ApplyMergeResult` (440). The journal lives in `:memory:` — appends are in-process memory writes, **not** the file/network/timer I/O that §5.3 forbids, so they belong in `Update` (this is the "continuous, ~free" journaling of §2). `DrainEdits` returning empty replaces the old `Revision()`-diff guard.
- **Global undo/redo.** Match `m.keys.Undo`/`m.keys.Redo` **before** routing keys to the focused surface. `journal.Undo()`/`Redo()` selects the target `is_undo_stop` event, computes its inverse/forward `[]buffer.AppliedEdit`, and returns the target `surface` + `cursors_before`/`focus_before`. The workspace drives that surface via `ApplyInverse`/`Reapply` + `SetCursors`, pulls focus there, and batches any returned `tea.Cmd` (markdownedit image re-expansion).
- **Flush/autosave timer.** Owns a `tea.Tick` debounce → on trigger, a snapshot+autosave Cmd. The Cmd factory **captures the `store` pointer and the snapshot inputs (doc id, content, hash) as locals** (CLAUDE.md §5.5/§6.2) — it never reads `m.store` or other fields inside the closure. `*sql.DB` is goroutine-safe; the permanent-DB write runs off the main loop while journal appends stay on it.
- **Dirty/guard removal.** Delete `isDirty`, `recomputeDirty` (and its call sites), `lastRev`/`prevRev` plumbing, `MarkDirty`/`MarkClean` calls, the dirty indicator, the `pendingDirtyKind`/`pendingDirtyAction`/`pending` machinery, `dirtyGuardOptions`/`quitGuardOptions`, the dirty branches in `requestOpenPath`/`requestCloseCurrent` (switch/close just load/close — autosave guarantees no loss), the dirty arm of `ConfirmQuitMsg` (quit proceeds directly), and the pending-continuation block in `FileSavedMsg`. See §7 for the exact lines.
- **No guards in Phase 1 — `origContent` deleted outright, the merge path deferred to Phase 2.** `origContent` had two roles: the **dirty comparison** (retired by autosave) and the 3-way **merge ancestor** at `workspace.go:435`. Both consumers leave Phase 1, so the field is **removed entirely** — there is no `baseContent`. The whole external-change/merge path moves to **Phase 2's document-aware guard** (§5.5): the per-file watcher (`startFileWatch`/`stopFileWatch`, `watchedFilePath`, `cancelFileWatch`), `FileChangedOnDiskMsg`/`FileMergedMsg`, `mergeGuardOptions`/`pendingMergeContent`, the `DataLossMergeAccept`/`Reject` cases, and the `merge.Merge` call all go with it. **Why it cannot stay in Phase 1:** the old guard assumes the manual-save *dirty* model; under continuous autosave, with no self-write filter (a Phase 2 piece), every autosave would trip the watcher and pop a spurious guard. Phase 2 rebuilds it document-aware — watcher with self-write filter, external→`source='external'` snapshot, 3-way merge with the **last snapshot as ancestor** (no transient in-memory byte field), conflict overlay. The **directory** watcher (filetree refresh: `watchedDir`/`cancelWatch`) is unrelated and stays. Phase 1 therefore has **no guards at all** — the no-unsaved-state model means switch/close/quit never need one.
- **Untitled → draft.** `CreateUntitled` creates a `documents` row with `path=''` (a **draft**); first non-empty content makes it snapshot-eligible but **not** disk-eligible. The current auto-create-on-first-edit block (inside `recomputeDirty`, 525–532) is **deleted** with the rest of that function. ⌘S / naming assigns the path and writes the file.

---

## 7. Migration (big-bang)

Setup must run **first** (§11): `go get github.com/mattn/go-sqlite3` and `go get github.com/klauspost/compress/zstd` — neither is in `go.mod` yet, so any docstate source fails to build until they are added.

| File | Change |
|------|--------|
| `pkg/ui/components/textedit/textedit.go` | **Remove:** `history history.UndoStack` field (53); `applyUndo`/`applyRedo` (810–869); `editKindFromCommand` (890–907); `UndoForTest` (1013–1018); the `key.Matches(…Undo/Redo)` branches (228–243); the `OperationHistory` arm of `applyOperation` (744–753); the `history` import. **Add:** `pendingEdits []buffer.AppliedEdit` field; `DrainEdits`, `Cursors`, `ApplyInverse`, `Reapply`, `SetCursors` (§6.1). **Change:** `applyOperation` (770–803) and `ReplaceRange` (686–693) append `applied` to `pendingEdits` instead of `history.Push`/`MergeIntoLast`; `SetContent` nils `pendingEdits`. |
| `pkg/ui/components/textedit/commands_history.go` | **Deleted** entirely. |
| `pkg/ui/components/textedit/commands_registry.go` | Remove the `registerHistoryCommands` call (27–30). |
| `pkg/ui/keymap/keymap.go` | Remove the resolver bindings `add(b.Undo, "history.undo", …)` / `add(b.Redo, "history.redo", …)` (302–303). **Keep** `Undo`/`Redo` field definitions and `AllPhysicalKeys` entries (209–210) — the workspace matches them. |
| `pkg/ui/components/markdownedit/markdownedit.go` | Remove the `UndoForTest` shadow (90–95). Add shadows returning `markdownedit.Model`: `DrainEdits`, `SetCursors`; and `ApplyInverse`/`Reapply` returning `(Model, tea.Cmd)` (run `afterContentChange`, like `ReplaceRange` at 204). `Cursors` promotes via the embed. |
| `pkg/ui/components/title/title.go` | Add `DrainEdits`/`Cursors`/`ApplyInverse`/`Reapply`/`SetCursors` forwarders to `m.field`. Update the `handleKey` comment (199) — title undo is workspace-driven. |
| `pkg/ui/components/chat/chat.go` | Add the same five forwarders to `m.prompt`, plus `PromptContent()`/`SetPromptContent()` for draft persistence. `display` is never journaled. |
| `pkg/editor/history/` | **Deleted** entirely. Its coalesce logic (`ShouldCoalesce`, the whitespace/≤300ms/same-kind rule) moves into `docstate/journal.go` as the `is_undo_stop` decision. |
| `pkg/ui/pages/workspace/workspace.go` | **Remove (dirty/guard):** `isDirty` (193–195); `recomputeDirty` (516–537) and its call sites (758, 772, 1090); `lastRev` field (142) and the `prevRev` locals (581, 594); `MarkDirty`/`MarkClean` calls; `pendingDirtyKind`/consts (78–84), `pendingDirtyAction` (104–108), `pending` field (128); `dirtyGuardOptions`/`quitGuardOptions` (86–97); the `isDirty` branches in `requestOpenPath` (371–375)/`requestCloseCurrent` (391–399); the dirty arm of `ConfirmQuitMsg` (1046–1050, quit proceeds directly). **Remove (→ Phase 2 document-aware guard):** `origContent` entirely (193–195 + all set-sites); the whole `handleDataLossGuardResponse` (422–499); `mergeGuardOptions` (99–102), `pendingMergeContent` (146); `FileChangedOnDiskMsg`/`FileMergedMsg` handlers (863–877); the `merge.Merge`/`ApplyMergeResult` call (435/440); the per-file watcher (`startFileWatch`/`stopFileWatch` + call sites, `watchedFilePath`, `cancelFileWatch`, 148–150); the `FileSavedMsg` pending-continuation block (911–931). **Keep:** the **directory** watcher (`watchedDir`/`cancelWatch`) and the footer `^C^C` quit-confirm chord. **Add:** `store *docstate.Store`, `Init()` open-ladder Cmd + `StoreReadyMsg`, journal drain at every mutation site, freeze-at-flush snapshot/autosave timer, draft persistence, global ⌘Z/⌘⇧Z routing. **Change:** `CreateUntitled` (1463) → draft (`path=''`, no `origContent`, no disk). |
| Tests | No `XxxForTest` survives. Delete the dirty/quit-guard suites (`TestGate1/2/3/5/6/7/9/10/13`, `TestDirtyGuardDiscardLoadsNewFile`, `setEditorDirty`/`setOrigContent` helpers). Delete the `TestMergeGuard_*`, `TestFileWatch_*`, and `TestOrigContent_*` suites — that functionality is **Phase 2** and is rewritten there (incl. external-change-undo via ⌘Z). Undo coverage comes from new docstate tests (type → ⌘Z) that assert by **reading the DB** (§9). |

---

## 8. Package structure

```
pkg/docstate/                 -- workspace-owned persistence + journal
  store.go        -- open ladder; two connections; schema migration; NewTestStore() (temp-file perm + :memory: journal); read helpers for tests
  snapshot.go     -- blobs put/get (zstd, SHA-256); snapshots create/query; reconstruct(doc, seq)
  journal.go      -- :memory: events: append; coalesce/is_undo_stop (ported from history.ShouldCoalesce); undo/redo edit construction (inverse/forward, from old applyUndo/applyRedo); replay-to(K)
  draft.go        -- drafts upsert/restore (chat prompt)
  watcher.go      -- Phase 2: per-file fsnotify, self-write filter, ExternalChangeMsg
  merge.go        -- Phase 2: wraps pkg/merge (char-level 3-way), conflict regions
  identity.go     -- Phase 3: inode/device stat, rename detection
  compaction.go   -- Phase 3: snapshot GC (incl. abandoned), retention policy
```
No `operations.go`, no `revision.go` — undo is the journal, not a permanent graph.

---

## 9. Test strategy & phases (Canon TDD)

### 9.1 Approach — data integrity is the acceptance criterion

This feature's whole purpose is to **not lose the user's work**, so persistence/integrity is the riskiest surface and gets tested first and hardest (CLAUDE.md §1.3: data loss is intolerable). Follow Canon TDD:

1. **Write the list** (§9.2) — concrete behaviors, not implementation. The list is living; add to it as the design surfaces edge cases.
2. **Pick one**, write *one* runnable, failing test that names the behavior.
3. **Make it pass** with the least code.
4. **Refactor** under green.
5. Repeat, draining the P0 (data-integrity) rows before P1/P2.

**Verify by reading the DB, not by poking fields.** A test drives behavior through the *real* public path (send `tea.KeyPressMsg` through `workspace.Update`; fire the flush trigger) and then asserts on actual persisted state — `SELECT`ing `blobs`/`snapshots`/`events`/`drafts` rows and reading the file back off disk. This proves the persistence path end-to-end; white-box field assertions cannot. **No `XxxForTest` methods exist** — undo is exercised by the same ⌘Z keypress the user presses; persistence by the same flush the timer fires.

**`docstate.NewTestStore(t *testing.T) *Store`:** opens a **real** store — journal in `:memory:`, permanent DB in `t.TempDir()` (a *file*, so a second open in a crash-recovery test sees committed rows; `:memory:` dies with its connection and cannot test reopen). It runs schema migration and accepts an injectable deterministic `Clock` (qa-instructions: no `time.Sleep`) for coalescing-window and timer tests. The workspace test fixture assigns it directly (`m.store = docstate.NewTestStore(t)`) — a real object on a real field, not a fake method. Tests needing read-back use the store's query helpers (or a second read-only connection to the temp-file DB).

### 9.2 Test list (P0 = cannot ship without)

**P0 — integrity & persistence**
- Blob round-trip is lossless for ASCII, CJK, emoji, and large (>1 MB) docs (zstd + SHA-256); identical content dedups to **one** `blobs` row.
- Flush writes exactly one `snapshots` row + its `blobs` row with correct `doc_id`/`blob_hash`/`source='local'`; `reconstruct(doc, latest)` equals the live buffer byte-for-byte.
- Autosave writes the **file**: after a flush, the on-disk bytes equal the flush's snapshot blob — both come from the *same* frozen content, so there is no live-buffer race (the freeze-at-flush form of the D13 invariant).
- **Crash recovery:** apply N edits → flush → `store.Close()` → reopen the temp-file DB → latest snapshot decompresses to the last-flushed content; the chat `drafts` row restores the prompt via `SetPromptContent`.
- Loss bound: the only acceptable loss is edits after the last flush; firing the flush trigger captures everything up to it.
- **Open-ladder degradation never silent:** missing dir → created; unwritable → `:memory:` + a surfaced warning (assert the warning message), app still usable.

**P1 — undo/redo correctness (through the real ⌘Z path)**
- Type, then `m.keys.Undo` keypress → buffer + `cursors_before` + focus restored; `m.keys.Redo` reapplies.
- Coalescing: same-kind ≤300 ms non-whitespace edits merge into one undo-stop; whitespace / >300 ms / **surface break** start a new stop — assert the `events.is_undo_stop` rows directly.
- Truncate-on-new-edit: undo then type deletes higher-`seq` events (assert row count); orphaned snapshots become GC-eligible.
- Focus-follows: ⌘Z targeting a `surface='title'` event while `main` is focused pulls focus to the title.

**P2 — surface & routing regressions (existing CLAUDE.md §8.1 classes)**
- Render purity; key routing (undo matched globally **before** the focused surface; unfocused surfaces ignore keys).
- `DrainEdits` returns empty (no journal row) when an `Update` made no buffer mutation.
- All surviving migrated editor tests pass against `NewTestStore`.

### Phase 1 — persistence + journal + undo + autosave
**Build:** two-DB `docstate.Store` (open ladder, `NewTestStore`); `documents`/`blobs`/`snapshots`/`drafts` with content-addressed zstd; `:memory:` `events` journal with coalescing, **global ⌘Z/⌘⇧Z** (edit-granular, focus-follows, truncate-on-new-edit) and snapshot-jump reconstruction; freeze-at-flush snapshots; **autosave-to-disk**; untitled **drafts**; remove `history.UndoStack`, dirty tracking, the quit-guard, **and the entire external-change path** (per-file watcher + merge guard + `origContent` → Phase 2); route undo at the workspace. Phase 1 is **local-only — no per-file watcher, no merge, no guard.** **Build the P0 rows first** (§9.2), then P1, then migrate the surfaces (P2).

**Exit criteria:** `go build ./...` clean (no `history` imports remain) and every P0+P1 row green.

### Phase 2 — document-aware guard + scrubber
**Build:** per-file watcher with self-write filtering; external → `source='external'` snapshot; 3-way merge wrapping `pkg/merge` with **the last snapshot as ancestor** (no `origContent`); conflict regions + overlay; the **scrubber** UI (walk journal + snapshots, preview/jump). This re-introduces, document-aware, the reconciliation removed in Phase 1.

**Verify:** external edit during active editing merges correctly; self-writes never merge; clean merge fast-forwards; accepting a merge applies a **local**, ⌘Z-undoable edit (§6.3) while the incoming snapshot stays out of the journal (§12); scrubber jump reconstructs exact state; resolve-conflict creates a new snapshot; the rewritten merge/watch suites pass.

### Phase 3 — identity + compaction
**Build:** `inode`+`device` stat on open with rename detection (history follows the file; inode-reuse detected via content mismatch); snapshot **compaction**/GC incl. abandoned-branch snapshots with a retention policy; optional `transaction_id` for multi-file atomics; `UNIQUE(inode, device) WHERE inode IS NOT NULL`.

**Verify:** rename on disk → history preserved under new path; inode reuse → new document; compaction preserves reachable + merge snapshots; undo still works post-GC (snapshot-level fallback); compaction never deletes snapshots < retention age.

---

## 10. Key files

- `pkg/ui/components/textedit/textedit.go` — remove `history`+undo+`UndoForTest`; add `pendingEdits` + drain/drive seam (§6.1)
- `pkg/ui/components/textedit/commands_history.go` — **deleted**; `commands_registry.go` — drop the call
- `pkg/ui/keymap/keymap.go` — drop the `history.undo`/`history.redo` resolver bindings (302–303); keep the key defs
- `pkg/ui/components/markdownedit/markdownedit.go` — `main` surface; remove `UndoForTest` shadow, add seam shadows (return `markdownedit.Model`; `ApplyInverse`/`Reapply` run `afterContentChange`)
- `pkg/ui/components/title/title.go` — `title` surface; forward the seam to `m.field`
- `pkg/ui/components/chat/chat.go` — `chat` surface; forward the seam to `m.prompt` (`chat.go:49`) + `PromptContent`/`SetPromptContent` for the draft
- `pkg/ui/pages/workspace/workspace.go` — owns `*docstate.Store`, journal, timers; removes dirty + quit-guard **and** the external-change/merge/watcher path + `origContent` (→ Phase 2, `:435`); `CreateUntitled` → draft
- `pkg/editor/history/` — **deleted** (coalesce rule → `docstate/journal.go`)
- `pkg/merge/` — **reused** (CGO 3-way) in Phase 2
- `pkg/editor/buffer/`, `pkg/editor/cursor/` — unchanged; consumed by `docstate`

---

## 11. Dependencies

**Setup step 0 — run before writing any `docstate` source** (both are absent from `go.mod`; building docstate fails until added):

```
go get github.com/mattn/go-sqlite3
go get github.com/klauspost/compress/zstd
```

(In the sandbox, Go commands need `GOPATH`/`GOCACHE`/`GOFLAGS=-buildvcs=false`/`TMPDIR` set per the project's Go-env note; `go get` needs network, so run it sandbox-disabled if blocked.)

- `github.com/mattn/go-sqlite3` — **new.** SQLite driver (CGO; the app already requires CGO via `pkg/merge`, `pkg/microphone`, `pkg/inputlang`).
- `github.com/klauspost/compress/zstd` — **new.** Blob compression.
- `github.com/fsnotify/fsnotify` — already in `go.mod` (v1.10.1); Phase 2 watcher.
- `pkg/merge` — already present (CGO char-level 3-way), Phase 2.

---

## 12. Deferred / open

- **External/agent edits vs the ⌘Z timeline** (Phase 2): does ⌘Z ever undo an agent's or an incoming external edit? Current lean: **no** — they are `snapshots` outside the ⌘Z journal, surfaced only as scrubber markers (preserves the spirit of the retired branch-aware undo).
- **⌘S as a named checkpoint** (alternative to force-flush+name): if "bookmark this version for the scrubber" is wanted later, add a labeled-snapshot feature; intentionally deferred to keep Phase 1 simple.
- **Cross-machine history**: a global DB keyed by physical identity follows a file across renames but does **not** travel with the vault to another machine — accepted (ADR-0005).

---

## 13. References

- ADRs: `docs/adr/0004` (workspace owns docstate) · `0005` (two stores) · `0006` (two-tier global undo) · `0007` (autosave retires dirty)
- Vocabulary: `CONTEXT.md` — *journal · snapshot · scrubber · draft*
- Test architecture: `qa-instructions.md` (deterministic `Clock`, no `time.Sleep`, `:memory:` fixtures)
