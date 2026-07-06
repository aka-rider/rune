package docstate

import (
	"database/sql"
	"errors"
	"fmt"
	"io/fs"
	"path/filepath"
	"sync/atomic"
	"time"

	"rune/pkg/vfs"
)

// tempCounter gives siblingTempPath extra uniqueness beyond a nanosecond
// timestamp (tests that fix the Store's clock can otherwise collide within
// one process).
var tempCounter atomic.Int64

// siblingTempPath returns a same-directory temp path for target, so a
// subsequent Exchange/RenameExcl/Rename is same-volume.
func siblingTempPath(target string) string {
	dir := filepath.Dir(target)
	base := filepath.Base(target)
	n := tempCounter.Add(1)
	return filepath.Join(dir, fmt.Sprintf(".rune-materialize-%s-%d-%d.tmp", base, time.Now().UnixNano(), n))
}

// Materialize writes content to docID's bound file under a CAS (compare-and-
// swap) contract: expect is the observation the caller last read as the
// current disk fact (typically from SavedObs, captured synchronously at
// save-start). seq is the journal position content corresponds to — ALSO
// captured synchronously at save-start (co-atomic with content and expect),
// never re-derived internally at commit time: while the async write is in
// flight the user can journal new edits that advance the head, and tagging
// the save observation with that LATER head would silently claim the
// written bytes reflect edits they don't (§1.4.2/§1.4.8 — the same
// discipline the pre-v4 "mark saved at position" caller-captured seq existed
// to preserve). bindNew is the caller's explicit "a missing target is OK to
// create" intent — true for a first-time bind-new and an explicit, prompt-
// confirmed recreate-after-delete; false for an ordinary overwrite-intent
// save, where a missing target must never be silently (re)created out from
// under the user (§1.4.4 — MatResult.Missing routes the caller to an
// explicit confirmation instead). Cmd-only — every step touches disk or
// commits a short SQLite write; no DB tx is ever held open across a vfs.FS
// call (I1: the disk operations and the durable record of them are strictly
// sequenced, never interleaved inside a transaction).
//
// Implements the Part III protocol:
//  1. Unconditionally read+hash the live target (metadata is only ever a
//     hint — content-hash is the only real guard, closing G3).
//  2. Live hash != expect -> record observation(origin='probe') of the fresh
//     bytes, refuse with Conflict{Fresh}. No write.
//  3. Write a sibling temp, then vfs.Exchange(temp, target) — atomic swap;
//     the replaced bytes are never unlinked, they now sit at the temp path.
//  4. Hash the swapped-out bytes. A mismatch means a writer raced us inside
//     the window: record observation(origin='swap') of what we actually
//     replaced (the bytes physically exist — I1 holds) and refuse with
//     Conflict{Fresh}. A match proceeds to commit.
//  5. One tx: observation(origin='save', hash of the bytes WE wrote) +
//     saved_obs update + re-Bind (post-swap stat; the swap gives a fresh
//     inode). Only after that tx commits does the swapped-out temp get
//     removed (I1: never discard before the record commits).
//  6. A target that doesn't exist yet, with bindNew true, is a CREATE
//     (bind-new / recreate-after-delete): vfs.RenameExcl a sibling temp onto
//     it — atomic, no-clobber (closes G1) — instead of the read+swap dance
//     above. With bindNew false, refuse instead with MatResult.Missing —
//     never silently create.
//  7. Where Exchange is unsupported (vfs.ErrUnsupported), falls back to a
//     fresh probe+rename: the unconditional pre-write hash bounds the
//     residual TOCTOU window to microseconds (documented, degraded path —
//     I1's physical guarantee does not hold here, only the size of the race
//     window is bounded).
func (s *Store) Materialize(docID int64, path, content string, expect ObsID, seq int64, bindNew bool) (MatResult, error) {
	fsys := s.fsys()
	data := []byte(content)

	// bindNew has no valid CAS expectation to compare against at all (a
	// first-time bind-new's expect is 0 — an untitled doc never had a prior
	// disk fact — and a recreate-after-delete's is likewise unusable, since
	// the whole point is that nothing SHOULD be there). The only meaningful
	// operation is atomic create-or-refuse via RenameExcl (step 6) —
	// REGARDLESS of whether the target currently exists: RenameExcl's own
	// no-clobber check (fs.ErrExist -> Conflict, closing G1) IS the refusal
	// path for "someone's already there," so this must never fall through to
	// the step 1-2 CAS read below, which would try to read a nonexistent
	// expect observation and surface a bogus internal error instead of a
	// graceful conflict.
	//
	// bindNew resolves against the CALLER's path, never documents.path: a
	// genuine first-time bind-new's DB row still has path='' at this point
	// (CreateScratch's untitled doc — commitSave only sets it once THIS
	// create succeeds), so reading path from the DB here would reject every
	// legitimate bind-new before ever reaching RenameExcl. A recreate-after-
	// delete's DB path already equals the caller's path, so using the
	// caller's value is equivalent there too.
	if bindNew {
		resolved, err := fsys.Resolve(path)
		if err != nil {
			return MatResult{}, fmt.Errorf("materialize doc %d: resolve: %w", docID, err)
		}
		// Recreate the parent directory too: a "recreate after delete" target
		// may have lost its whole parent dir, not just the file.
		if dir := filepath.Dir(resolved); dir != "" {
			if err := fsys.MkdirAll(dir, 0o755); err != nil {
				return MatResult{}, fmt.Errorf("materialize doc %d: mkdir: %w", docID, err)
			}
		}
		return s.materializeCreate(fsys, docID, resolved, data, seq)
	}

	var dbPath string
	if err := s.perm.QueryRow(`SELECT path FROM documents WHERE id=?`, docID).Scan(&dbPath); err != nil {
		return MatResult{}, fmt.Errorf("materialize doc %d: read path: %w", docID, err)
	}
	if dbPath == "" {
		return MatResult{}, fmt.Errorf("materialize doc %d: no path bound (untitled document)", docID)
	}
	resolved, err := fsys.Resolve(dbPath)
	if err != nil {
		return MatResult{}, fmt.Errorf("materialize doc %d: resolve: %w", docID, err)
	}

	// Step 1: unconditional read+hash of the live target.
	liveData, statErr := fsys.ReadFile(resolved)
	if statErr != nil {
		if !errors.Is(statErr, fs.ErrNotExist) {
			return MatResult{}, fmt.Errorf("materialize doc %d: read target: %w", docID, statErr)
		}
		// §1.4.4: an ordinary overwrite-intent save must never silently
		// create a file the caller didn't explicitly ask to (re)create —
		// surface Missing so the caller can route to a confirmation guard.
		return MatResult{Committed: false, Missing: true}, nil
	}

	expectObs, err := s.getObservation(expect)
	if err != nil {
		return MatResult{}, fmt.Errorf("materialize doc %d: read expect observation: %w", docID, err)
	}

	// Step 2: live hash != expect -> refuse, no write.
	if hashBytes(liveData) != expectObs.BlobHash {
		fresh, err := s.recordFresh(fsys, docID, resolved, liveData, "probe")
		if err != nil {
			return MatResult{}, fmt.Errorf("materialize doc %d: record fresh observation: %w", docID, err)
		}
		return MatResult{Committed: false, Fresh: fresh}, nil
	}

	return s.materializeOverwrite(fsys, docID, resolved, data, expectObs, seq)
}

