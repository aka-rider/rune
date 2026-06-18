# Data-loss remedy — executable plan (rev. 2)

> **Deliverable of this task:** replace the body of `plan-data-loss-remedy.md` with the
> corrected plan below. The critique judged rev. 1 "not executable as written" (7 structural
> blockers, 2 High). Every blocker is resolved here and grounded in the verified source.

---

## Context

`rune` lost user content through three interacting defects, all confirmed against the code:

1. **Dictation clear (root trigger).** When the speech engine resets a segment it emits
   `PartialTranscriptionMsg{Accumulated: ""}` (`dictengine/start_darwin.go:65-68`). The dictation
   component turns that empty string into `pendingEdit{start, end: startOff+appliedLen, text: ""}`
   (`dictation/dictation.go:92-100`), which the workspace applies verbatim as
   `m.editor.ReplaceRange(s, e, "")` (`workspace.go:664-669`) — deleting the committed region down
   to empty. No guard exists at any layer.
2. **Disk-destruction vector.** Every main-surface edit schedules an autosave: `journalEdit`
   (`workspace.go:571-585`) → `scheduleFlush` (`551-569`) → `pendingFlushMsg` handler (`1052-1059`)
   → `startSave` → `saveFileCmd` → `os.WriteFile` (`workspace_fileio.go:79`), `flushDelay` later.
   A dictation-cleared buffer therefore overwrites the real `.md` on disk with `"\n"`.
3. **No recovery, no close guard.** `documents.path` is not unique (only the non-unique
   `idx_documents_path`, `store.go:41`); the undo/redo journal lives in a `:memory:` DB
   (`store.go:65-80`, `openMem` `:114`) and dies on exit; `FileLoadedMsg` (`960-988`) never reads a
   snapshot back; and `requestCloseCurrent` (`459-466`) closes dirty buffers with no prompt (BUG1).

**Chosen model (unchanged):** SQLite is the recovery layer (durable snapshot + undo/redo DAG); the
`.md` on disk is written only on explicit ⌘S (`workspace.go:707`) plus save-on-close. Autosave never
touches disk. Fixes are ordered by causal/architectural depth.

---

## Fix 1 — Dictation must never delete committed text (root trigger)

**File:** `pkg/ui/components/dictation/dictation.go` (handler at `:92-100`).

- In the `dictengine.PartialTranscriptionMsg` / `FinalTranscriptionMsg` case: if the incoming text
  is empty or whitespace-only, **drop the pending edit** (leave `m.pending`/`m.hasPending`/`m.appliedLen`
  untouched) and return. An empty interim resets the engine's segment, not the document — never emit
  a replace that collapses a non-empty applied region to empty.
- Recompute `appliedLen` from the text actually applied and clamp `[start, end]` to the live buffer
  length. `appliedLen` and `startOff` are **byte** offsets feeding `ReplaceRange`/`buffer.Edit`
  (which are byte-indexed), so `len(text)` here is correct — do **not** switch these to
  `utf8.RuneCountInString` (CLAUDE.md §1.5 applies to *display* math, not buffer offsets).
- If the buffer was reloaded mid-session (desync — e.g. `end > bufferLen`), cancel the pending edit
  rather than replace a stale range. Guard stays in the producer (dictation component), not in
  `ReplaceRange`.

*Status: the critic confirmed Fix 1's diagnosis and location are correct; this is the one fix that
needed no structural change.*

---

## Fix 2 — Durable, per-document undo/redo DAG (core data-model fix)

**Files:** `pkg/docstate/store.go`, `pkg/docstate/journal.go`, `pkg/docstate/snapshot.go`.

The `events` table is ephemeral (`:memory:`) and global (keyed only by `surface`). Make it durable
and document-scoped.

