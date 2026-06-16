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
- **Merge** (Phase 2) — external/agent edits reconciled via the existing `pkg/merge`.

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
| External detection | per-file watch; **self-writes filtered** | 0007 |
| Merge | existing CGO `pkg/merge` (char-level 3-way), wired at `workspace.go:434` | — |
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
Then stamp recent events' `anchor_snapshot_id`, and **if the doc has a path, write the file to disk**, recording the written content hash so the watcher ignores the self-write (§5.5). Untitled drafts snapshot but skip the disk write; the chat prompt upserts its `drafts` row.

### 5.3 Undo / redo (⌘Z / ⌘⇧Z) — workspace-global
Handled at the workspace (routed before keys reach a surface). ⌘Z selects the last `is_undo_stop` event, inverts its `edits` on that event's `surface` (via textedit primitives), restores `cursors_before`/`focus_before`, and **pulls focus there**. ⌘⇧Z mirrors forward. A new edit after undo deletes events with higher `seq` (truncate — abandoned futures gone; their orphaned snapshots become GC-eligible).

### 5.4 Scrubber (time travel) — Phase 2
Reconstruct any event K: load `anchor_snapshot_id`'s blob, then replay `edits` for events in `(anchor, K]` on that surface. The scrubber is a new component rendering the journal as a linear timeline with snapshot markers.

### 5.5 External change + merge — Phase 2
Per-file watcher Write event → **filter self-writes** (compare to the last-written hash from §5.2) → read disk content → if it differs from the last snapshot, create a `source='external'` snapshot, then 3-way merge via `pkg/merge` (already wired at `workspace.go:434`). A clean merge writes a `source='merge'` snapshot (2 parents); conflicts produce conflict regions + an overlay. Whether the merge enters the ⌘Z journal is **deferred** (§12).

### 5.6 Crash recovery
The journal died with the session — nothing to replay. On open, reopen the last `snapshots` row per document (decompress its blob) and restore the chat `drafts` row. Loss is bounded by the unflushed live region (≤~2s idle, ≤~10s mid-burst), consistent with the `:memory:` degradation rung.

---

## 6. The seam — textedit ↔ workspace

There is no `editor` package. The surfaces are **`textedit`** (base) and **`markdownedit`** (the `main` editor), plus the **`title`** and **`chat input`** textedit instances. The workspace already forwards messages to each and polls `m.editor.Revision()` each frame — the journal seam extends that pull pattern (no new hot-path messages, §5.4 of CLAUDE.md).

### textedit — loses undo, gains a thin seam
Remove `history.UndoStack`, `applyUndo`/`applyRedo`, and the `history.undo`/`history.redo` commands. Keep buffer/cursors; add:

```go
// Observation (pull): what changed in the last Update, for the workspace to journal.
func (m Model) DrainEdits() ([]buffer.AppliedEdit, Model)   // applied edits since last drain
func (m Model) Cursors() []cursor.Cursor                    // current cursor/selection state

// Driving (workspace-owned undo/redo and scrubber):
func (m Model) ApplyInverse(edits []buffer.AppliedEdit) Model
func (m Model) SetCursors(cs []cursor.Cursor) Model
func (m Model) SetContent(s string) Model                   // exists; used for snapshot jumps
```

`buffer.AppliedEdit{Start, End int; Deleted, Insert string}` and `cursor.Cursor{Position, Anchor, DesiredCol, ID int}` are reused unchanged; inverse of an applied edit is `{Start, Start+len(Insert), Insert: Deleted}`.

### workspace — owns the store, journal, timers
- Constructs `*docstate.Store` (open ladder) and passes nothing storage-related into textedit.
- Routes ⌘Z/⌘⇧Z globally **before** forwarding keys; on other keys, forwards to the focused surface, then `DrainEdits()` + reads `Cursors()` and appends `events` tagged with `surface` and current focus.
- Owns the freeze-at-flush timer (`tea.Tick` debounce) → snapshot + autosave.
- **Removes** `isDirty`, `origContent` dirty-semantics, `MarkDirty`/`MarkClean`, the dirty indicator, and the `^C^C` quit-guard. **Keeps** `SaveIdentity`'s async write-tracking for autosave I/O completion/errors.
- Untitled handling: `CreateUntitled` creates a `documents` row with `path=''` (a **draft**); first non-empty content makes it dirty-eligible for snapshotting but **not** disk; removes the current auto-create-on-first-edit (`workspace.go:524`). ⌘S / naming assigns the path and writes the file.

---

## 7. Migration (big-bang)

