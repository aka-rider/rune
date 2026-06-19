# Data-loss remedy — executable plan (rev. 6)

> **Deliverable of this task:** this file *is* the plan. Rev. 6 closes the second critic round
> (N1–N9, plus the B2 import-cycle) against rev. 5 — all wiring/mechanism corrections; the VFS
> architecture and all seven Fixes carry forward unchanged. The load-bearing fixes: journal
> transactions run through a pinned `sql.Tx` with `SetMaxOpenConns(1)` + `_txlock=immediate`, **not**
> raw `BEGIN IMMEDIATE` over a pooled handle (N1); `RecoverDocument` replays through a pure
> `buffer.ReplayForward`, **not** the UI-layer `markdownedit.Reapply` (which would be an import
> cycle) (B2); `Content(docID)` is the `current_seq` reconstruction, not raw `LatestSnapshot` (N3);
> the DL1 fuzz invariant gets its VFS content from the driver, keeping `invariant` docstate-free
> (N2); `opentabs` carries a real `DocID` per tab so untitled tabs no longer collide on `path=""`
> (N4); the async snapshot co-captures `(content, seq)` in the `Update` pass (N5); undo stays
> doc-chronological with focus following the returned surface (N6); the chat journal reuses one
> stable reserved doc (N7); the fuzz mirror is pinned to a single doc (N8); and `events.seq ≥ 1` is
> pinned so the `snapshots.seq=0` backfill never collides (N9).
>
> Rev. 5 closed the first critic round (B1–B9) against rev. 4; rev. 4 closed the original 7
> structural blockers. Those resolutions (the VFS model, `documents.current_seq`, seq-tagged
> snapshots, the inode/path migration, the dictation guard, the Cancel-last guard set) are preserved.

---

## Context

`rune` loses user content through interacting defects, all confirmed against the code:

1. **Dictation clear (root trigger).** The dictation engine (`dictengine/start_darwin.go`) is
   **cumulative**: every chunk re-transcribes the whole session, so `Accumulated` in
   `PartialTranscriptionMsg` is always the full re-transcription so far. When whisper returns an
   empty string for a transient bad result, `Accumulated == ""` is emitted; the dictation
   component (`dictation/dictation.go:92-104`) turns it into
   `pendingEdit{start, end, text: ""}`, applied verbatim as `m.editor.ReplaceRange(s, e, "")`
   (`workspace.go:664-673`) — deleting the committed region. No guard exists at any layer.
2. **Disk-destruction vectors (every disk write is unsafe).**
   - **Autosave → disk.** Every main-surface edit schedules an autosave: `journalEdit`
     (`workspace.go:573`) → `scheduleFlush` (`:551`) → `pendingFlushMsg` (`:1052`) → `startSave`
     → `saveFileCmd` → `os.WriteFile` (`workspace_fileio.go:79`). A dictation-cleared buffer thus
     overwrites the real `.md` with `"\n"`.
   - **Non-atomic + clobbering first-write.** `saveFileCmd` (`:79`), the image-paste write, **and
     `createFileCmd` (`workspace.go:1437`)** all use `os.WriteFile`/`os.Create`. None is atomic
     (a crash mid-write truncates the file), and `createFileCmd` does **no existence check** — so
     naming an "Untitled" buffer over a name that already exists on disk **silently truncates that
     file** (Catastrophic, rung 1). rev. 3 missed this third site.
3. **No durable recovery; untitled work is volatile; no close guard.**
   - The undo/redo journal lives in a `:memory:` DB (`store.go:65-80`, `openMem` `:114`) and dies
     on exit. `documents.path` is non-unique (`idx_documents_path`, `:41`), so `EnsureDocument`
     (`store.go:303`) can create a fresh row per call and `SELECT id WHERE path=?` is
     non-deterministic. `FileLoadedMsg` (`:960`) never reads a snapshot back.
   - **Untitled buffers are not durable at all.** `scheduleFlush` snapshots only when
     `docID > 0` (`:557`), and untitled runs with `docID == 0` (`:448`, `:517`) — so an unsaved
     "Untitled" buffer is never snapshotted and evaporates on crash. Worse, its content lives in a
     volatile RAM stash (`untitledContent`/`untitledTitle`, `:119-120`), restored by hand on tab
     switch (`showUntitled`, `:445`).
   - `requestCloseCurrent` (`:459`) closes dirty buffers with no prompt (BUG1).

---

## Architecture — SQLite is the document VFS

```
            write path                          read path
   rune ──▶ simple API ──▶ SQLite ──▶ disk     disk ──▶ SQLite ──▶ simple API ──▶ rune
  (editor)   (docstate)    (VFS)   (.md, on    (.md)    (VFS)      (docstate)    (editor)
                                    ⌘S only)   seed once
```

**One store, one API, two disk edges.** `docstate` is a virtual filesystem for documents. The
workspace speaks **only** the docstate API; it never reads or writes the user's `.md` except across
the VFS's two disk edges:

- **load (disk → VFS):** opening a path resolves its VFS document and restores **content + undo/redo
  DAG** from the VFS. Disk is read only to *seed* a document the VFS has never seen, or to *reconcile*
  an external change (§1.4.7); a document the VFS already knows — **including a clean, previously-saved
  one** — loads its history from the VFS, never from a bare disk read. The buffer the user sees is the
  VFS document (`disk → sqlite → simple api → rune`).
- **materialize (VFS → disk):** on an explicit, intentional act — ⌘S and save-on-close — write the
  VFS document's current bytes back to the file, atomically. **Nothing else touches the `.md`.**

**Everything open is a VFS document (`doc`).** A `doc` has a stable id (the VFS primary key),
durable content (snapshots/blobs), and a durable per-doc undo DAG (events). A `doc` is in one of two
states:

- **bound** — attached to a real file by `(inode, device)` + path. ⌘S materializes it back.
- **unbound ("Untitled")** — no disk path. It is a *real, durable, recoverable* VFS file that simply
  has not been materialized yet. **"Untitled 1, 2, 3" are VFS files, not disk files.** Naming an
  untitled doc = its first materialize = binding it to a path.

Help is **not** a doc — it is a transient read-only view that borrows the editor. It is never
journaled, snapshotted, or recovered. After this change `docID == 0` means exactly **"no document"
(help)**, and nothing else.

### Why this is the unified fix

The data-safety rules fall *out of the model* instead of being patched into each call site:

| CLAUDE.md rule | Satisfied because |
|---|---|
| §1.4.2 — disk changes only on explicit save | autosave writes the VFS; only **materialize** touches disk |
| §1.4.3 / §1.4.6 — unsaved work **and** undo history survive crash *and* reopen | the VFS is a **WAL'd on-disk DB**; **every open restores content *and* the per-doc undo DAG** — clean files included; history is never dropped on load |
| §1.4.1 — atomic, durable writes through one helper | **materialize** is the single VFS→disk primitive |
| (new) — never clobber an existing file | binding to an **occupied path is refused** ("File already exists") |
| §1.4.6 — stable, rename-safe identity | **document id ≠ disk binding**; rename re-binds and keeps history |
| §1.4.7 — externally-changed file is a hazard | reconciled at the **load edge** against the doc's baseline fingerprint |
| §1.3 — validate edits before persist | the VFS write boundary clamps/drops suspect edits (dictation) |

### What the reframe deletes

These special-cases disappear — track them as removals in review:

- the `untitledContent` / `untitledTitle` RAM stash (`workspace.go:119-120`) and `showUntitled`'s
  hand-restore (`:445`) — untitled content now lives in the VFS like every other doc;
- the `docID == 0`-means-untitled overload — untitled gets a real doc id; `0` means help only;
- `nextUntitled` statting the disk and tolerating empty `.md` files (`:1484`) — untitled naming is
  VFS-side and creates **no** disk files;
- the "ephemeral untitled row" concept from rev. 3 Fix 6 §4 — untitled is just the default unbound
  doc, not a row that needs GC special-casing;
- two divergent first-write paths (`createFileCmd` vs `saveFileCmd`) — both collapse into one
  **materialize** primitive.

### The simple API (docstate surface)

The workspace orchestrates I/O (it owns the disk domain, D12 / §5.3), but it does so **only** through
these verbs. "Materialize" is a workspace `tea.Cmd` built on the one shared atomic-write helper; the
rest are docstate methods.