1. **Move events into the perm DB; eliminate the `:memory:` journal.**
   - Add the `events` table to `permSchema` with a new `doc_id INTEGER NOT NULL REFERENCES documents(id)`
     column and `CREATE INDEX idx_events_doc ON events(doc_id, seq)`. Keep the existing columns
     (incl. `anchor_snapshot_id`, `is_undo_stop`).
   - Delete the `mem *sql.DB` field, `openMem`, `memSchema`, `initMemSchema`. Route every event query
     to `s.perm`. `Close()` closes only `perm`. `OpenInMemory` (used by fuzz/tests) already makes
     `perm` a `:memory:` DB, so ephemeral runs stay ephemeral with no extra code.
   - **No data migration for events:** they never existed in perm before, so `CREATE TABLE IF NOT
     EXISTS events` simply adds an empty table to existing DBs. (This removes the impossible "repoint
     events" step from rev. 1's Fix 6 — events were not in perm and not doc-scoped.)
   - Enable WAL on perm (`PRAGMA journal_mode=WAL` in `openPerm`) so per-edit durability is sub-ms.
   - The `events.doc_id` FK requires every journaled buffer to have a `documents` row. Untitled
     buffers get an **ephemeral row** (Fix 6 §4) so they keep working; `docID == 0` surfaces (help)
     must not journal (`journalEdit` already short-circuits when there are no edits — extend it to
     skip when `m.docID == 0`).

2. **Scope the journal by `doc_id`.** Add `docID int64` as the first parameter to `AppendEdit`,
   `UndoTarget`, `RedoTarget`, `AllEdits`. Add `AND doc_id=?` to every event query (truncate,
   coalesce, undo/redo target, all-edits) and `doc_id` to the INSERT. Each file now owns an isolated
   undo history; `surface` stays a secondary discriminator within a doc.

3. **Persist the undo pointer per document (resolves the "currentSeq is RAM-only" blocker).**
   - Remove the single `Store.currentSeq int64` field (and the `currentSeq: math.MaxInt64` initializer
     from all 5 `&Store{…}` literals — see Critical files).
   - Add `current_seq INTEGER` to the `documents` table: `NULL` = "at end of history" (the old
     `math.MaxInt64` sentinel), any value `N` = "undone to just before N". `AppendEdit`/`UndoTarget`/
     `RedoTarget` read and write `documents.current_seq` for their `docID` in the same statement batch
     as the event mutation, so the pointer is durable for free and survives reopen.

4. **Coordinate snapshots with the DAG.** When appending an event, set `anchor_snapshot_id` to the
   doc's latest snapshot id so the buffer = anchor-snapshot content + events after the anchor.
   Recovery content (Fix 3) reads `LatestSnapshot(docID)` (full content, already implemented at
   `snapshot.go:84`); the events provide the steppable undo trail. Snapshot as a periodic compaction
   base, not the only recovery source.
   - *Optional refinement (folds in Fix 4's debounce):* gate snapshot creation by `flushGen` so a
     burst of keystrokes produces one snapshot at the end of the quiet window instead of one per
     keystroke (today `scheduleFlush` snapshots on every edit). See Fix 4.

5. **Concurrency / fail-fast.** Journal writes stay synchronous against WAL'd SQLite (bounded sub-ms
   local writes; keeps undo/redo synchronous as today). This is a deliberate, documented exception to
   CLAUDE.md §5.3's "file I/O → Cmd" — justified because the writes are bounded local ops and making
   undo async would be a large, risky refactor. If profiling shows event-loop stalls, move
   `AppendEdit` (writes only) into a Cmd. Data-integrity failures fail-fast and surface (§1.3) — the
   current `_ = err` swallow in `journalEdit` (`workspace.go:577`) must be replaced with a surfaced
   error.

---

## Fix 3 — Recovery restores BOTH buffer and DAG on open (the directive)

**Files:** `pkg/ui/pages/workspace/workspace.go` (`FileLoadedMsg`, `:960-988`), `pkg/docstate/*`.

- On open, after `EnsureDocument(path)` returns a **stable `doc_id` (from Fix 6)**, fire a
  workspace-owned recovery Cmd (SQLite reads are I/O → Cmd, §5.3/§6.2; capture `store`, `docID`,
  `diskContent` into locals, §5.5). The Cmd calls a new docstate read API
  `RecoverDocument(docID) (content string, position int64, hasHistory bool, err error)` that returns
  `LatestSnapshot(docID)` as content (and the persisted `current_seq` as position), then returns a
  `RecoveredMsg{Path, Content, …}` defined in the workspace package (§2.4 — the workspace owns it).
- Handler: if `hasHistory` and reconstructed content **differs** from the just-loaded disk content,
  `SetContent(recovered)` + `MarkDirty` (do **not** write disk — the `.md` is untouched until ⌘S) and
  show the footer notice "Recovered unsaved changes — ⌘S to keep, ⌘Z to step back". The undo/redo DAG
  is restored implicitly: events persist in perm keyed by `docID` and `current_seq` is loaded, so ⌘Z
  steps back through history (incl. undoing the dictation-clear) — recovery is editable history, not a
  frozen snapshot.
- **Guard false positives:** skip recovery when reconstructed == disk, or when `hasHistory` is false;
  never silently overwrite disk.

*(Cross-reference fixed: rev. 1 said "doc_id from Fix 5" — identity is Fix 6, not the close guard.)*

---

## Fix 4 — Autosave is SQLite-only; delete the disk autosave vector

**Files:** `pkg/ui/pages/workspace/workspace.go`, `workspace_fileio.go`.

The critique is precise here: the SQLite snapshot **already** happens in `scheduleFlush` (`:561`,
`CreateSnapshot(docID, content, "local")`) and the event **already** appends in `journalEdit`
(`:577`). The *only* disk-write trigger is the `pendingFlushMsg` handler.

- **Delete only the disk-write branch** inside the `pendingFlushMsg` handler (`workspace.go:1052-1059`):
  remove the `if m.filePath != "" && !m.activeSave.InFlight { m, cmd = m.startSave(); … }` body. This
  deletes the disk-destruction vector outright.
- **Do NOT add a second snapshot.** `scheduleFlush` keeps creating the one SQLite snapshot. The only
  remaining `startSave` callers are the explicit ⌘S handler (`:707-711`) and the save-on-close guard
  (Fix 5) — both user-initiated.
- **Keep `flushGen` and `pendingFlushMsg`.** `workspace_fuzz.go:42` exports `FlushGen: m.flushGen`;
  the field and message type must remain so the fuzz inspector stays consistent. After this change the
  `pendingFlushMsg` case either becomes a no-op or (recommended, the Fix 2 §4 refinement) fires a
  debounced `snapshotCmd` gated by `gen == m.flushGen`, moving `CreateSnapshot` out of the per-edit
  timer and into the debounce point so a typing burst yields one snapshot instead of one per keystroke.
  Either way, exactly one snapshot path exists and no disk write remains.

---

## Fix 5 — Close/quit dirty guard (BUG1)

**Files:** `pkg/ui/pages/workspace/workspace.go` (+ optional `workspace_guard.go`). The footer needs
**no change** — its `View()` (`footer.go:216-224`) already renders "[S]ave [D]iscard [Esc] Cancel"
and it already resolves Escape to `guardOptions[len-1]` (`footer.go:128-129`). The bug is entirely in
the workspace's option set and routing.

1. **Guard option set with Cancel last (resolves the Escape-discards blocker).** Replace
   `quitGuardOptions` (`workspace.go:65-68`, currently `[Save, Discard]`) with:
   ```go
   var dataLossGuardOptions = []footer.GuardOption{
       {Key: 's', Response: footer.DataLossSave},
       {Key: 'd', Response: footer.DataLossDiscard},
       {Key: 0,   Response: footer.DataLossCancel}, // Esc → guardOptions[len-1] = Cancel
   }
   ```
   `Key: 0` (NUL) can't be typed, so Cancel is reachable only via Escape — matching the rendered
   "[Esc] Cancel". `DataLossCancel` (`footer.go:39`, dead today) becomes live.

2. **One shared pending-action chokepoint (resolves the "bool can't carry paths" blocker).** Replace
   the `pendingQuitAfterSave bool` field (`:132`) with:
   ```go
   type actionKind int
   const (actionNone actionKind = iota; actionQuit; actionClose)
   type pendingDataLossAction struct { kind actionKind; closePath, nextPath string }
   ```
   Store `m.pendingAction pendingDataLossAction` (zero value = none). It survives the async
   Save→`FileSavedMsg` round-trip (§5.5).

3. **`requestCloseCurrent` checks live dirty (fixes BUG1, `:459-466`).** Before `executeClose`:
   dirty = `m.editor.Revision() != m.cleanRev && !m.viewingHelp()` (same predicate as `syncDirty`,
   `:632-642`). If dirty → stash `pendingDataLossAction{kind: actionClose, closePath: m.filePath,
   nextPath}` and `m.footer.SetGuard(GuardDirty, dataLossGuardOptions)`; if clean → `executeClose` now.

4. **Generalize the two resolution sites.**
   - `ConfirmQuitMsg` (`:1153-1162`): on dirty, set `pendingDataLossAction{kind: actionQuit}` and raise
     the guard with `dataLossGuardOptions`; on clean, run the quit teardown.
   - `DataLossGuardResponseMsg` (`:1164-1180`): `Save` → set action (already stashed) then `startSave`
     and run the action on `FileSavedMsg`; `Discard` → run the action now (discarding); `Cancel` →
     clear `m.pendingAction`, keep the buffer. Extract the quit teardown (disable dict, close store,
     `DeleteAllImagesCmd` + `tea.Quit`) — currently duplicated at `:1019-1022`, `:1158-1162`,
     `:1173-1177` — into one helper, and a `runPendingAction()` that dispatches `actionQuit` vs
     `actionClose` (→ `executeClose(closePath, nextPath)`), clearing `m.pendingAction`.
   - `FileSavedMsg` (`:1011-1024`): replace the `pendingQuitAfterSave` block with
     `if m.pendingAction.kind != actionNone { return m.runPendingAction() }` (guarded by the existing
     `RequestID` match at `:1012`, so a plain ⌘S save is a no-op).

5. **Route keys to the footer while a guard is active.** Add at the top of the `tea.KeyPressMsg` case:
   `if m.footer.InGuard() { m.footer, cmd = m.footer.Update(msg); return m, cmd }` — so global keys
   (^w, ⌘S, focus) can't fire while the prompt is up; the footer consumes the key and emits the
   response (`footer.go:110-135`). `footer.InGuard()` already exists (`footer.go:97`).

6. **Edges.** Clean tab / sole untitled (`:461-463`) → no guard. `Save` on an empty-path buffer can't
   write → footer error + keep buffer + clear action (never discard).

---

## Fix 6 — Stable document identity: inode+device primary (prereq for per-doc recovery)

**File:** `pkg/docstate/store.go`. Resolves the rev. 1 contradiction by committing to the footnote's
model:

> **Identity = `(inode, device)`.** A file is opened *by filename*, but identity is keyed on its
> `(inode, device)` read via `stat`. The filename is **secondary metadata**: if the inode matches an
> existing row whose stored path differs, the file was renamed while rune was closed — adopt the new
> path and surface a warning `file was renamed: <oldPath> → <newPath>`. This makes recovery robust to
> rename-while-closed (which fsnotify reports late) and to path reuse.

1. **Unique key on `(inode, device)`, not path.** `CREATE UNIQUE INDEX IF NOT EXISTS
   idx_documents_inode ON documents(inode, device) WHERE inode != 0`. Keep the **non-unique**
   `idx_documents_path` for the degraded-fallback lookup only (path is no longer an identity key).
   The partial `WHERE inode != 0` lets inode-less rows (untitled buffers; see §4) coexist.

2. **`EnsureDocument(path)` keys on inode.** Confirmed `EnsureDocument` is only called for files that
   exist on disk (`FileLoadedMsg` `:982` and `StoreReadyMsg` `:1045-1046`, both after the file was
   read from disk), so `stat` always yields a real inode there.
   ```
   stat(path) -> (inode, device)              // fi.Sys().(*syscall.Stat_t).Ino/.Dev (unix/darwin)
   if stat fails or inode == 0:                // race: file vanished -> degraded fallback
       lookup/insert by path; return
   row = SELECT id, path FROM documents WHERE inode=? AND device=?
   if row found:
       if row.path != path: renamedFrom = row.path; UPDATE documents SET path=?, last_seen_at=? …
       else:                UPDATE documents SET last_seen_at=? …
       return {ID: row.id, RenamedFrom: renamedFrom}
   else:
       INSERT documents(path, inode, device, created_at, last_seen_at); return {ID: newID}
   ```
   Change the signature to `EnsureDocument(path string) (DocumentRef, error)` where
   `DocumentRef{ID int64; RenamedFrom string}`. The workspace surfaces the rename warning via
   `footer.ShowErrorMsg` when `RenamedFrom != ""` (message text built in the workspace, §2.4). The two
   workspace call sites and any test calls must adopt the new return shape (see Critical files).

3. **Minimal migration framework (none exists today — confirmed: no `user_version`/`ALTER`/migration
   code).** In `initPermSchema`, after `CREATE TABLE IF NOT EXISTS`, run `migrate(db)` gated on
   `PRAGMA user_version`. Step 1 (one-time, then bump to 1), in a transaction respecting
   `foreign_keys=ON` (`store.go:85`, `:102`) and the `snapshots.doc_id → documents.id` FK (`:50`):
   1. **Backfill** `(inode, device)` for existing rows by `stat`-ing each row's `path` (best-effort;
      if the file is gone, leave `0` — that row stays out of the unique index as an orphan). Existing
      rows have `inode = NULL/0` today (`:36-37` never written).
   2. Among rows now sharing a non-zero `(inode, device)`: survivor = `MIN(id)`;
      `UPDATE snapshots SET doc_id = survivor WHERE doc_id IN (dups)` (repoint **before** delete);
      `DELETE FROM documents WHERE id IN (dups)`.
   3. `CREATE UNIQUE INDEX … ON documents(inode, device) WHERE inode != 0` (must run **after** dedup).
   (Only `snapshots` needs repointing — the perm `events` table is brand-new and empty, so no event FK
   violation is possible.)