// materializeOverwrite runs steps 3-5 (or the step-7 fallback) once the
// pre-write hash has confirmed the live target still matches expect.
func (s *Store) materializeOverwrite(fsys vfs.FS, docID int64, resolved string, data []byte, expectObs Observation, seq int64) (MatResult, error) {
	temp := siblingTempPath(resolved)
	if err := fsys.WriteFile(temp, data, 0o644); err != nil {
		return MatResult{}, fmt.Errorf("materialize doc %d: write temp: %w", docID, err)
	}

	if err := fsys.Exchange(temp, resolved); err != nil {
		_ = fsys.Remove(temp) // fire-and-forget: best-effort cleanup; the temp never became the record of anything
		if errors.Is(err, vfs.ErrUnsupported) {
			return s.materializeFallbackRename(fsys, docID, resolved, data, expectObs, seq)
		}
		return MatResult{}, fmt.Errorf("materialize doc %d: exchange: %w", docID, err)
	}

	// Step 4: temp now holds what USED TO be at resolved (the displaced
	// bytes) — read+hash it. I1 is physically true here: the swap never
	// unlinked anything, so the displaced bytes are still on disk right now,
	// at temp, whether or not they match what we expected.
	displaced, err := fsys.ReadFile(temp)
	if err != nil {
		return MatResult{}, fmt.Errorf("materialize doc %d: read displaced temp: %w", docID, err)
	}
	if hashBytes(displaced) != expectObs.BlobHash {
		// F5 swap-race: a writer raced us INSIDE the atomic-swap window. The
		// swap already physically happened — OUR bytes are the ones sitting
		// at resolved right now, and the RACED writer's bytes are what got
		// displaced. Make the record match that physical reality rather than
		// discarding our own already-landed write: capture the displaced
		// bytes (I1, unchanged), THEN commit OUR write for real (saved_obs
		// moves to what's actually on disk — the CAS record stays truthful),
		// and remove the temp only after BOTH commit (never before either
		// record lands, closing the pre-fix orphaned-temp gap).
		fresh, err := s.recordFresh(fsys, docID, temp, displaced, "swap")
		if err != nil {
			return MatResult{}, fmt.Errorf("materialize doc %d: record swap observation: %w", docID, err)
		}
		saved, err := s.commitSave(fsys, docID, resolved, data, seq)
		if err != nil {
			return MatResult{}, fmt.Errorf("materialize doc %d: commit save after swap-race: %w", docID, err)
		}
		if err := fsys.Remove(temp); err != nil {
			_ = err // fire-and-forget: an orphaned temp is disk hygiene, not data safety — both durable records already committed
		}
		return MatResult{Committed: true, Raced: true, Saved: saved, Fresh: fresh}, nil
	}

	saved, err := s.commitSave(fsys, docID, resolved, data, seq)
	if err != nil {
		return MatResult{}, fmt.Errorf("materialize doc %d: commit save: %w", docID, err)
	}
	// Only after the tx commits: remove the displaced-bytes temp (I1 — never
	// discard before the record commits).
	if err := fsys.Remove(temp); err != nil {
		_ = err // fire-and-forget: an orphaned temp is disk hygiene, not data safety — the durable record already committed
	}
	return MatResult{Committed: true, Saved: saved}, nil
}