| VFS verb | docstate / workspace surface | Status | Notes |
|---|---|---|---|
| open a file | `OpenPath(path) (DocRef, error)` | replaces `EnsureDocument` | stat → bind-or-create; returns `{ID, RenamedFrom}` (Fix 3) |
| new untitled | `CreateScratch(name) (DocRef, error)` | **new** | unbound durable doc; no disk, no inode |
| read content | `Content(docID) (string, error)` | **= `current_seq` reconstruction** (N3) | the VFS read path (tab switch-back); same reconstruction as `RecoverDocument`, **not** raw `LatestSnapshot` |
| recover | `RecoverDocument(docID) (content string, hasHistory bool, err error)` | **new** (Fix 5) | content reconstructed **at `current_seq`** (the buffer last shown) + whether a DAG exists |
| journal edit | `AppendEdit(docID, …) (seq int64, err error)` | **+`docID`, returns `seq`** (Fix 2) | per-doc DAG; returns the head seq so the caller co-tags the snapshot (N5) |
| snapshot | `CreateSnapshot(docID, content, source string, seq int64) (int64, error)` | updated | captures head seq at snapshot time; used as recovery anchor by `RecoverDocument` (Fix 6) |
| undo / redo | `UndoTarget(docID)` / `RedoTarget(docID)` | **+`docID`** (Fix 2) | per-doc, durable `current_seq` |
| materialize | `materializeCmd(docID, path, content, baseline)` | **new** (Fix 4) | atomic temp→fsync→rename; refuses occupied new path |
| close store | `Close()` | exists | closes `perm` only after Fix 2 |

`DocRef{ ID int64; RenamedFrom string }`. A doc is "bound" iff its `path != ''`.

### Decisions baked in (flagged for review)

1. **Every open loads content *and* the undo/redo DAG from the VFS — always, clean files included.**
   Opening a document restores both its bytes and its history from the VFS; the editor never binds to
   a bare disk read. A "clean" file (VFS content == disk) is *not* an exception — its DAG must still
   load so ⌘Z steps back through prior sessions. **Dropping the DAG on open is data loss (§1.4.3,
   §1.4.6), not an optimization.** Disk is read only to *seed* a document the VFS has never seen, or to
   *reconcile* an external change (§1.4.7). There is no "disk-only" read path for any document.
2. **Untitled docs are durable and recoverable across restarts.** An unsaved "Untitled" buffer
   survives a crash and is offered for recovery on next launch (today it is silently lost). Empty
   untitled docs (no content, no history) are GC'd on open; non-empty ones are retained until
   materialized or explicitly discarded.
3. **First save refuses to clobber.** Binding an untitled doc to a path that already exists on disk
   fails with **"File already exists"**; the buffer is kept and the user picks another name. ⌘S of an
   already-bound doc overwrites its own file (after the §1.4.7 external-change check).

---

## Fix 1 — Dictation must never delete committed text (the VFS write boundary)

**File:** `pkg/ui/components/dictation/dictation.go` (handler at `:92-104`). *Unchanged from rev. 3
— this is the validation at the boundary where async edits enter the VFS (§1.3).*

- In the `dictengine.PartialTranscriptionMsg` case: if the incoming `Accumulated` is empty or
  whitespace-only, **drop the pending edit** (leave `m.pending`/`m.hasPending`/`m.appliedLen`
  untouched) and return. Because the engine is cumulative, `appliedLen` still points at the end of
  the last-applied text; the next non-empty partial correctly replaces `[startOff, startOff+appliedLen]`
  with the new full transcription. Never emit a replace that collapses a non-empty applied region to
  empty.
- `appliedLen`/`startOff` are **byte** offsets feeding byte-indexed `ReplaceRange`/`buffer.Edit`, so
  `len(text)` is correct — do **not** switch to `utf8.RuneCountInString` (CLAUDE.md §1.5 governs
  *display* math, not buffer offsets).
- If the buffer was reloaded mid-session (`end > bufferLen`), cancel the pending edit rather than
  replace a stale range. The guard stays in the producer (dictation), not in `ReplaceRange`.

---

## Fix 2 — docstate *is* the VFS: durable, per-document DAG; untitled is a first-class unbound doc

**Files:** `pkg/docstate/store.go`, `journal.go`, `snapshot.go`. *Merges rev. 3 Fix 2 (durable DAG)
and Fix 6 §4 (untitled rows) — under the VFS model they are the same change: make every document,
bound or unbound, a durable VFS entity.*

1. **Move events into the perm DB; eliminate the `:memory:` journal.**
   - Add `events` to `permSchema` with `doc_id INTEGER NOT NULL REFERENCES documents(id)` and
     `CREATE INDEX idx_events_doc ON events(doc_id, seq)`. Keep existing columns (incl.
     `anchor_snapshot_id`, `is_undo_stop`).
   - Delete `mem *sql.DB`, `openMem`, `memSchema`, `initMemSchema`. Route every event query to
     `s.perm`; `Close()` closes only `perm`. `OpenInMemory`/`OpenAt`/`NewTestStore` already make
     `perm` a `:memory:` or temp DB, so ephemeral runs stay ephemeral with no extra code. Drop the
     `mem`/`openMem` wiring from all five `Open*`/`NewTestStore` constructors.
   - **No data migration for events** — they never existed in perm, so `CREATE TABLE IF NOT EXISTS`
     adds an empty table to existing DBs.
   - **Enable WAL on perm (`PRAGMA journal_mode=WAL` in `openPerm`).** WAL is relevant only for
     the file-backed path (on-disk `rune.db`); it is a silent no-op on `:memory:` (the fuzz path
     and the in-memory fallback remain in DELETE journal mode — this is correct and expected). No
     correctness property depends on WAL; it only lowers durability latency on the file-backed path
     so "at most a few seconds at risk" (§1.4.3) holds. After dropping `mem` and `currentSeq` from
     all five `&Store{}` literals, verify that `Open`, `OpenInMemory`, `OpenAt`, `NewTestStore`, and
     any remaining constructor compile cleanly and still journal correctly.
   - **Single connection per store (N1).** `database/sql` pools connections and routes each statement
     to an arbitrary idle one — fatal for the multi-statement transactions in §3 (a raw
     `BEGIN IMMEDIATE` and its `COMMIT` could land on different connections and run in autocommit) and
     for `:memory:` stores (a second pooled connection opens a *fresh, empty* in-memory DB). Call
     `db.SetMaxOpenConns(1)` on **every** store handle right after `sql.Open`, and build the DSN with
     `_txlock=immediate` (so `db.Begin()` issues `BEGIN IMMEDIATE`) and `_busy_timeout=5000`. This one
     cap dissolves the pooling-vs-transaction *and* `:memory:`-per-connection hazards in a single
     place (CLAUDE.md "prefer unified solutions"); the throughput cost is nil for a local single-user
     editor. The current DSN is `file:%s?_foreign_keys=on` (`store.go:101`) — extend it to
     `file:%s?_foreign_keys=on&_txlock=immediate&_busy_timeout=5000`.

2. **Scope the journal by `doc_id`; undo is per-document.** Add `docID int64` as the first
   parameter to `AppendEdit`, `UndoTarget`, `RedoTarget`, `AllEdits`. Add `AND doc_id=?` to every
   event query and `doc_id` to the INSERT.

   Surface → docID mapping:
   - `"main"` and `"title"` surface edits journal under the **active document's `docID`** (the title
     is part of that document's VFS record).
   - `"chat"` is a global assistant surface that spans all documents; it journals under a reserved
     singleton `chatDocID` — a single `path=''` unbound doc row created once at store-ready and
     stored as `m.chatDocID int64` on the workspace Model (created in §5 below). Chat edits are
     isolated from any document's undo history.
   - Undo/redo become **doc-scoped**: `handleUndo`/`handleRedo` pick `docID = m.docID` when
     main/title is focused, `m.chatDocID` when chat is focused, and call `UndoTarget(docID)` /
     `RedoTarget(docID)` accordingly. Undo no longer crosses *documents* — a strict improvement over
     today's single global journal.
   - **Within a doc, undo stays chronological across `main`+`title` (N6).** Both surfaces share one
     `docID`, so `UndoTarget(m.docID)` returns the newest undo-stop regardless of which of the two is
     focused; `handleUndo` then dispatches by the returned `surface` and lets focus follow it
     (unchanged from today, `workspace.go:495-514`). This is deliberate — main and title are one
     document's edit stream — so a title-focused ⌘Z may step back a body edit and move focus to the
     editor. Do **not** add `AND surface=?` scoping; that would split one document's history into two
     independent stacks.