4. **Untitled buffers get an ephemeral row** (so eliminating the `:memory:` journal in Fix 2 does not
   regress untitled undo). `CreateUntitled` inserts a fresh `documents` row with `path=''`,
   `inode=0`, `device=0` and sets `m.docID` to it; untitled edits journal under that id. These rows
   are excluded from the unique index (`inode = 0`), are never recovery targets, and may be GC'd on
   open (e.g. delete `path='' AND inode=0` rows older than the current session). `showUntitled`/
   `showHelp` keep `docID` semantics consistent (help stays `docID=0`, never journaled).

5. **Residual edge (documented).** Inode reuse — fileA deleted, fileB later assigned fileA's inode on
   the same device — would attribute fileA's history to fileB; the rename warning fires (paths differ)
   so the user is notified. Tighter disambiguation (size/created_at corroboration) is future hardening.

---

## Resolution of the 7 blockers

| # | Sev | Blocker (verified) | Resolution |
|---|-----|--------------------|------------|
| 1 | 🔴 | Fix 6 contradictory; "repoint events" impossible (events in `:memory:`, not doc-scoped) | Fix 6: single identity model — **`(inode, device)` primary**, filename secondary (rename → adopt + warn); events get `doc_id` natively in Fix 2's perm schema — **no event repoint**, no migration for events |
| 2 | 🔴 | Journal API change breaks tests + fuzz driver; not listed | Critical files now lists `docstate_test.go` (~25 call sites + `&Store{…}` at `:31`) and `internal/fuzz/driver/driver.go:92`; Fix 2 specifies the `docID` threading |
| 3 | 🟠 | `EnsureDocument` accumulates dupes; no migration framework; FK repoint order | Fix 6: partial UNIQUE index + idempotent dedup + `user_version` migration with repoint-before-delete |
| 4 | 🟠 | "persist currentSeq" but it's RAM-only, no table/column; test never asserts it | Fix 2 §3: `documents.current_seq` column (NULL = end); remove `Store.currentSeq`; verification asserts pointer survives reopen |
| 5 | 🟠 | Fix 4 overstated — snapshot+append already happen; only `pendingFlushMsg` writes disk | Fix 4: delete *only* the disk-write branch; forbid a second snapshot; keep `flushGen` for `workspace_fuzz.go:42` |
| 6 | 🟠 | `quitGuardOptions` has no Cancel; Escape → Discard = data loss; `DataLossCancel` dead | Fix 5 §1: `dataLossGuardOptions` with Cancel **last** → Escape = Cancel; footer already renders/handles it |
| 7 | 🟡 | `pendingQuitAfterSave` bool can't carry close/next paths; two save-paths race | Fix 5 §2,§4: replace bool with `pendingDataLossAction{kind, closePath, nextPath}`; one resolution path |