// materializeCreate runs step 6: an atomic, no-clobber RenameExcl for a
// bind-new or recreate-after-delete target.
func (s *Store) materializeCreate(fsys vfs.FS, docID int64, resolved string, data []byte, seq int64) (MatResult, error) {
	temp := siblingTempPath(resolved)
	if err := fsys.WriteFile(temp, data, 0o644); err != nil {
		return MatResult{}, fmt.Errorf("materialize doc %d: write temp: %w", docID, err)
	}
	if err := fsys.RenameExcl(temp, resolved); err != nil {
		_ = fsys.Remove(temp) // fire-and-forget: best-effort cleanup
		if errors.Is(err, fs.ErrExist) {
			// A concurrent creator raced us — record what's actually there now.
			liveData, readErr := fsys.ReadFile(resolved)
			if readErr != nil {
				return MatResult{}, fmt.Errorf("materialize doc %d: create raced, re-read: %w", docID, readErr)
			}
			fresh, err := s.recordFresh(fsys, docID, resolved, liveData, "probe")
			if err != nil {
				return MatResult{}, fmt.Errorf("materialize doc %d: record raced observation: %w", docID, err)
			}
			return MatResult{Committed: false, Fresh: fresh}, nil
		}
		if errors.Is(err, vfs.ErrUnsupported) {
			// No-hardlink/no-RENAME_EXCL filesystem (review, below-cap
			// finding): degrade to check-then-write with the residual TOCTOU
			// window documented — same acceptance as the Exchange fallback.
			// The existence check still refuses an already-present target.
			if _, statErr := fsys.Stat(resolved); statErr == nil {
				liveData, readErr := fsys.ReadFile(resolved)
				if readErr != nil {
					return MatResult{}, fmt.Errorf("materialize doc %d: create fallback re-read: %w", docID, readErr)
				}
				fresh, err := s.recordFresh(fsys, docID, resolved, liveData, "probe")
				if err != nil {
					return MatResult{}, fmt.Errorf("materialize doc %d: record create-fallback observation: %w", docID, err)
				}
				return MatResult{Committed: false, Fresh: fresh}, nil
			} else if !errors.Is(statErr, fs.ErrNotExist) {
				return MatResult{}, fmt.Errorf("materialize doc %d: create fallback stat: %w", docID, statErr)
			}
			if err := fsys.WriteFile(resolved, data, 0o644); err != nil {
				return MatResult{}, fmt.Errorf("materialize doc %d: create fallback write: %w", docID, err)
			}
			saved, err := s.commitSave(fsys, docID, resolved, data, seq)
			if err != nil {
				return MatResult{}, fmt.Errorf("materialize doc %d: create fallback commit: %w", docID, err)
			}
			return MatResult{Committed: true, Saved: saved}, nil
		}
		return MatResult{}, fmt.Errorf("materialize doc %d: renameexcl: %w", docID, err)
	}
	saved, err := s.commitSave(fsys, docID, resolved, data, seq)
	if err != nil {
		return MatResult{}, fmt.Errorf("materialize doc %d: commit save: %w", docID, err)
	}
	return MatResult{Committed: true, Saved: saved}, nil
}

