# Plan: Persistent Document State Management (docstate) — v2 (Post-Critic)

## TL;DR

Replace the in-memory linear undo stack with a Git-like revision DAG backed by SQLite. Content-addressed compressed blobs store snapshots; operations persist for keystroke-level undo across restarts. External file changes detected via fsnotify, merged with character-level 3-way merge. Branch-aware undo lets users walk their own edit history independent of external/agent changes.

---

## Critic Fixes Applied

1. **Ops schema split**: Operations now have `base_revision_id` (where they start from) and `committed_revision_id` (nullable — NULL = pending/uncommitted, set when flush creates revision). Exact queries defined for both crash recovery and undo.
2. **Identity in Phase 1**: `documents` table uses nullable `inode`/`device` (no UNIQUE constraint until Phase 3). Phase 1 uses path-based identity with synthetic doc IDs. Untitled docs have `inode=NULL, device=NULL, path=''`.
3. **Cursor state in API**: `ApplyEdit` accepts both `cursorsBefore` and `cursorsAfter`. Operations table stores both.
4. **Save boundary**: Added `documents.saved_revision_id` column + `revisions.is_save` flag. `StartSave`/`FileSavedMsg` update `saved_revision_id` only on matching request ID. `IsDirty()` = HEAD revision differs from `saved_revision_id` OR uncommitted ops exist.
5. **History package migration scope**: Full enumeration of all `history.*` call sites. `EditKind` constants move to `pkg/docstate`. All tests that set `m.history` directly are rewritten.
6. **Flush wiring**: Editor exposes exported `FlushDocument() (Model, tea.Cmd)` method. Workspace calls this. Editor owns an internal idle timer via `tea.Tick`-based debounce command.

---

## Architecture Decisions