---

## Critical files

- `pkg/ui/components/dictation/dictation.go` — drop pending edit on empty/whitespace interim (Fix 1).
  *(Root signal originates in `dictengine/start_darwin.go:65-68`; the guard belongs in the consumer.)*
- `pkg/docstate/store.go` — events → perm schema + `doc_id` + index; remove `mem`/`openMem`/`memSchema`;
  WAL; `documents.current_seq`; remove `Store.currentSeq` field; UNIQUE index on `(inode, device)`;
  `user_version` migration (inode backfill + dedup + snapshot repoint); `EnsureDocument` rekeyed on
  inode, new return `(DocumentRef{ID, RenamedFrom}, error)`; ephemeral untitled-doc helper (Fix 2, 6).
- `pkg/docstate/journal.go` — `docID int64` param on `AppendEdit`/`UndoTarget`/`RedoTarget`/`AllEdits`;
  `s.mem` → `s.perm`; `AND doc_id=?` on all queries; read/write `documents.current_seq` (Fix 2).
- `pkg/docstate/snapshot.go` — `anchor_snapshot_id` coordination + new `RecoverDocument(docID)` read
  API (Fix 2, 3).
- `pkg/ui/pages/workspace/workspace.go` — recovery on `FileLoadedMsg` (`:960`); delete disk-autosave
  branch (`:1052-1059`); `dataLossGuardOptions` (`:65`); `pendingDataLossAction` (replaces
  `pendingQuitAfterSave` `:132`); dirty check in `requestCloseCurrent` (`:459`); generalize
  `ConfirmQuitMsg` (`:1153`) + `DataLossGuardResponseMsg` (`:1164`) + `FileSavedMsg` (`:1011`);
  `InGuard()` early-return in `KeyPressMsg`; surface the journal error at `:577`; adopt the new
  `EnsureDocument` return + rename warning at the two call sites (`:982`, `:1046`); `CreateUntitled`
  allocates an ephemeral untitled doc row; `journalEdit` skips when `m.docID == 0` (Fix 2, 3, 4, 5, 6).