// materializeFallbackRename is the step-7 degraded path for a platform where
// Exchange is unsupported: a fresh probe, then an ATOMIC write of the new
// content (F8). The pre-write hash already performed in Materialize (step 2)
// plus this re-check right before the write bounds the residual TOCTOU
// window to microseconds; unlike Exchange this path does NOT physically
// preserve the displaced bytes (fsys.WriteFile's atomic swap discards
// whatever it replaces), so I1's guarantee is honored on the happy path
// (the re-check refuses before any discard) but not against an adversarial
// writer racing inside this microsecond window — documented, accepted
// degradation for platforms without an atomic swap primitive.
//
// fsys.WriteFile is itself already atomic and durable (Disk.WriteFile
// delegates to atomicfile.Write: temp -> fsync -> rename -> parent-dir
// fsync, §1.4.1) — a hand-rolled temp+Rename here (the pre-fix shape)
// duplicated that machinery WITHOUT the parent-directory fsync, silently
// losing the durability guarantee on every non-darwin overwrite (F8).
func (s *Store) materializeFallbackRename(fsys vfs.FS, docID int64, resolved string, data []byte, expectObs Observation, seq int64) (MatResult, error) {
	liveData, err := fsys.ReadFile(resolved)
	if err != nil {
		return MatResult{}, fmt.Errorf("materialize doc %d: fallback re-read: %w", docID, err)
	}
	if hashBytes(liveData) != expectObs.BlobHash {
		fresh, err := s.recordFresh(fsys, docID, resolved, liveData, "probe")
		if err != nil {
			return MatResult{}, fmt.Errorf("materialize doc %d: record fallback observation: %w", docID, err)
		}
		return MatResult{Committed: false, Fresh: fresh}, nil
	}
	if err := fsys.WriteFile(resolved, data, 0o644); err != nil {
		return MatResult{}, fmt.Errorf("materialize doc %d: fallback write: %w", docID, err)
	}
	saved, err := s.commitSave(fsys, docID, resolved, data, seq)
	if err != nil {
		return MatResult{}, fmt.Errorf("materialize doc %d: fallback commit save: %w", docID, err)
	}
	return MatResult{Committed: true, Saved: saved}, nil
}

// recordFresh puts data's content as a blob and records an observation of it
// at path's current stat, for the Conflict{Fresh} outcomes.
func (s *Store) recordFresh(fsys vfs.FS, docID int64, path string, data []byte, origin string) (Observation, error) {
	hash, err := s.PutBlob(string(data))
	if err != nil {
		return Observation{}, fmt.Errorf("put blob: %w", err)
	}
	return s.observeFromStat(fsys, docID, path, hash, sql.NullInt64{}, origin)
}