3. **Persist the undo pointer per document — explicit transactions.**
   SQLite's `WITH` clause admits only `SELECT` — writable CTEs do not exist in SQLite.
   Coordinating the multi-statement sequence required by each journal op (delete future events,
   reset `current_seq`, insert a new event) requires an explicit transaction.
   `go-sqlite3` v1.14.45 bundles SQLite 3.53.2, which supports `BEGIN IMMEDIATE` and `RETURNING`
   (added in 3.35) but rejects any `WITH … INSERT/UPDATE/DELETE` construct — those are a syntax
   error. A transaction *is* the atomic unit; a single trailing DML with `RETURNING` is fine for
   returning the new `seq` but cannot coordinate the cross-table multi-statement sequence.

   - Remove `Store.currentSeq int64` and the `currentSeq: math.MaxInt64` initializer from all five
     `&Store{…}` literals.
   - Add `current_seq INTEGER` to `documents`: `NULL` = "at end of history" (replaces the old
     `math.MaxInt64` sentinel), `N` = "undone to just before event N".
   - **Run each op through a pinned `sql.Tx`, not raw `Exec` (N1).** Open with
     `tx, err := s.perm.Begin()` (the `_txlock=immediate` DSN from §1 makes this a `BEGIN IMMEDIATE`),
     run every statement on `tx`, then `tx.Commit()` (or `tx.Rollback()` on any error). **Never**
     issue `BEGIN IMMEDIATE`/`COMMIT` as separate `s.perm.Exec` calls — with the connection pool they
     can land on different connections and silently run in autocommit, defeating atomicity
     (`SetMaxOpenConns(1)` from §1 is the backstop). Read any `RETURNING` value with
     `tx.QueryRow(…).Scan(&seq)`, **never** `Exec`+`LastInsertId` (that returns the rowid, not `seq`):
     - **`AppendEdit`** (returns `(seq int64, err error)`): `tx` → (if `current_seq IS NOT NULL`)
       `DELETE FROM events WHERE doc_id=? AND seq > ?` → `UPDATE documents SET current_seq=NULL
       WHERE id=?` → `INSERT INTO events(doc_id, …) VALUES(?, …) RETURNING seq` (read via
       `QueryRow.Scan`) → `Commit`. Return the new `seq` so the caller co-tags the snapshot (N5).
     - **`UndoTarget`**: `tx` → `SELECT seq, surface, edits, cursors_before FROM events
       WHERE doc_id=? AND is_undo_stop=1 AND seq<=? ORDER BY seq DESC LIMIT 1` → if found:
       `UPDATE documents SET current_seq=seq-1 WHERE id=?` → `Commit`. Returns event data, `ok=true`.
     - **`RedoTarget`**: `tx` → `SELECT … WHERE doc_id=? AND is_undo_stop=1 AND seq>?
       ORDER BY seq ASC LIMIT 1` → if found: `UPDATE documents SET current_seq=seq WHERE id=?` →
       `Commit`.
   - The IMMEDIATE lock intent prevents writer starvation against WAL readers. No partial-write crash
     window: SQLite rolls the transaction back on restart.

4. **Coordinate snapshots with the DAG via seq-tagged snapshots.** Snapshots must be anchored to a
   specific DAG position so `RecoverDocument` knows where to begin forward replay. Without a seq
   anchor there is nothing to replay *from* — the first ⌘Z would invert against the wrong base
   (silent corruption, §1.3).

   - Add `seq INTEGER NOT NULL DEFAULT 0` to the `snapshots` table. `seq` records the DAG event seq
     the snapshot content corresponds to — i.e., `MAX(seq)` of events for that doc at capture time,
     or `documents.current_seq` when the snapshot is taken while the user is in an undone state.
   - Update `CreateSnapshot(docID, content, source string, seq int64) (int64, error)` to accept and
     persist `seq` **as a passed-in value** — `CreateSnapshot` never re-derives it from the DB.
     **Co-capture `(content, seq)` in the same `Update` pass (N5).** The snapshot's `content` is the
     editor buffer; its `seq` must be the head seq that buffer corresponds to. The model is the only
     consistent, single-threaded source for both: `journalEdit` records `AppendEdit`'s returned `seq`
     on the model (e.g. `m.headSeq int64`); when `snapshotCmd`'s closure is built (in the
     `pendingFlushMsg` handler and on switch-away), capture `m.editor.Content()` and `m.headSeq`
     together into locals (§5.5) and pass both to `CreateSnapshot`. Never read `MAX(seq)` from the DB
     inside `CreateSnapshot` on the async path — an edit landing between content-capture and the DB
     read would tag stale content with a newer seq and re-open the B2 corruption window.
   - `RecoverDocument(docID)` (Fix 5 §3) uses `seq` as the recovery anchor: find the newest
     snapshot with `seq ≤ current_seq`, then forward-replay events in `(snap.seq, current_seq]`.
     In the common case (`current_seq` NULL at head, latest snapshot taken at head seq) no replay
     is needed — the snapshot loads verbatim.
   - Relationship to `anchor_snapshot_id`: `snapshots.seq` is the recovery anchor used by
     `RecoverDocument`. `anchor_snapshot_id` on events is retained solely as a future compaction
     hint (the snapshot base below which events can be pruned). Never use `anchor_snapshot_id` for
     recovery — that would create two inconsistent linkages.
   - `anchor_snapshot_id` on append: resolved as a scalar subquery inside `AppendEdit`'s
     transaction: `(SELECT id FROM snapshots WHERE doc_id=? ORDER BY id DESC LIMIT 1)` folded into
     the `INSERT` — no extra round-trip.
   - Migration: backfill `snapshots.seq = 0` for existing rows (the `events` table is brand-new and
     empty for existing databases, so existing snapshots have no events to replay and load verbatim;
     `seq=0` is correct and safe). This relies on **`events.seq ≥ 1`** (N9): keep `events.seq`
     `INTEGER PRIMARY KEY AUTOINCREMENT` so real seqs start at 1 and a backfilled `seq=0` snapshot can
     never collide with a real event. Do not reset the AUTOINCREMENT counter or insert an explicit
     `seq=0` event.

5. **Untitled is the default unbound doc — no special row type.** `CreateScratch(name)` inserts a
   `documents` row with `path=''`, `inode=0`, `device=0`, returns its id, and the workspace sets
   `m.docID` to it. Untitled edits journal and snapshot under that id exactly like a bound doc — so
   untitled work is now crash-recoverable. These rows are excluded from both unique indexes (Fix 3),
   so many untitled docs coexist. Help keeps `docID==0` and is never journaled — extend `journalEdit`
   and `scheduleFlush` to skip when `m.docID == 0`.

   **Chat singleton (`chatDocID`).** Chat is a global surface; it needs a journal doc but its history
   is not worth keeping across sessions. To avoid leaking one abandoned chat doc per launch (N7),
   reuse a **single stable reserved row** rather than `CreateScratch`-ing a new one each time. On
   `StoreReadyMsg`, resolve the reserved chat doc by a sentinel path that can never name a real file
   (e.g. `path='\x00chat'`, unique and excluded from the inode index since `inode=0`): `INSERT OR
   IGNORE` it, then **truncate its events** (`DELETE FROM events WHERE doc_id=?`) so each session
   starts clean, and store its id as `m.chatDocID int64`. Chat surface edits always journal under this
   fixed id regardless of which document is active. The `chatDocID` row is **not** snapshotted by
   `scheduleFlush` — it is not a document buffer; only `journalEdit` uses it. Add `chatDocID int64` to
   the workspace `Model` struct.

6. **Concurrency / fail-fast.** Journal writes stay synchronous against WAL'd SQLite (bounded sub-ms
   local writes; keeps undo/redo synchronous as today). This is a deliberate, documented exception to
   §5.3 — justified because the writes are bounded local ops and making undo async would be a large,
   risky refactor. If profiling shows event-loop stalls, move `AppendEdit` (writes only) into a Cmd.
   The `_ = err` swallows in `journalEdit` (`workspace.go:577`) and `scheduleFlush` (`:563`) must be
   replaced with surfaced errors (§1.3).

---

## Fix 3 — Document identity vs. disk binding (resolves the "what is identity?" muddle)

**File:** `pkg/docstate/store.go`. *rev. 3 said "Identity = (inode, device)" but then had to exclude
untitled (inode=0) from the index — because inode can't identify a document that has no inode. The
VFS model removes the contradiction:*

> **Document identity = `doc_id`** (the VFS primary key). It is what snapshots and the undo DAG key
> on; it is stable across renames and never `0` for a real document. **Disk-binding identity =
> `(inode, device)` + path** — secondary metadata describing *which file, if any, a document is bound
> to*. A document binds to at most one file; a file binds to at most one document. Untitled documents
> are unbound. The filesystem is authoritative for *binding*; the VFS is authoritative for *identity
> and history*.

1. **Two unique indexes — over the binding, not the document.**
   - `CREATE UNIQUE INDEX IF NOT EXISTS idx_documents_inode ON documents(inode, device) WHERE inode != 0`
     — one document per real file.
   - Change `idx_documents_path` to **unique**:
     `CREATE UNIQUE INDEX IF NOT EXISTS idx_documents_path ON documents(path) WHERE path != ''`
     — no two documents share a non-empty path (covers degraded inode=0 rows too).
   - Untitled rows (`path=''`, `inode=0`) are excluded from both — many unbound docs are correct.