- `pkg/ui/pages/workspace/workspace_fileio.go` — `saveFileCmd`/`loadFileCmd`; new recovery/snapshot
  Cmds (Fix 3, 4).
- **`pkg/docstate/docstate_test.go`** — update for the new `docID` signatures: ~15 `AppendEdit`, 6
  `UndoTarget`, 4 `RedoTarget` call sites; every `EnsureDocument` call adopts the new
  `(DocumentRef, error)` return; the `&Store{… currentSeq: math.MaxInt64}` literal at `:31` (drop the
  field); extend `TestCrashRecovery` (`:148-200`) to assert `current_seq` survives reopen.
  *(Tests are `package docstate` — they touch unexported fields directly.)*
- **`internal/fuzz/driver/driver.go`** — `AllEdits("main")` at `:92` must pass a `docID` (EnsureDocument
  a fixed path first); update any other journal calls. **Listed because it is the second compile-break
  site omitted from rev. 1.**

---

## Verification

- **Dictation (Fix 1):** `dictation_test.go` — non-empty partial then `Accumulated:""` → assert no
  clearing edit and buffer text retained. Integration: content + dictation from offset 0 + empty
  interim → `editor.Content()` unchanged.
- **Durable, per-doc DAG (Fix 2):** open store, `EnsureDocument(A)`/`EnsureDocument(B)`, append edits to
  each, `Close`, reopen → events + `current_seq` survive; edits to A never appear in B's journal.