// commitSave is step 5: ONE tx — observation(origin='save', hash of the
// bytes WE WROTE, by construction — never a re-read, closing G4) + saved_obs
// update + re-Bind (path/inode/device/kind='file', post-swap stat — the swap
// or rename gives the file a fresh inode; without this the next OpenPath
// would see "a new inode at this path" and orphan the undo DAG, §1.4.6).
// seq is the caller's save-start-captured journal position (Materialize's
// own doc comment) — NEVER re-read here: a later edit may have advanced the
// head while the disk I/O above was in flight, and re-reading now would tag
// the save observation with a position the written bytes don't actually
// reflect (§1.4.2/§1.4.8). All disk I/O for this step (the post-write stat)
// happens BEFORE the tx opens — the tx itself is pure SQLite (I1's
// no-tx-across-disk-I/O contract).
func (s *Store) commitSave(fsys vfs.FS, docID int64, resolved string, data []byte, seq int64) (Observation, error) {
	hash, err := s.PutBlob(string(data))
	if err != nil {
		return Observation{}, fmt.Errorf("put saved blob: %w", err)
	}

	// D12/D13/§1.7: ONE stat feeds BOTH the observations row (recordAdoptionTx
	// below) AND the documents rebind (inodeArg/deviceArg further down) — both
	// gated on the SAME real idOK from vfs.FileID, never reconstituted from
	// the value as `inode != 0` (the exact sentinel bug this fixes: a failed
	// post-write stat used to still write a false "inode 0" identity because
	// nothing carried "was the stat's identity actually usable" separately
	// from the zero-valued ino/dev/n locals it degraded to).
	size, mtime, inodeArg, deviceArg, nlinkArg := statIdentity(fsys, resolved)
	idOK := inodeArg.Valid

	tx, err := s.perm.Begin()
	if err != nil {
		return Observation{}, fmt.Errorf("begin: %w", err)
	}
	// Routed through recordAdoptionTx (the shared one-tx BODY) instead of
	// reimplementing "read prior saved_obs, insert, advance saved_obs"
	// inline — called directly (not via recordAdoption's self-transacting
	// wrapper) because this observation/saved_obs move must commit in the
	// SAME tx as the re-Bind below, never a second independent transaction.
	// supersedes (whatever saved_obs held immediately before this write, so
	// a later ResolveAbandon can restore it exactly) is threaded through in
	// the returned Observation.
	at := s.clock().UTC().Format(time.RFC3339Nano)
	seqVal := sql.NullInt64{Int64: seq, Valid: true}
	obs, err := s.recordAdoptionTx(tx, docID, hash, size, mtime, inodeArg, deviceArg, nlinkArg, "save", seqVal, at)
	if err != nil {
		tx.Rollback() //nolint:errcheck
		return Observation{}, err
	}
	// Re-Bind inline (same tx as the plan's step 5 specifies): free any
	// other row holding this path or this inode, then rebind.
	if _, err := tx.Exec(`UPDATE documents SET path='' WHERE path=? AND id!=?`, resolved, docID); err != nil {
		tx.Rollback() //nolint:errcheck
		return Observation{}, fmt.Errorf("free path holder: %w", err)
	}
	// documents.inode/device record "identity unknown" as NULL, never the
	// literal 0 sentinel (§1.7) — both the eviction and the rebind below are
	// NULL-safe so a failed post-write stat (idOK==false) never writes a
	// false identity into documents. Gated on idOK (the real vfs.FileID
	// validity bit captured above), not `inode != 0` (D12 — the exact
	// in-memory sentinel §1.7 forbids: a legitimate inode could theoretically
	// collide with the zero value on some platform/filesystem, and more to
	// the point, `inode != 0` silently treats "stat failed, ino defaulted to
	// 0" as "no identity" only by coincidence, not by construction).
	if idOK {
		if _, err := tx.Exec(`UPDATE documents SET inode=NULL, device=NULL WHERE inode=? AND device=? AND id!=?`, inodeArg.Int64, deviceArg.Int64, docID); err != nil {
			tx.Rollback() //nolint:errcheck
			return Observation{}, fmt.Errorf("evict inode holder: %w", err)
		}
	}
	if _, err := tx.Exec(
		`UPDATE documents SET path=?, inode=?, device=?, kind='file', last_seen_at=? WHERE id=?`,
		resolved, inodeArg, deviceArg, at, docID,
	); err != nil {
		tx.Rollback() //nolint:errcheck
		return Observation{}, fmt.Errorf("rebind: %w", err)
	}
	if err := tx.Commit(); err != nil {
		return Observation{}, fmt.Errorf("commit: %w", err)
	}

	return obs, nil
}