2. **`OpenPath(path)` keys on inode** (replaces `EnsureDocument`). It is only called for files that
   exist on disk (`FileLoadedMsg` `:976`, `StoreReadyMsg` `:1046` — both after the file was read), so
   `stat` yields a real inode.

   **Build-tagged file-identity helpers.** Inode retrieval is platform-specific and must not pull
   `syscall` directly into `store.go` (which would prevent compilation on non-unix targets). Add two
   build-tagged files in `pkg/docstate/`:
   - `fileid_unix.go` (`//go:build unix`) — uses `syscall.Stat_t` from the OS-specific stat result
     to return `(inode, device uint64, ok bool)`.
   - `fileid_other.go` (`//go:build !unix`) — returns `(0, 0, false)` as a no-syscall fallback.

   `OpenPath` calls `fileID(path)` from these helpers. When `ok == false` (non-unix platform, or
   `stat` fails), degrade to path-keying (matches the `inode==0` fallback branch below). `store.go`
   must not import `syscall` directly.

   ```
   stat(path) -> (inode, device)              // via fileID() helper (fileid_unix.go); fallback: inode=0, ok=false
   if stat fails or inode == 0:               // race: file vanished -> degraded fallback by path
       row = SELECT id FROM documents WHERE path=? AND inode=0
       found -> UPDATE last_seen_at; return id ; else INSERT(inode=0); return newID
   row = SELECT id, path FROM documents WHERE inode=? AND device=?
   if row found:
       if row.path != path:                   // renamed while rune was closed -> re-bind + warn
           UPDATE documents SET path='' WHERE path=? AND id != row.id   // free a reused path
           UPDATE documents SET path=?, last_seen_at=? WHERE id=row.id
           renamedFrom = row.path
       else UPDATE last_seen_at WHERE id=row.id
       return {ID: row.id, RenamedFrom: renamedFrom}
   else:                                       // new inode at this path (deleted+recreated)
       UPDATE documents SET path='' WHERE path=? AND inode != ?   // evict the stale path holder
       INSERT documents(path, inode, device, …); return {ID: newID}
   ```
   Signature `OpenPath(path string) (DocRef, error)` with `DocRef{ID int64; RenamedFrom string}`. The
   workspace surfaces the rename warning via `footer.ShowErrorMsg` when `RenamedFrom != ""` (text
   built in the workspace, §2.4). Both call sites and any tests adopt the new return.

3. **Minimal migration framework** (no `user_version`/`ALTER`/migration code exists today). In
   `initPermSchema`, after `CREATE TABLE IF NOT EXISTS`, run `migrate(db)` gated on `PRAGMA
   user_version`. Step 1 (one-time, bump to 1), in a transaction respecting `foreign_keys=ON`
   (`store.go:85`) and the `snapshots.doc_id → documents.id` FK (`:50`):
   1. **Backfill `(inode, device)`** by `stat`-ing each row's `path` (best-effort; gone ⇒ leave `0`).
   2. **Dedup by inode:** among rows sharing a non-zero `(inode, device)`: survivor = `MIN(id)`;
      `UPDATE snapshots SET doc_id=survivor WHERE doc_id IN (dups)` (repoint **before** delete);
      `DELETE FROM documents WHERE id IN (dups)`.
   3. **Dedup by path:** among rows still sharing a non-empty path (inode=0 orphans from the old
      `EnsureDocument` bug): survivor = the row whose live `stat` matches, else `MIN(id)`; repoint
      snapshots; delete others.
   4. **Create both unique indexes** (must run **after** both dedup passes).
   Only `snapshots` needs repointing — perm `events` is brand-new and empty.

4. **Residual edge (documented).** Inode reuse (fileA deleted, fileB later assigned fileA's inode on
   the same device) would attribute fileA's history to fileB; the rename warning fires (paths differ)
   so the user is notified. Size/created_at corroboration is future hardening.

---

## Fix 4 — The VFS→disk edge: one atomic durable *materialize*; refuse to clobber

**Files:** new `pkg/ui/pages/workspace/workspace_fileio.go` helper (+ optional small `atomicwrite`
package), `markdownedit/commands_image.go`. *This is the §1.4.1 helper rev. 3 named but never made a
first-class fix, plus the "File already exists" guard, plus the §1.4.7 external-change check — all of
which live at this single edge.*

1. **One atomic, durable write helper.** Every write of user content to disk goes through it:
   1. write to a temp file in the **same directory** as the target (same filesystem → atomic rename);
   2. `Sync()` the temp file;
   3. `os.Rename` temp over target (atomic replace — a reader sees old or new, never partial);
   4. `fsync` the parent directory so the rename survives a crash.
   Write the VFS bytes **verbatim** — no normalization of line endings, trailing newline, encoding, or
   BOM (§1.4.5). A save is "done" — and the buffer "clean" — only when the rename returns.

2. **Route all three current writers through it:** `saveFileCmd` (`workspace_fileio.go:79`),
   `createFileCmd` (`workspace.go:1437`), and the image-paste write. Per "prefer unified solutions,"
   none keeps its own `os.WriteFile`.

3. **Materialize has two modes:**
   - **bind-new (first save of an untitled / rename to a new name):** create with `O_CREATE|O_EXCL`
     (or `stat` then refuse). If the path exists → **"File already exists"** error; **keep the
     buffer**, do not write, do not bind. This replaces `createFileCmd`'s silent truncate. On
     success, the workspace calls `OpenPath(newPath)` to bind the doc (Fix 3) and records the new
     baseline fingerprint.
   - **overwrite-bound (⌘S on an already-bound doc):** before writing, compare the file's current
     fingerprint (mtime + size, hash on mismatch) to the doc's load baseline. Diverged → refuse and
     surface a conflict (§1.4.7); never silently win. Unchanged → atomic-overwrite, then update the
     baseline.

4. **The materialize Cmd carries everything it needs** (capture `docID`, `path`, `content`,
   `baseline` into locals before the closure, §5.5) and returns `FileSavedMsg` / `FileSaveErrorMsg`
   (existing) or a new `FileExistsMsg{Path}` for the bind-new conflict, handled by showing the footer
   error and keeping the buffer.

---

## Fix 5 — The disk→VFS edge: every open restores content *and* the undo/redo DAG

**Files:** `pkg/ui/pages/workspace/workspace.go` (`FileLoadedMsg` `:960`; tab-switch via
`requestOpenPath` `:388`), `pkg/docstate/snapshot.go`. *rev. 3 Fix 3, reframed and corrected: loading
a document is **always** a VFS read of content + history. "Clean" suppresses the dirty notice — it
never skips loading the DAG. **Dropping the DAG on open is the data loss the VFS exists to prevent**
(§1.4.3/§1.4.6); there is no disk-only fast path.*

1. **Open is a VFS read.** On `FileLoadedMsg`, after `OpenPath(path)` returns a stable `docID`
   (Fix 3), fire a workspace-owned recovery Cmd (SQLite read = I/O → Cmd, §5.3/§6.2; capture `store`,
   `docID`, `diskContent` into locals). It calls
   `RecoverDocument(docID) (content string, hasHistory bool, err error)` and returns
   `RecoveredMsg{Path, Content, HasHistory}` (workspace package, §2.4). `content` is the document
   reconstructed **at the persisted `current_seq`** — the exact buffer the user last saw — not merely
   `LatestSnapshot` (see §3).

2. **Three cases — but the DAG always loads.**
   - **No history** (`hasHistory == false`, the file never entered the VFS): show `diskContent`, empty
     DAG. *This is the only case that reads disk into the editor.*
   - **Clean** (`hasHistory`, `content == diskContent`): show `content`, **load the DAG**, mark clean,
     no notice. ⌘Z now steps back through earlier sessions. *Skipping the DAG here is exactly the loss
     the VFS prevents — do not optimize it away.*
   - **Unsaved** (`hasHistory`, `content != diskContent`): `SetContent(content)` + `MarkDirty` (the
     `.md` stays untouched until ⌘S) + show "Recovered unsaved changes — ⌘S to keep, ⌘Z to step back".
     Recovery is *editable history*, not a frozen snapshot.
   In every history case the DAG is live by virtue of `docID`: events persist in perm keyed by
   `docID`, `current_seq` is in the DB, so `UndoTarget(docID)`/`RedoTarget(docID)` resolve against the
   restored history with no extra wiring — **provided the shown content matches `current_seq`** (§3).