| Decision | Choice |
|----------|--------|
| History model | DAG with merges |
| Undo semantics | Branch-aware (undo user's edits only) |
| In-memory buffer | Keep flat string (`buffer.Buffer`) |
| Snapshot storage | Content-addressed blobs, SHA-256 keyed |
| Blob compression | zstd |
| Operation persistence | Yes — for keystroke undo across restarts |
| Write timing | Ops every ~300ms (coalesced group); blob on 2s idle |
| SQLite location | `.rune/state.db` in vault root |
| File identity | Phase 1: path-based. Phase 3: inode + device + content_hash |
| External detection | Hybrid: recursive dir watch + per-file content watch |
| Merge algorithm | Character-level 3-way merge (existing Go lib) |
| Conflict UX | Markers in buffer + UI overlay (accept/reject) |
| Multi-file atomics | Explicit `transaction_id` (nullable) |
| GC strategy | Time-based compaction (collapse minor local revisions) |
| Source tagging | `local` / `external` / `merge` / `agent:<name>` |
| Package location | New `pkg/docstate/` |
| Migration | Big bang replacement |
| Phasing | 3 phases: persist → merge → identity |

---

## SQLite Schema (v2)

```sql
-- File identity
CREATE TABLE documents (
  id INTEGER PRIMARY KEY,
  inode INTEGER,              -- nullable until Phase 3 (NULL for untitled)
  device INTEGER,             -- nullable until Phase 3
  path TEXT NOT NULL DEFAULT '',
  content_hash TEXT,          -- SHA-256 of last-known content (similarity heuristic)
  saved_revision_id INTEGER,  -- points to last revision written to disk
  created_at TEXT NOT NULL,
  last_seen_at TEXT NOT NULL
);
CREATE INDEX idx_documents_path ON documents(path) WHERE path != '';

-- Content-addressed blob storage
CREATE TABLE blobs (
  hash TEXT PRIMARY KEY,       -- SHA-256 of uncompressed content
  content BLOB NOT NULL        -- zstd compressed full document text
);

-- Revision DAG
CREATE TABLE revisions (
  id INTEGER PRIMARY KEY,
  doc_id INTEGER NOT NULL REFERENCES documents(id),
  blob_hash TEXT NOT NULL REFERENCES blobs(hash),
  parent_ids TEXT,             -- JSON array of parent revision IDs (NULL for root)
  timestamp TEXT NOT NULL,     -- ISO8601
  source TEXT NOT NULL,        -- 'local' | 'external' | 'merge' | 'agent:<name>'
  minor INTEGER NOT NULL DEFAULT 1,
  is_save INTEGER NOT NULL DEFAULT 0,  -- 1 if this revision was written to disk
  transaction_id TEXT,         -- nullable; groups multi-file atomic ops
  session_id TEXT              -- app session identifier
);
CREATE INDEX idx_revisions_doc ON revisions(doc_id, timestamp);
CREATE INDEX idx_revisions_txn ON revisions(transaction_id) WHERE transaction_id IS NOT NULL;

-- Fine-grained operations (keystroke-level undo across restarts)
CREATE TABLE operations (
  id INTEGER PRIMARY KEY,
  doc_id INTEGER NOT NULL REFERENCES documents(id),
  base_revision_id INTEGER NOT NULL REFERENCES revisions(id),  -- ops start from this snapshot
  committed_revision_id INTEGER REFERENCES revisions(id),      -- NULL = pending; set on flush
  edits BLOB NOT NULL,         -- JSON: [{start, end, insert, deleted}]
  cursors_before BLOB NOT NULL,-- JSON: [{position, anchor, id}]
  cursors_after BLOB NOT NULL, -- JSON: [{position, anchor, id}]
  timestamp TEXT NOT NULL,
  seq INTEGER NOT NULL,        -- ordering within the group (ascending)
  kind INTEGER NOT NULL        -- EditKind enum value
);
CREATE INDEX idx_ops_pending ON operations(doc_id, base_revision_id, seq)
  WHERE committed_revision_id IS NULL;
CREATE INDEX idx_ops_committed ON operations(doc_id, committed_revision_id, seq)
  WHERE committed_revision_id IS NOT NULL;
```

### Key Queries

**Crash recovery** (find uncommitted ops after last known revision):
```sql
SELECT * FROM operations
WHERE doc_id = ? AND committed_revision_id IS NULL
ORDER BY seq ASC;
```
Then: load blob from `base_revision_id`, replay ops in seq order → restored buffer.

**Undo within current pending ops** (before flush):
```sql
SELECT * FROM operations
WHERE doc_id = ? AND committed_revision_id IS NULL AND seq = (
  SELECT MAX(seq) FROM operations WHERE doc_id = ? AND committed_revision_id IS NULL
);
```
Delete that row, apply inverse to buffer.

**Undo within a committed revision** (after flush, walking DAG):
```sql
SELECT * FROM operations
WHERE doc_id = ? AND committed_revision_id = ?
ORDER BY seq DESC LIMIT 1;
```

**Flush** (commit pending ops to a new revision):
```sql
BEGIN;
INSERT INTO blobs (hash, content) VALUES (?, ?) ON CONFLICT DO NOTHING;
INSERT INTO revisions (doc_id, blob_hash, parent_ids, timestamp, source, minor) VALUES (...);
UPDATE operations SET committed_revision_id = ? WHERE doc_id = ? AND committed_revision_id IS NULL;
COMMIT;
```

**IsDirty**:
```sql
-- Dirty if: uncommitted ops exist OR head revision != saved_revision_id
SELECT EXISTS(
  SELECT 1 FROM operations WHERE doc_id = ? AND committed_revision_id IS NULL
) OR (
  SELECT COALESCE(
    (SELECT id FROM revisions WHERE doc_id = ? ORDER BY id DESC LIMIT 1), -1
  ) != COALESCE(saved_revision_id, -2)
  FROM documents WHERE id = ?
);
```

---

## Core Data Flow

### Normal Editing
1. User types → `buffer.ApplyEdits()` → operation row INSERT with `committed_revision_id = NULL`, `base_revision_id = current HEAD revision`
2. Coalesce logic: if `ShouldCoalesce()`, UPDATE last pending op row (append edits JSON, update cursors_after)
3. User pauses (2s idle) → flush: serialize buffer → blob → new revision → UPDATE all pending ops: `SET committed_revision_id = new_revision_id`

### Crash Recovery
1. On startup: for each doc with pending ops (`committed_revision_id IS NULL`):
2. Load blob from `base_revision_id` → decompress → reconstruct buffer
3. Replay all pending ops (ascending seq) → buffer = exact pre-crash state
4. Resume editing; next flush creates the revision that was interrupted

### External Change Detection
1. Per-file fsnotify detects Write event on open file
2. Read new disk content → SHA-256 hash → compare to last-known revision's blob_hash
3. If different: create `external` revision (new blob + revision row with `source='external'`, parent = last disk-synced revision)
4. Trigger merge

### Merge
1. Find LCA (Lowest Common Ancestor) of local HEAD and new external revision in the DAG
2. Decompress 3 blobs: LCA, local HEAD, external
3. Run character-level 3-way merge
4. If clean: create `merge` revision (2 parents: local HEAD + external, source='merge')
5. If conflicts: insert conflict markers, create merge revision, mark as conflicted. UI overlay shows accept/reject per hunk.

### Branch-Aware Undo
1. **Pending ops exist**: delete last pending op row, apply its inverse, restore `cursors_before`
2. **No pending ops, at a committed revision**: load ops for HEAD revision (committed_revision_id = HEAD), walk from highest seq down. Each undo deletes an op row and applies inverse.
3. **Revision ops exhausted**: follow first parent (local branch convention) to previous revision WHERE `source = 'local'`. Load that revision's blob as new buffer state. Load that revision's ops for further fine-grained undo.
4. **Skip non-local**: when traversing DAG, skip `external`/`merge`/`agent:*` revisions.
5. **Ops GC'd (post-compaction)**: fallback to snapshot-level jump (restore blob content directly).

### Redo
- Mirror of undo: walk forward through ops/revisions, follow forward edges in DAG for local branch.

### Save Flow
1. `StartSave()` → records `activeSave.RequestID` + `ContentHash` (unchanged from current)
2. On `FileSavedMsg` with matching request ID:
   - Force flush (if pending ops exist → create revision)
   - Mark HEAD revision: `UPDATE revisions SET is_save = 1 WHERE id = HEAD`
   - Update document: `UPDATE documents SET saved_revision_id = HEAD WHERE id = doc_id`
   - `IsDirty()` now returns false (HEAD = saved_revision_id, no pending ops)

---

## Package Structure

```
pkg/docstate/
  store.go          -- SQLite connection, schema migration, blob put/get
  document.go       -- Document struct (in-memory state: buffer + DAG position + pending ops)
  revision.go       -- Revision, DAG traversal, LCA algorithm
  operations.go     -- Operation persistence, replay, inverse, coalesce
  merge.go          -- 3-way character merge, conflict detection
  watcher.go        -- Per-file fsnotify, change detection
  compaction.go     -- Time-based GC of minor revisions
  identity.go       -- Inode/device lookup, rename detection
  editkind.go       -- EditKind enum (migrated from history package)
```

---

## Integration with Editor Component

The editor component (`pkg/ui/components/editor/`) currently owns:
- `buf buffer.Buffer` — stays (flat string, fast for notes)
- `history history.UndoStack` — **replaced** by `docstate.Document`
- `dirty bool` + `savedContentHash` — **replaced** by `doc.IsDirty()` + `doc.MarkSaved()`
- `activeSave SaveIdentity` — stays on editor (in-flight save tracking logic unchanged)
- `filePath string` — stays on editor as display cache; `doc.Path()` is source of truth

New editor Model fields:
```go
type Model struct {
    buf      buffer.Buffer          // still the in-memory editing buffer
    doc      docstate.Document      // replaces history + dirty tracking
    // activeSave stays — unchanged save-in-flight logic
    // filePath stays — cached from doc.Path() for breadcrumb/title
    // ... rest unchanged
}
```

### `docstate.Document` API (value type, per CLAUDE.md §5.1)

```go
// EditKind lives in pkg/docstate (moved from history package)
type EditKind int
const (
    EditInsertChar EditKind = iota
    EditDeleteChar
    EditPaste
    EditNewline
    EditMoveLine
    EditCloneLine
    EditBatch
)

type Document struct { /* unexported fields: store ref, doc_id, head_revision_id, pending ops cache */ }

// Core editing
func (d Document) ApplyEdit(edits []buffer.AppliedEdit, cursorsBefore []cursor.Cursor, cursorsAfter []cursor.Cursor, kind EditKind, now time.Time) (Document, error)
func (d Document) ShouldCoalesce(kind EditKind, now time.Time) bool

// Undo/Redo — returns new doc state + buffer content + cursors to restore
func (d Document) Undo(currentBuf buffer.Buffer) (Document, buffer.Buffer, []cursor.Cursor, bool)
func (d Document) Redo(currentBuf buffer.Buffer) (Document, buffer.Buffer, []cursor.Cursor, bool)
func (d Document) CanUndo() bool
func (d Document) CanRedo() bool

// Persistence
func (d Document) Flush(currentBuf buffer.Buffer) (Document, error)  // create revision from pending ops
func (d Document) MarkSaved(revisionID int64) (Document, error)      // set saved_revision_id

// State queries
func (d Document) IsDirty() bool         // pending ops exist OR head != saved_revision_id
func (d Document) Path() string
func (d Document) HeadRevisionID() int64

// Lifecycle
func (d Document) Close() error           // flush + cleanup
```

### Editor Exported Methods (for workspace)

```go
// FlushDocument persists pending operations as a snapshot revision.
// Called by workspace on idle timer or before quit.
func (m Model) FlushDocument() (Model, tea.Cmd)

// IdleFlushCmd returns a debounced tea.Cmd that sends flushTickMsg after 2s.
// Editor resets this timer on each edit.
func (m Model) scheduleIdleFlush() tea.Cmd
```

The editor internally handles a `flushTickMsg` in its Update:
```go
case flushTickMsg:
    if m.doc.HasPendingOps() && time.Since(m.lastEditTime) >= 2*time.Second {
        var err error
        m.doc, err = m.doc.Flush(m.buf)
        // handle err
    }
```

---

## Migration: All history.* Call Sites

Files that import/use `history` package (must be updated):

| File | Usage | Migration |
|------|-------|-----------|
| `editor/apply.go` | `applyOperation(op, history.EditKind, now)`, `history.EditGroup`, coalesce, `applyUndo`, `applyRedo`, `editKindFromCommand` | Rewrite to use `doc.ApplyEdit(...)` / `doc.Undo(buf)` / `doc.Redo(buf)`. `editKindFromCommand` returns `docstate.EditKind` |
| `editor/editor.go` | `history history.UndoStack` field, `history.New(time.Now)` in constructor, direct `m.history.CanUndo()` in insert-char handler | Replace field with `doc docstate.Document`. Constructor takes `*docstate.Store` param. |
| `editor/commands_clipboard.go` | `m.applyOperation(op, history.EditPaste, now)` | Changes to `docstate.EditPaste` — just constant rename |
| `editor/commands_image.go` | `m.applyOperation(op, history.EditPaste, now)` | Same — constant rename |
| `editor/dictation.go` (×2) | `m.applyOperation(op, history.EditPaste, time.Now())` | Same — constant rename |
| `editor/editor.go:448` | `m.applyOperation(res.Operation, history.EditInsertChar, time.Now())` | Same — constant rename |
| `editor/commands_multi_test.go` | `m.history = history.New(...)` | Use test helper that creates doc with in-memory store |
| `editor/commands_history_test.go` | `m.history = history.New(clk.Now)`, `m.history.CanUndo()` | Rewrite tests to use docstate test fixture |
| `editor/editor_test.go` (×3) | `m.history = history.New(...)` | Rewrite tests to use docstate test fixture |
| `editor/commands_edit_test.go` | `m.history = history.New(...)` | Rewrite tests to use docstate test fixture |
| `pkg/editor/history/history.go` | Package definition | Delete after all above migrated |
| `pkg/editor/history/history_test.go` | Unit tests for old stack | Delete (logic moves to docstate tests) |

**Test migration strategy**: Create `docstate/testutil.go` with:
```go
func NewTestDocument(clock func() time.Time) Document  // in-memory SQLite, no disk
```
Editor tests use this instead of `history.New(...)`.

---

## Phase 1: Persistence + Crash Recovery + Undo + Save

**Scope**: SQLite store, blob management, revision DAG, operation persistence, branch-aware undo/redo, crash recovery, save boundary, editor integration, full history package migration.

**Steps**:
1. Create `pkg/docstate/` package with SQLite schema + migrations (`modernc.org/sqlite`, pure-Go)
2. `editkind.go`: Define `EditKind` constants (moved from `history` package)
3. `store.go`: SQLite connection pool, schema creation, blob put/get (zstd via `klauspost/compress/zstd`)
4. `revision.go`: Create revision, query HEAD for doc, find LCA (BFS), parent traversal
5. `operations.go`: Write pending op, commit pending ops (UPDATE `committed_revision_id`), read ops for replay, read ops for undo, inverse computation, coalesce logic (moved from history)
6. `document.go`: `Document` value type with full API. Constructor accepts `*Store` + doc_id. Opens doc from latest revision + replays pending ops (crash recovery built-in to Open).
7. `identity.go` (Phase 1 minimal): `OpenByPath(path) Document` — lookup doc by path, create if not found. For untitled: `CreateUntitled() Document`. Inode/device columns left NULL.
8. `store.go` addition: `NewTestStore()` returning in-memory SQLite for tests.
9. **Editor migration**: Replace `history` field with `doc` field. Rewrite `applyOperation` → call `doc.ApplyEdit`. Rewrite `applyUndo`/`applyRedo` → call `doc.Undo(buf)`/`doc.Redo(buf)`. Add `FlushDocument()` exported method. Add `flushTickMsg` handler + idle timer scheduling.
10. **Save boundary**: On `FileSavedMsg` match: call `doc.Flush()` if pending ops, then `doc.MarkSaved(head)`. `IsDirty()` delegates to `doc.IsDirty()`.
11. **All test migrations**: Update every test file that imports `history` (see table above). Create `docstate/testutil.go`.
12. **Workspace wiring**: Pass `*docstate.Store` through app → workspace → editor constructor. On quit: call `m.editor.FlushDocument()`. Create `.rune/` directory on first vault open.
13. **Delete** `pkg/editor/history/` package entirely.

**Verification**:
- `go build ./...` compiles clean (no history imports remain)
- Unit tests: blob round-trip, revision DAG traversal, LCA, undo/redo sequences, coalesce behavior
- Crash recovery test: write ops without flush, re-open doc, verify buffer matches pre-crash
- Save boundary test: edit → save → undo past save → `IsDirty()` = true; redo to save → `IsDirty()` = false
- Integration test: full edit → quit → reopen → verify state preserved
- All existing editor tests pass (rewritten to use docstate fixtures)
- `go test ./pkg/docstate/... ./pkg/ui/components/editor/...` passes

## Phase 2: Watcher + Merge

**Scope**: Per-file fsnotify, external change detection, 3-way character merge, conflict markers, basic UI overlay.

**Steps**:
1. `watcher.go`: Per-file watcher (fsnotify Write events + 100ms debounce). Emits `ExternalChangeMsg{DocID, NewContent}` as a tea.Msg.
2. On change detected: hash compare → if different, create `external` revision via `doc.RecordExternal(content)`
3. Integrate character-level 3-way merge Go library (research: `github.com/sergi/go-diff` + custom diff3, or `github.com/hexops/gotextdiff`)
4. `merge.go`: LCA lookup → decompress 3 blobs → character-level diff3 → produce merged text + conflict regions
5. `document.go` addition: `MergeExternal(diskContent []byte) (Document, buffer.Buffer, []ConflictRegion, error)`
6. Editor: handle `ExternalChangeMsg` → call `doc.MergeExternal` → update buffer → if conflicts, set conflict state
7. Conflict marker insertion: `<<<<<<<` / `=======` / `>>>>>>>` with source labels
8. Editor Model: add `conflicts []ConflictRegion` field (start/end byte offsets + local/remote text)
9. Render UI overlay: highlight conflict regions in View(), show accept-local/accept-remote/accept-both keybindings when cursor is in a conflict region
10. On conflict resolution: remove markers from buffer, clear conflict entry, create new local revision

**Verification**:
- Unit tests: merge clean changes (no overlap), merge overlapping → conflict markers correct
- Integration test: modify file externally while editor has unsaved edits → verify merge result
- Test: resolve conflict → markers removed, new revision created
- Test: merge with no local changes → fast-forward (just update HEAD, no merge node)

## Phase 3: Identity + Transactions + Compaction

**Scope**: Inode-based file identity, rename handling, transaction_id grouping, multi-file undo, time-based compaction.

**Steps**:
1. `identity.go` (full): On file open, stat file → populate `inode`/`device` on documents row
2. Lookup logic: check (inode, device) first → if found, update path if changed (rename detected). If not found → fall back to path lookup → if found, verify content_hash matches (detect inode reuse vs genuine new file).
3. Add `UNIQUE(inode, device) WHERE inode IS NOT NULL` partial index
4. `document.go` addition: `BeginTransaction() string` returns UUID. Attach to subsequent revisions.
5. Multi-file undo: query all revisions with same `transaction_id`, undo all atomically (requires `*Store`-level API, not per-Document)
6. `compaction.go`: Collapse consecutive minor local revisions older than 24h into one (keep latest blob, delete intermediate blobs + their ops). 30-day rule: collapse to daily granularity. Protect: `is_save=1`, `source IN ('merge', 'agent:*')` revisions.
7. Schedule compaction: on app startup + hourly `tea.Tick` command (workspace owns timer)
8. Handle edge case: if undo walks into compacted region (ops deleted), gracefully fall back to snapshot-level jump

**Verification**:
- Test: rename file on disk → reopen → history preserved under new path
- Test: inode reuse (delete + create with different content) → new document, not linked to old
- Test: multi-file transaction undo → all files revert to pre-transaction state
- Test: compaction reduces revision count while preserving save/merge nodes
- Test: undo still works after compaction (snapshot-level fallback for GC'd ops)
- Test: compaction doesn't delete ops for revisions < 24h old

---

## Key Relevant Files

- `pkg/editor/buffer/buffer.go` — Buffer type stays; docstate consumes its `ApplyEdits` output
- `pkg/editor/history/history.go` — **Deleted** in Phase 1 step 13
- `pkg/ui/components/editor/editor.go` — `doc` field replaces `history`, `dirty`, `savedContentHash`
- `pkg/ui/components/editor/apply.go` — `applyOperation`/`applyUndo`/`applyRedo` fully rewritten
- `pkg/ui/components/editor/commands_clipboard.go` — `history.EditPaste` → `docstate.EditPaste`
- `pkg/ui/components/editor/commands_image.go` — same constant rename
- `pkg/ui/components/editor/dictation.go` — same constant rename (×2)
- `pkg/ui/components/editor/commands_history.go` — undo/redo dispatch unchanged in structure
- `pkg/ui/components/editor/commands_history_test.go` — rewritten (docstate fixture)
- `pkg/ui/components/editor/editor_test.go` — rewritten (docstate fixture)
- `pkg/ui/components/editor/commands_edit_test.go` — rewritten (docstate fixture)
- `pkg/ui/components/editor/commands_multi_test.go` — rewritten (docstate fixture)
- `pkg/ui/pages/workspace/workspace.go` — Store lifecycle, FlushDocument on quit

---

## Dependencies

- `modernc.org/sqlite` — Pure-Go SQLite (no CGO)
- `github.com/klauspost/compress/zstd` — Fast zstd compression
- `github.com/fsnotify/fsnotify` — Already in use for dir watching
- 3-way merge: TBD Phase 2 (fallback: `github.com/sergi/go-diff` + custom diff3 logic)

---

## ADR Appendix

### ADR-1: Snapshots as Ground Truth, Ops as Secondary

**Context**: Need crash recovery + keystroke undo across restarts.
**Decision**: Full document snapshots (compressed blobs) are the canonical state. Operations persist for fine-grained undo but can be reconstructed or discarded. If ops are lost, system degrades gracefully to snapshot-level undo.
**Consequences**: Two write paths (ops every 300ms, blobs on idle). Crash recovery replays ops on last blob. Compaction can safely delete old ops.

### ADR-2: DAG with Branch-Aware Undo

**Context**: Multiple sources of edits (user, agents, external tools). User should be able to undo their own changes without undoing agent/external changes.
**Decision**: Revision DAG where each node has a `source` tag. Undo traverses only `source='local'` nodes. External/merge/agent nodes are skipped but preserved in graph.
**Consequences**: More complex undo traversal. LCA computation needed for merges. Users can always recover any state by navigating the DAG.

### ADR-3: Character-Level 3-Way Merge

**Context**: Markdown notes frequently edited concurrently by user + agents. Line-level merge is too coarse (agents often modify within a line).
**Decision**: Use character-level diff algorithm for 3-way merge. Conflicts at character granularity.
**Consequences**: Better merge quality for inline edits. Need a capable diff library. Slightly more complex conflict regions.

### ADR-4: Content-Addressed Blobs with zstd

**Context**: Need fast random access to any historical snapshot (for merge LCA, for undo across restarts).
**Decision**: Store full document as zstd-compressed blob keyed by SHA-256. No delta chains.
**Consequences**: Any revision readable in O(1) without chain-walking. Storage grows linearly with edits (mitigated by dedup of identical content and compaction). zstd decompression is <1ms for note-sized files.

### ADR-5: Inode-Based File Identity (Phase 3)

**Context**: Files are renamed/moved frequently in note vaults. History should survive renames.
**Decision**: Track files by (inode, device) from OS stat. Content hash stored for future similarity heuristic. Phase 1 uses path-only identity with nullable inode/device.
**Consequences**: Works on macOS/Linux. Doesn't survive cross-device moves. Inode reuse detected via content hash mismatch. Path stored as fallback. Untitled docs have NULL identity until saved.

### ADR-6: Ops Frequent, Blobs on Pause

**Context**: Balance between crash recovery granularity and storage efficiency.
**Decision**: Operations written every coalesced group (~300ms). Full snapshot blob created on 2s idle pause.
**Consequences**: Crash loses at most 2s of work (ops are durable). Blob count stays manageable (~30/hour active editing vs ~10,800/hour if every group were a blob). Compaction further reduces over time.

### ADR-7: SQLite in Vault Root

**Context**: Need persistent storage tied to the vault.
**Decision**: `.rune/state.db` in vault root directory. Created on first use.
**Consequences**: History travels with vault. Survives app reinstall. Should be .gitignored. User can inspect/backup. Multiple vaults have independent state.

### ADR-8: Big Bang Migration

**Context**: Replacing history.UndoStack + dirty tracking with docstate.
**Decision**: Replace all at once rather than incremental layering.
**Consequences**: Temporary test breakage during migration. Cleaner final result. No legacy adapter code. Existing `history` package deleted after migration. All call sites enumerated (18 locations across 10 files).

### ADR-9: Operations Schema — Pending vs Committed (Critic Fix #1)

**Context**: Need to distinguish uncommitted ops (for crash recovery) from committed ops (for undo navigation within a revision).
**Decision**: Operations have `base_revision_id` (always set) and `committed_revision_id` (NULL while pending, set to new revision ID on flush). Separate indexes for each query path.
**Consequences**: Crash recovery = query WHERE `committed_revision_id IS NULL`. Undo within revision = query WHERE `committed_revision_id = X`. Flush = UPDATE SET `committed_revision_id = new_id`. No ambiguity.

### ADR-10: Save Boundary Persistence (Critic Fix #4)

**Context**: Need durable dirty/clean state that survives restarts and interacts correctly with undo.
**Decision**: `documents.saved_revision_id` points to last revision that matches disk. `revisions.is_save` marks which revision was the save point. `IsDirty()` = (HEAD != saved_revision_id) OR (pending ops exist).
**Consequences**: Undo past save point → dirty. Redo back to save → clean. Save during pending ops → flush first. Matches current `StartSave`/`FileSavedMsg` protocol exactly.