- **Recovery (Fix 3):** edit (snapshot+DAG) without ⌘S → simulate restart → reopen → buffer
  reconstructed AND ⌘Z undoes to the prior state (incl. undoing a simulated dictation-clear); assert
  no restore when reconstructed == disk.
- **No disk autosave (Fix 4):** assert `saveFileCmd` is reached only via ⌘S / save-on-close; assert
  exactly one snapshot path and that a typing burst (with the optional debounce) yields one snapshot.
- **Close guard (Fix 5):** Key-Routing + Async-IO — edit→^w shows the guard; **Esc keeps the buffer**
  (no discard); Discard closes; Save closes after `FileSavedMsg`; quit (^C^C) still guards via the same
  shared path; `Save` on empty-path buffer → error + buffer kept.
- **Identity (Fix 6):** open same file twice → single row keyed by `(inode, device)`, stable `docID`;
  rename the file on disk then open by the new name → same `docID`, `RenamedFrom` set, warning shown;
  a different file (different inode) at the same path → new `docID` (old history not attributed);
  migration test: legacy rows with `inode=0` + a snapshot → backfill stats the path, dedups by inode,
  repoints the snapshot, builds the unique index; untitled buffer → ephemeral `inode=0` row, journals
  + undoes, excluded from the unique index.
- **Manual (`qa-instructions.md`):** dictate, pause/reset mid-stream (never wiped); ^w on a dirty
  buffer prompts and Esc keeps it; `kill -9` then reopen → content + undo history restored; on-disk
  `.md` only ever changes on ⌘S.