3. **Content↔DAG consistency invariant (§1.3).** The content loaded into the editor MUST equal the
   reconstruction at `current_seq`. Undo applies *inverse* edits to the live buffer (`ApplyInverse`,
   `workspace.go:497`); if the loaded base doesn't match the DAG position, the first ⌘Z inverts against
   the wrong bytes — silent corruption (rung 1). `RecoverDocument` owns this reconstruction using
   **seq-tagged snapshots + forward replay** (Fix 2 §4):

   1. Read `cs` = `documents.current_seq` for `docID` (`NULL` → treat as head: `SELECT MAX(seq)
      FROM events WHERE doc_id=?`, or `0` if no events).
   2. Find `snap = SELECT … FROM snapshots WHERE doc_id=? AND seq <= cs ORDER BY seq DESC LIMIT 1`.
      If no snapshot exists, return `hasHistory=false` (fall through to disk read, Fix 5 §2 case 1).
   3. Load `snap.content` from the blob store via `GetBlob(snap.blob_hash)`.
   4. Forward-replay events in `(snap.seq, cs]`: `SELECT edits FROM events WHERE doc_id=? AND seq>?
      AND seq<=? ORDER BY seq ASC`, then apply each batch to `content` with a **pure buffer-level**
      `buffer.ReplayForward(content string, batches [][]buffer.AppliedEdit) string` (B2). `docstate`
      must **not** call `markdownedit.Reapply` — that is a UI-editor *method* in
      `pkg/ui/components/markdownedit`, and importing it into `pkg/docstate` inverts the layering (an
      import cycle through the workspace). Add `ReplayForward` in `pkg/editor/buffer` modelled on the
      existing pure `replayBatch` in `internal/fuzz/driver/driver.go:44` (which already replays
      `string + []buffer.AppliedEdit` using only `pkg/editor/buffer`); `docstate` already imports
      `pkg/editor/buffer`, so there is no new dependency.
   5. Return the resulting content, `hasHistory=true`.

   The common case (`current_seq` NULL at head, latest snapshot taken at head seq) has `snap.seq ==
   cs`, so step 4 selects zero events and the snapshot loads verbatim — no replay overhead. Assert
   the round-trip in tests: after recovery the first ⌘Z must invert against the correct base.

4. **Tab switch-back reads the VFS, not disk.** On switching *away* from a doc, force one synchronous
   `CreateSnapshot` (sub-ms) so the VFS holds the latest bytes; on switching *back*, load via
   `Content(docID)` — which returns the **`current_seq` reconstruction** (§3), identical to
   `RecoverDocument`'s content, **not** raw `LatestSnapshot` (N3). This deletes the
   `untitledContent`/`untitledTitle` RAM stash and `showUntitled`'s disk-bypass — **all** tabs (bound
   and untitled) restore the same way, from the VFS. A clean bound doc whose disk file changed
   externally is reconciled here (re-seed from disk if the doc is clean; surface a conflict if both
   diverged, §1.4.7).

5. **Untitled recovery on launch.** Because untitled docs are durable (Fix 2 §5), non-empty unbound
   docs from a previous session are offered for recovery on startup (today they are lost). Empty ones
   are GC'd.

*(Cross-reference: `docID` comes from Fix 3. `current_seq` lives in the DB; `RecoverDocument` reads it
to reconstruct the shown content, and undo/redo read it to step — no `position` in the signature.)*

---

## Fix 6 — Autosave is VFS-only; delete the disk autosave vector

**File:** `pkg/ui/pages/workspace/workspace.go`. *rev. 3 Fix 4, unchanged — it is the mechanism that
makes "disk changes only on ⌘S" (Decision 1, §1.4.2) true.* The snapshot already happens in
`scheduleFlush` (`:561`) and the event in `journalEdit` (`:577`); the *only* disk-write trigger is the
`pendingFlushMsg` handler.

- **Delete the disk-write branch** in the `pendingFlushMsg` handler (`workspace.go:1052-1059`): remove
  the `if m.filePath != "" && !m.activeSave.InFlight { m, cmd = m.startSave(); … }` body. The
  disk-destruction vector is gone outright.
- **Move `CreateSnapshot` out of the goroutine — mandatory debounce.** Today `scheduleFlush` snapshots
  inside the goroutine (one per edit tick). Change the goroutine to return **only**
  `pendingFlushMsg{gen}`. In the handler, when `gen == m.flushGen`, fire a `snapshotCmd` (a `tea.Cmd`
  calling `CreateSnapshot`). `flushGen` is the optimistic lock — only the last edit's message
  snapshots; a typing burst yields exactly one snapshot at the end of the quiet window. (The
  switch-away force-snapshot in Fix 5 §3 is the same `snapshotCmd`, run synchronously.)
- **Keep `flushGen` and `pendingFlushMsg`** — `workspace_fuzz.go:42` exports `FlushGen`. After this
  change `pendingFlushMsg` fires `snapshotCmd` (gen matches) or is a no-op (stale). One snapshot path,
  zero disk writes.
- **Repoint the DL1 fuzz invariant to the VFS — via the driver (N2).** The current
  `CheckDataLossInvariants` (`invariant.go:703`) asserts `os.ReadFile(ActiveFilePath) == buffer` after
  every `FileSavedMsg`. Once autosave no longer writes disk this assertion never fires and silently
  passes — a dead check. Redefine DL1 to assert the **VFS** content == buffer. But `invariant`
  deliberately does **not** import `docstate` (it has only `Snapshot.DocID`, no store handle — comment
  at `invariant.go:725`), so the **driver** (which already holds `*docstate.Store`, `driver.go`)
  computes `store.Content(docID)` after each settled `snapshotCmd` and passes it in: change the
  signature to `CheckDataLossInvariants(s Snapshot, vfsContent string)` (or stash `VFSContent` on
  `Snapshot` before the check). The invariant compares `vfsContent == buffer`; `docstate` never enters
  the `invariant` package. Also fix the `os.ReadFile` absent-masking at `:708-710`
  (`if err != nil { return nil }` silently skips checks) — make a missing file a hard
  `&Violation{InvariantID: "DL1", …}`, not a silent pass. Update the seed comment at
  `session_fuzz_test.go:262` ("autosave should write to disk" → "autosave snapshots to VFS") and the
  `// Seed: autosave snapshot (DL2)` at `:216` — there is no DL2 invariant; relabel to DL1.

---

## Fix 7 — Close/quit dirty guard (BUG1)

**Files:** `pkg/ui/pages/workspace/workspace.go` (+ optional `workspace_guard.go`). *rev. 3 Fix 5,
unchanged mechanics; the VFS model clarifies one edge (untitled).* The footer needs **no change** —
its `View()` (`footer.go:216`) already renders "[S]ave [D]iscard [Esc] Cancel" and resolves Escape to
`guardOptions[len-1]` (`:128`).

1. **Guard option set with Cancel last** (resolves the Escape-discards blocker). Replace
   `quitGuardOptions` (`workspace.go:65`, currently `[Save, Discard]`) with:
   ```go
   var dataLossGuardOptions = []footer.GuardOption{
       {Key: 's', Response: footer.DataLossSave},
       {Key: 'd', Response: footer.DataLossDiscard},
       {Key: 0,   Response: footer.DataLossCancel}, // Esc → guardOptions[len-1] = Cancel
   }
   ```
   `DataLossCancel` (`footer.go:39`, dead today) goes live.

   **NUL-key guard in footer.go.** The `opt.Key` loop (`footer.go:111`) matches via
   `msg.Text == string(opt.Key)`. `string(rune(0))` produces `"\x00"` — not the empty string —
   so a literal NUL keypress would resolve to `DataLossCancel` unintentionally. Fix: in the loop,
   **skip any option where `opt.Key == 0`** before the `msg.Text` comparison. `Key == 0` is the
   Escape-only sentinel and must be reachable exclusively via the `m.keys.Cancel` branch at
   `footer.go:128`, not via a typed character. The existing "press-last-option-on-Escape" logic
   already handles it correctly; the loop guard prevents the NUL shortcut.

2. **One shared pending-action chokepoint.** Replace `pendingQuitAfterSave bool` (`:132`) with:
   ```go
   type actionKind int
   const (actionNone actionKind = iota; actionQuit; actionClose)
   type pendingDataLossAction struct {
       kind          actionKind
       closeDocID    int64    // actionClose: the doc being closed
       closePath     string   // actionClose
       nextPath      string   // actionClose
       dirtyQueue    []opentabs.TabHandle // actionQuit: remaining dirty tabs to prompt
   }
   ```
   Store `m.pendingAction` (zero = none). It survives the async Save→`FileSavedMsg` round-trip (§5.5).