| Area | Change |
|------|--------|
| `pkg/ui/components/textedit/textedit.go` | Remove `history.UndoStack` field, `applyUndo`/`applyRedo`, `editKindFromCommand`, undo/redo commands. Add the drain/drive seam. `applyOperation` becomes "apply to buffer + expose applied edits." |
| `history.*` coalesce logic | Moves into `docstate/journal.go` (the `is_undo_stop` rule). |
| `pkg/ui/pages/workspace/workspace.go` | Remove `isDirty`, `origContent` dirty-semantics, `MarkDirty`/`MarkClean`, dirty indicator, `^C^C` quit-guard. Add `*docstate.Store`, journal drain, snapshot/autosave timers, draft persistence, global ⌘Z routing. |
| `pkg/editor/history/` | **Deleted** entirely. |
| Tests | History/dirty/quit-guard tests deleted or rewritten against a `docstate` `:memory:` fixture (`docstate.NewTestStore()`). |

---

## 8. Package structure

```
pkg/docstate/                 -- workspace-owned persistence + journal
  store.go        -- open ladder; two connections; schema migration; NewTestStore()
  snapshot.go     -- blobs put/get (zstd, SHA-256); snapshots create/query; reconstruct(doc, seq)
  journal.go      -- :memory: events: append, coalesce/is_undo_stop, undo/redo, replay-to(K)
  draft.go        -- drafts upsert/restore (chat prompt)
  watcher.go      -- Phase 2: per-file fsnotify, self-write filter, ExternalChangeMsg
  merge.go        -- Phase 2: wraps pkg/merge (char-level 3-way), conflict regions
  identity.go     -- Phase 3: inode/device stat, rename detection
  compaction.go   -- Phase 3: snapshot GC (incl. abandoned), retention policy
```
No `operations.go`, no `revision.go` — undo is the journal, not a permanent graph.

---

## 9. Phases

### Phase 1 — persistence + journal + undo + autosave
**Build:** two-DB `docstate.Store` (open ladder, `NewTestStore`); `documents`/`blobs`/`snapshots`/`drafts` with content-addressed zstd; `:memory:` `events` journal with coalescing, **global ⌘Z/⌘⇧Z** (edit-granular, focus-follows, truncate-on-new-edit) and snapshot-jump reconstruction; freeze-at-flush snapshots; **autosave-to-disk** (self-write hash tracked); untitled **drafts**; remove `history.UndoStack`, dirty tracking, and the quit-guard; route undo at the workspace.

**Verify:**
- `go build ./...` clean (no `history` imports remain).
- Journal: undo/redo sequences; coalescing boundaries incl. **surface break**; truncate-on-new-edit.
- Snapshot: blob round-trip (zstd + SHA-256 dedup); reconstruct(doc, seq) == expected buffer.
- Autosave: writes the file; a self-write never triggers a merge; ⌘S on untitled assigns path + writes.
- Crash: kill mid-session → reopen → last snapshot restored; chat draft restored.
- All migrated editor tests pass against the `:memory:` fixture.

### Phase 2 — watcher + merge + scrubber
**Build:** per-file watcher with self-write filtering; external → `source='external'` snapshot; 3-way merge wrapping `pkg/merge`; conflict regions + overlay; the **scrubber** UI (walk journal + snapshots, preview/jump).

**Verify:** external edit during active editing merges correctly; self-writes never merge; clean merge fast-forwards; scrubber jump reconstructs exact state; resolve-conflict creates a new snapshot.

### Phase 3 — identity + compaction
**Build:** `inode`+`device` stat on open with rename detection (history follows the file; inode-reuse detected via content mismatch); snapshot **compaction**/GC incl. abandoned-branch snapshots with a retention policy; optional `transaction_id` for multi-file atomics; `UNIQUE(inode, device) WHERE inode IS NOT NULL`.

**Verify:** rename on disk → history preserved under new path; inode reuse → new document; compaction preserves reachable + merge snapshots; undo still works post-GC (snapshot-level fallback); compaction never deletes snapshots < retention age.

---

## 10. Key files

- `pkg/ui/components/textedit/textedit.go` — remove `history`; add drain/drive seam
- `pkg/ui/components/markdownedit/` — journaled as the `main` surface
- `pkg/ui/components/chat/chat.go` — prompt persisted as the `chat` draft (`chat.go:49`)
- `pkg/ui/pages/workspace/workspace.go` — owns `*docstate.Store`, journal, timers; removes dirty + quit-guard; `CreateUntitled` → draft
- `pkg/editor/history/` — **deleted**
- `pkg/merge/` — **reused** (CGO 3-way) in Phase 2
- `pkg/editor/buffer/`, `pkg/editor/cursor/` — unchanged; consumed by `docstate`

---

## 11. Dependencies

- `github.com/mattn/go-sqlite3` — SQLite driver (CGO; the app already requires CGO via `pkg/merge`, `pkg/microphone`, `pkg/inputlang`)
- `github.com/klauspost/compress/zstd` — blob compression
- `github.com/fsnotify/fsnotify` — already present (Phase 2 watcher)
- `pkg/merge` — already present (CGO char-level 3-way), Phase 2

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