3. **`requestCloseCurrent` checks live dirty (fixes BUG1, `:459`).** Before `executeClose`:
   `dirty = m.editor.Revision() != m.cleanRev && !m.viewingHelp()`. Dirty → stash
   `pendingDataLossAction{kind: actionClose, closePath: m.filePath, nextPath}` +
   `m.footer.SetGuard(GuardDirty, dataLossGuardOptions)`; clean → `executeClose` now.

4. **Generalize resolution — multi-tab quit loop.**
   - `ConfirmQuitMsg` (`:1153`): collect dirty tabs from `opentabs.DirtyTabs() []TabHandle`.
     Non-empty → `pendingDataLossAction{kind: actionQuit, dirtyQueue: []TabHandle{…}}` + raise
     the guard for `dirtyQueue[0]` with a context label; empty → quit teardown.
   - `DataLossGuardResponseMsg` (`:1164`):
     - **Save**: if `dirtyQueue[0].DocID == m.docID` → `startSave()`; else → `materializeCmd` over
       the tab's `Content(docID)` — the `current_seq` reconstruction (N3), i.e. the buffer the user
       would see, **not** raw `LatestSnapshot`. On `FileSavedMsg`, pop `dirtyQueue[0]`; empty →
       teardown; else → next.
       An **untitled** dirty tab cannot Save without a name → footer error, keep the buffer (its work
       is already durable in the VFS); offer "Save As" rather than discard.
     - **Discard**: pop and continue; for `actionClose` → `executeClose(closePath, nextPath)`.
     - **Cancel**: clear `m.pendingAction` entirely, keep buffers.
     Extract the quit teardown (disable dict, close store, `DeleteAllImagesCmd` + `tea.Quit`) —
     duplicated at `:1019`, `:1158`, `:1173` — into one helper. `runPendingAction()` dispatches
     `actionQuit` vs `actionClose`, clearing `m.pendingAction`.
   - `FileSavedMsg` (`:1011`): `if m.pendingAction.kind != actionNone { return m.runPendingAction() }`
     (guarded by the existing `RequestID` match).
   - **Non-current-tab save needs the tab's `docID` — and `opentabs` must actually carry it (N4).**
     `opentabs` is path-keyed today (`Tab{Path,Name,Pinned,Dirty,Active}`, `opentabs.go:19`; every op
     matches on `Path`), and untitled tabs all open with `path=""` — so two untitled docs collapse
     onto one tab and have no distinct key. Rework:
     1. Add `DocID int64` to `Tab`.
     2. The docID isn't known when the tab is first created (`OpenFile(path)` runs before
        `OpenPath`/`CreateScratch` returns the id). Add `opentabs.SetDocID(path, docID) Model` (or,
        cleaner, change `OpenFile` to take `(path, docID)`) and call it the instant the id resolves —
        at `FileLoadedMsg` right after `OpenPath` (`workspace.go:970-979`) and in `CreateUntitled`
        right after `CreateScratch`.
     3. **Re-key tab identity off `docID`, not path**, so multiple `path=""` untitled tabs coexist
        (this is what makes "Untitled 1, 2, 3 are distinct VFS files" true). `CloseFile`/`SetTabName`/
        `MarkDirty`/`MarkClean`/`RenameFile` — all path-matched today — must match on `docID`.
     4. `DirtyPaths() []string` becomes `DirtyTabs() []TabHandle` where `TabHandle{DocID int64; Label
        string}`, so the quit loop and non-current-tab `materializeCmd(docID, …)` need no path
        reverse-lookup. Replace any `pathToDocID map[string]int64` with the `opentabs`-resident field.

5. **Route keys to the footer while a guard is active.** Top of the `tea.KeyPressMsg` case:
   `if m.footer.InGuard() { m.footer, cmd = m.footer.Update(msg); return m, cmd }` — global keys
   (^w, ⌘S, focus) can't fire while the prompt is up. `footer.InGuard()` already exists (`:97`).

6. **Edges.** A dirty **untitled** quit *preserves* (durable in the VFS, recoverable next launch) — no
   data loss, so quit need not block on never-named docs (Decision 2); explicit *close/discard* of an
   untitled still prompts (discard removes the VFS doc). Clean tab → no guard.

---

## Resolution of blockers

| # | Sev | Blocker (verified) | Resolution |
|---|-----|--------------------|------------|
| 1 | 🔴 | Fix 6 contradictory ("identity = inode" yet untitled has no inode); "repoint events" impossible | **Architecture + Fix 3**: identity = `doc_id`; `(inode, device)` is the *disk binding*, absent for untitled. Events get `doc_id` natively in Fix 2's perm schema — no event repoint |
| 2 | 🔴 | Journal API change breaks tests + fuzz driver; not listed | Critical files lists `docstate_test.go` + `driver.go:92`; Fix 2 specifies `docID` threading |
| 3 | 🟠 | `EnsureDocument` accumulates dupes; no migration framework; FK repoint order | Fix 3: UNIQUE inode+path indexes + `user_version` migration, dedup-then-index order |
| 4 | 🟠 | "persist currentSeq" but it's RAM-only | Fix 2 §3: `documents.current_seq` (NULL=end); remove `Store.currentSeq`; explicit `BEGIN IMMEDIATE` transactions; verify pointer survives reopen |
| 5 | 🟠 | Fix 4 overstated — snapshot+append already happen; only `pendingFlushMsg` writes disk | Fix 6: delete disk-write branch; move snapshot to `snapshotCmd` (mandatory debounce via `flushGen`) |
| 6 | 🟠 | `quitGuardOptions` has no Cancel; Escape → Discard = data loss | Fix 7 §1: `dataLossGuardOptions` with Cancel **last** → Escape = Cancel |
| 7 | 🟡 | `pendingQuitAfterSave` bool can't carry close/next paths | Fix 7 §2,§4: `pendingDataLossAction{kind, closePath, nextPath, dirtyQueue}` |
| 8 | 🟡 | Non-unique `idx_documents_path` lets `EnsureDocument` create a fresh row per call | Fix 3: UNIQUE path index; migration dedups; `OpenPath` evicts stale path holders |
| 9 | 🟡 | Quit guard saves only current tab; other dirty tabs silently discarded | Fix 7 §4: multi-tab quit loop via `dirtyQueue`; non-current tabs saved from the `current_seq` reconstruction (`Content(docID)`, N3) |
| 10 | 🟡 | `RecoverDocument` returned unused `position int64` | Fix 5: trimmed to `(content, hasHistory, err)` |
| **11** | 🔴 | **(new) `createFileCmd` `os.WriteFile` truncates an existing file** — naming an untitled over a real file clobbers it; no existence check | **Fix 4 §3 bind-new**: `O_CREATE|O_EXCL` → "File already exists", keep buffer; routed through the atomic helper |
| **12** | 🟠 | **(new) untitled work is volatile** — `scheduleFlush` gates on `docID>0`, so untitled is never snapshotted; content lives in a RAM stash and dies on crash | **Fix 2 §5 + Fix 5 §3,§4**: untitled is a durable unbound VFS doc, journaled + snapshotted + recoverable; RAM stash deleted |

### Rev. 5 blockers (critic findings against rev. 4)

| # | Sev | Blocker | Resolution |
|---|-----|---------|------------|
| B1 | 🔴 | SQLite has no writable CTEs — Fix 2 §3's DML-CTE framing is a syntax error in SQLite 3.53.2 | Fix 2 §3: replaced with a multi-statement transaction (run via a pinned `sql.Tx`, **not** raw `Exec` — see N1); SQLite version recorded |
| B2 | 🔴 | `RecoverDocument` had no schema for reconstruction — snapshots were untagged so "replay backward from head" had no anchor; first ⌘Z would invert against wrong bytes | Fix 2 §4 + Fix 5 §3: `snapshots` gains `seq` column; `CreateSnapshot` captures head seq; recovery = newest `seq ≤ current_seq` snapshot + forward replay via the pure `buffer.ReplayForward` (N2 — **not** the UI `Reapply`); migration backfills `seq=0` |
| B3 | 🔴 | DL1 fuzz invariant asserts disk == buffer — invalid once autosave stops writing disk; `os.ReadFile` absent silently passes; DL2 referenced but does not exist | Fix 6: DL1 repointed to VFS (`store.Content(docID)` == buffer); absent-file masking becomes hard violation; DL2 label removed from seeds |
| B4 | 🔴 | Undo scope unspecified — `"chat"` cross-document undo; `chatDocID` missing from Model spec | Fix 2 §2 + §5: main/title → `m.docID`; chat → `m.chatDocID` singleton created at store-ready; handleUndo/Redo dispatch by focused surface |
| B5 | 🟠 | Footer "needs no change" was wrong — `Key==0` sentinel can be hit by `"\x00"` keypress via the `opt.Key` loop | Fix 7 §1: added NUL-key guard — loop skips `opt.Key == 0`; Cancel reachable only via `m.keys.Cancel` branch |
| B6 | 🟠 | No build-tagged file-identity helper; `store.go` would have to import `syscall` directly, breaking non-unix builds | Fix 3 §2: `fileid_unix.go` (`//go:build unix`) + `fileid_other.go` (`//go:build !unix`) added to Critical files; `store.go` uses `fileID()` shim |
| B7 | 🟠 | WAL presented as a correctness requirement ("makes §1.4.3 hold for every doc") — it is a silent no-op on `:memory:` and affects only durability latency | Fix 2 §1: WAL qualified as best-effort, file-backed only; no correctness property depends on it |
| B8 | 🟡 | Durability tests (untitled + `current_seq` reopen) specified on the `:memory:` fuzz path — cannot simulate a crash/restart | Verification + Critical files: durability tests pinned to file-backed `NewTestStore` path; `:memory:` tests excluded from that class |
| B9 | 🟡 | `pathToDocID map[string]int64` keyed by path — untitled tabs all share `path=""` and would collide | Fix 7 §4: tab→docID keyed by `docID`; `DirtyPaths()` → `DirtyTabs() []TabHandle`; populated from both `OpenPath` and `CreateScratch` |

### Rev. 6 blockers (critic findings against rev. 5)

| # | Sev | Blocker | Resolution |
|---|-----|---------|------------|
| N1 | 🔴 | `BEGIN IMMEDIATE … COMMIT` issued as raw `Exec` over a pooled `*sql.DB` can split across connections → journal writes run in autocommit, non-atomic (re-opens B2 under crash) | Fix 2 §1,§3: `SetMaxOpenConns(1)` + `_txlock=immediate`/`_busy_timeout` DSN; run every op through a pinned `sql.Tx` (`s.perm.Begin()`); read `RETURNING` via `QueryRow.Scan` |
| B2 | 🔴 | `RecoverDocument` (in `docstate`) specified to replay via `markdownedit.Reapply` — a UI-layer method; importing it into `docstate` is an import cycle, so the reconstruction had no compilable form | Fix 5 §3: replay via a new **pure** `buffer.ReplayForward` (modelled on `driver.go:44 replayBatch`); strike all `markdownedit.Reapply` references |
| N3 | 🔴 | `Content(docID)` defined as "wraps `LatestSnapshot`" yet used as the `current_seq` reconstruction (tab switch-back, DL1) — raw latest is wrong after an undo (desync + spurious DL1 fail) | API table + Fix 5 §4 + Fix 7 §4: `Content(docID)` **is** the `current_seq` reconstruction; `LatestSnapshot` demoted to an internal building block |
| N2 | 🔴 | DL1 redefinition had no path to VFS content — `invariant` intentionally does not import `docstate` | Fix 6: the **driver** computes `store.Content(docID)` and passes `vfsContent` into `CheckDataLossInvariants`; `invariant` stays docstate-free |
| N4 | 🟠 | `opentabs` is path-keyed; untitled tabs all share `path=""` and collide — "Untitled 1,2,3 distinct" unachievable; no `SetDocID` wiring/ordering specified | Fix 7 §4: add `Tab.DocID`; `SetDocID` after `OpenPath`/`CreateScratch`; re-key `CloseFile`/`MarkDirty`/`SetTabName`/`RenameFile` on `docID` |
| N5 | 🟠 | Async snapshot read `seq` from the DB at exec time while `content` was captured earlier — an interleaving edit mis-tags content with a newer seq (re-opens B2) | Fix 2 §4: `AppendEdit` returns `seq`; the model tracks `m.headSeq`; `snapshotCmd` co-captures `(content, seq)` in the `Update` pass; `CreateSnapshot` never re-derives seq |
| N6 | 🟠 | "doc-scoped undo is strictly safer" overclaimed — main+title share one `docID`, so a title-focused ⌘Z can step a body edit | Fix 2 §2: undo is doc-scoped across documents but stays **chronological within a doc**; focus follows the returned surface (unchanged); no `surface` scoping |
| N7 | 🟡 | `CreateScratch("chat")` every launch leaks one abandoned chat doc + its events per session, unbounded | Fix 2 §5: reuse one stable reserved chat row (sentinel path); `DELETE FROM events WHERE doc_id=chatDocID` on open |
| N8 | 🟡 | `AllEdits(docID,…)` per-doc breaks the fuzz SHADOW mirror (assumes one continuous "main" journal) | Critical files (`driver.go`): pin the fuzz session to a single doc + fixed `docID`; state the constraint |
| N9 | 🟡 | `snapshots.seq=0` backfill could collide with a real `seq=0` event under a future schema change | Fix 2 §4: pin `events.seq ≥ 1` (keep `AUTOINCREMENT`; never insert an explicit `seq=0`) |

---

## Critical files

- `pkg/ui/components/dictation/dictation.go` — drop pending edit on empty/whitespace `Accumulated`;
  leave `appliedLen` untouched (Fix 1). *(Root signal: `dictengine/start_darwin.go:65-68`.)*
- `pkg/docstate/store.go` — events → perm schema + `doc_id` + index; remove `mem`/`openMem`/
  `memSchema`; WAL (file-backed only); **`SetMaxOpenConns(1)` on every store handle + DSN
  `_txlock=immediate&_busy_timeout=5000`** (N1); `documents.current_seq`; remove `Store.currentSeq`;
  UNIQUE `(inode,device)` and `path` indexes; `user_version` migration (inode backfill + inode dedup
  + path dedup + indexes); `EnsureDocument` → `OpenPath(path) (DocRef, error)` rekeyed on inode with
  path-eviction; new `CreateScratch(name) (DocRef, error)` for untitled; reserved chat row + event
  truncation on open (N7); `Content(docID)` = `current_seq` reconstruction (N3) (Fix 2, 3).
- `pkg/docstate/journal.go` — `docID int64` on `AppendEdit`/`UndoTarget`/`RedoTarget`/`AllEdits`;
  **`AppendEdit` now returns `(seq int64, err error)`** (the head seq, for snapshot co-tagging, N5);
  `s.mem` → `s.perm`; run each op through a **pinned `sql.Tx`** (`s.perm.Begin()`, `BEGIN IMMEDIATE`
  via the `_txlock` DSN), **not** raw `Exec` (N1) — no writable CTEs (SQLite 3.53.2 rejects them);
  read `RETURNING seq` via `QueryRow.Scan`; `AND doc_id=?` on all queries; `anchor_snapshot_id`
  resolved as a scalar subquery inside `AppendEdit`'s transaction (Fix 2).
- `pkg/docstate/snapshot.go` — add `seq INTEGER NOT NULL DEFAULT 0` column to `snapshots` table;
  update `CreateSnapshot(docID, content, source string, seq int64) (int64, error)` to accept and
  persist a **passed-in** `seq` (never re-derived from the DB, N5); new `RecoverDocument(docID)
  (content string, hasHistory bool, err error)` that finds the newest snapshot with `seq ≤
  current_seq` and forward-replays events in `(snap.seq, current_seq]` via the pure
  `buffer.ReplayForward` (**not** `markdownedit.Reapply` — import cycle, B2) — crash-safe at any undo
  state (Fix 2 §4, Fix 5 §3). `Content(docID)` returns this same reconstruction (N3). Migration:
  backfill `snapshots.seq = 0` for existing rows.
- `pkg/editor/buffer` — new pure `ReplayForward(content string, batches [][]buffer.AppliedEdit) string`
  (modelled on `internal/fuzz/driver/driver.go:44 replayBatch`), used by `RecoverDocument` so
  `docstate` never imports the UI `markdownedit` (B2).
- **new** atomic-write helper (in `workspace_fileio.go` or a small `atomicwrite` package) — temp →
  `Sync` → `Rename` → parent-dir `fsync`; verbatim bytes (Fix 4). `saveFileCmd`, `createFileCmd`, and
  the image-paste write all route through it; `createFileCmd`/bind-new uses `O_CREATE|O_EXCL` →
  `FileExistsMsg`.
- `pkg/ui/pages/workspace/workspace.go` — adopt `OpenPath`/`CreateScratch`; recovery on
  `FileLoadedMsg` (`:960`); read-through-VFS on tab switch + remove `untitledContent`/`untitledTitle`
  and `showUntitled`'s disk-bypass (`:119`, `:445`); force-snapshot on switch-away; delete
  disk-autosave branch (`:1052`); mandatory debounce (`snapshotCmd`, Fix 6); `dataLossGuardOptions`
  (`:65`); `pendingDataLossAction` + `dirtyQueue` (replaces `pendingQuitAfterSave` `:132`); dirty
  check in `requestCloseCurrent` (`:459`); multi-tab quit loop (`:1153`, `:1164`, `:1011`);
  `materializeCmd` + "File already exists" handling for naming an untitled (`:914-925`, `:1431`);
  `InGuard()` early-return in `KeyPressMsg`; surface journal/snapshot errors (`:577`, `:563`); rename
  warning at the `OpenPath` call sites (`:976`, `:1046`); `nextUntitled` decoupled from disk
  (VFS-side naming, `:1484`); `journalEdit`/`scheduleFlush` skip when `m.docID == 0` (help);
  `chatDocID int64` field on `Model` (Fix 2 §5); `m.headSeq int64` tracking `AppendEdit`'s returned
  seq, co-captured with content into `snapshotCmd` (N5); `opentabs.SetDocID` calls right after
  `OpenPath`/`CreateScratch` (N4); tab→docID keyed by `docID` via `opentabs.DirtyTabs() []TabHandle`
  (replaces `pathToDocID map[string]int64`, Fix 7 §4).
- `pkg/ui/components/markdownedit/commands_image.go` — image-paste write routes through the atomic
  helper (Fix 4).
- `pkg/ui/components/opentabs/` — add `DocID int64` to `Tab`; add `SetDocID(path, docID) Model` (or
  fold the id into `OpenFile`) called the instant `OpenPath`/`CreateScratch` returns; **re-key
  `CloseFile`/`MarkDirty`/`MarkClean`/`SetTabName`/`RenameFile` on `docID`, not path**, so multiple
  `path=""` untitled tabs coexist (N4); replace `DirtyPaths() []string` with `DirtyTabs() []TabHandle`
  where `TabHandle{DocID int64; Label string}` (Fix 7 §4).
- **`pkg/docstate/docstate_test.go`** — new `docID` signatures (~15 `AppendEdit` — now also returning
  `(seq, err)`, N5; 6 `UndoTarget`, 4 `RedoTarget`); `EnsureDocument`→`OpenPath` `(DocRef, error)`;
  drop `currentSeq: math.MaxInt64` at
  `:31`; extend `TestCrashRecovery` (`:148-200`) to assert `current_seq` **and** `snapshots.seq`
  survive reopen; add tests for the UNIQUE path constraint, path-eviction on inode collision, and
  `CreateScratch` durability. **Durability tests (untitled-durability + `current_seq`-survives-
  reopen) MUST run on the file-backed path** (`NewTestStore` / temp dir), **not** the `:memory:`
  fuzz path — crash-recovery requires real disk; in-memory stores cannot simulate a restart.
  `TestCrashRecovery` for `current_seq` depends on B1's `documents.current_seq` perm column
  existing first — note the ordering. *(Tests are `package docstate` — they touch unexported fields
  directly.)*
- **`internal/fuzz/driver/driver.go`** — `AllEdits("main")` at `:92` must pass a `docID`; **pin the
  fuzz session to a single document with a fixed `docID`** (call `OpenPath`/`CreateScratch` once up
  front) so the SHADOW mirror's one-continuous-"main"-journal assumption holds (N8); after each
  settled `snapshotCmd`, compute `store.Content(docID)` and pass it into `CheckDataLossInvariants`
  (N2). The existing pure `replayBatch` (`:44`) is the template for `buffer.ReplayForward` (B2).
- **`internal/fuzz/invariant/invariant.go`** — redefine DL1 to compare a `vfsContent string`
  parameter == buffer (signature `CheckDataLossInvariants(s Snapshot, vfsContent string)`, or a
  `Snapshot.VFSContent` field) — the **driver** supplies `store.Content(docID)`, so `invariant` stays
  docstate-free (N2); remove `os.ReadFile`-absent masking (`:708-710` silently returns `nil` when the
  file is gone — change to a hard `DL1` violation); no DL2 invariant is defined and no reference to
  one may remain.
- **`pkg/ui/pages/workspace/session_fuzz_test.go`** — update seed comment at `:216` from
  `"autosave snapshot (DL2)"` to `"autosave snapshot (DL1)"` (DL2 does not exist); update seed
  comment at `:262` from `"autosave should write it to disk"` to `"autosave snapshots to VFS"`.
- **new `pkg/docstate/fileid_unix.go`** (`//go:build unix`) — returns `(inode, device uint64,
  ok bool)` via `syscall.Stat_t`; called by `OpenPath` for inode-based identity.
- **new `pkg/docstate/fileid_other.go`** (`//go:build !unix`) — returns `(0, 0, false)`; triggers
  the path-keying fallback in `OpenPath`. `store.go` must not import `syscall` directly.

---

## Verification

- **Dictation (Fix 1):** `dictation_test.go` — non-empty partial then `Accumulated:""` → no clearing
  edit, buffer retained. Integration: content + dictation from offset 0 + empty interim →
  `editor.Content()` unchanged; the next non-empty partial replaces previous text (cumulative engine).
- **Durable, per-doc DAG (Fix 2):** using `NewTestStore` (file-backed temp path — **not**
  `:memory:`): `OpenPath(A)`/`OpenPath(B)`, append to each, `Close`, reopen → events +
  `current_seq` + `snapshots.seq` survive; A's edits never appear in B's journal; `current_seq`
  at a non-head position is restored exactly. **`CreateScratch` durability** (file-backed): append
  to an untitled doc, close, reopen → its history survives and ⌘Z works (today it is lost). Extend
  `TestCrashRecovery` to assert `current_seq` is correctly preserved after simulated crash; this
  test requires the file-backed path and depends on B1's `documents.current_seq` column.
  **Transaction atomicity (N1):** a forced error mid-`AppendEdit` (between prune and insert) rolls
  back — `events` and `current_seq` stay consistent; verify ops run on a pinned `sql.Tx`, not raw
  `Exec`.
- **Identity (Fix 3):** open same file twice → one row keyed by `(inode, device)`, stable `docID`;
  rename on disk then open by the new name → same `docID`, `RenamedFrom` set, warning shown; a
  different file (new inode) at the same path → old row evicted to `path=''`, new `docID`; migration
  test: legacy `inode=0` rows + a snapshot → backfill stats, dedup by inode, dedup by path, both
  unique indexes built.
- **Materialize / no-clobber (Fix 4):** ⌘S writes via temp→fsync→rename (assert no partial file on a
  simulated mid-write failure); byte-identical round-trip (CRLF, no trailing newline, BOM preserved);
  **naming an untitled over an existing file → "File already exists", buffer kept, target file
  untouched**; ⌘S on a file changed externally → conflict surfaced, not clobbered.
- **Open restores content + DAG (Fix 5):** edit without ⌘S → restart → reopen → buffer reconstructed
  AND ⌘Z steps back (incl. a simulated dictation-clear). **Clean file** (saved, then reopened) → no
  dirty notice, but ⌘Z still steps back through the prior session's edits — assert the DAG loaded.
  Loaded content == reconstruction at `current_seq` — assert the first ⌘Z inverts against the correct
  base (no desync). Switch away from a dirty tab and back → content from the VFS, not disk. Unsaved
  **untitled** offered for recovery after a simulated crash. Two untitled buffers (`Untitled 1`,
  `Untitled 2`) coexist as distinct VFS docs/tabs with distinct `docID`s — no `path=''` collision
  (N4); closing one keeps the other.
- **No disk autosave + mandatory debounce (Fix 6):** `saveFileCmd`/materialize reached only via ⌘S /
  save-on-close; `CreateSnapshot` not called in the goroutine (only `snapshotCmd` / switch-away); a
  typing burst yields exactly one snapshot, tagged with the head `seq` matching its content (N5 — no
  mis-tag under interleaved edits). **DL1 fuzz invariant:** after `snapshotCmd` settles, the
  driver-supplied `store.Content(docID)` == buffer (VFS, not disk; `invariant` stays docstate-free,
  N2); a missing file path is a hard DL1 violation, not a silent pass. No mention of DL2 remains.
- **Close/quit guard (Fix 7):** edit→^w shows the guard; **Esc keeps the buffer** (no discard);
  Discard closes; Save closes after `FileSavedMsg`; quit (^C^C) with two dirty tabs prompts each in
  sequence; Cancel aborts the whole quit; non-current tab Save writes its `current_seq` reconstruction
  (`Content(docID)`, N3); dirty untitled quit → preserved in VFS (recoverable), not discarded.
- **Manual (`qa-instructions.md`):** dictate, pause/reset mid-stream (never wiped); ^w on a dirty
  buffer prompts and Esc keeps it; name an "Untitled" over an existing file → refused, original
  intact; `kill -9` then reopen → content + undo history restored (including an unsaved untitled); the
  on-disk `.md` changes only on ⌘S; quit with multiple dirty tabs prompts for each.
