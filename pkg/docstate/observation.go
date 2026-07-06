// Package docstate is the recovery/undo/CAS-expectation store for every open
// document — the journal (events/snapshots), the disk-observation history
// (observations), and the CAS baseline (session_documents.saved_obs).
//
// # Session scoping (v10)
//
// Every journaled edit and every recorded observation carries a session_id
// (Store.sessionID, one row per Store construction — see liveness.go): two
// rune windows sharing one workDir/rune.db no longer share a single linear
// journal. AppendEdit, RecoverDocument, HasHistory, UndoPeek/RedoPeek,
// CurrentSeq, and session_documents.saved_obs are all scoped to
// (doc_id, session_id) — a session's own undo/redo/coalescing/"ours" can
// never see or be corrupted by a DIFFERENT session's edits to the same doc.
// ancestorAt's ELIGIBILITY filter (which observations may serve as an
// ancestor) is scoped the same way, for a DIFFERENT reason: a foreign
// session's save/load/resolve observation is exactly as seq-correlated and
// origin-eligible as this session's own, and must never silently become
// this session's ancestor too (that collapse is what would let a [M]erge
// discard the other session's real change with zero conflict markers
// shown). newestObservation ("theirs", below) is the ONE query that stays
// deliberately UNSCOPED: any session's disk fact is everyone's disk fact,
// and that is what lets two sessions on the same file reconcile through the
// ordinary conflict-guard/3-way-merge machinery already built for "rune vs.
// an external tool" — see CONSTITUTION.md §12.
//
// # The Adoption Contract
//
// saved_obs may move only inside commitSave (a Materialize ack, i.e. our own
// successful write), AdoptEqual (a hash-equality auto-reconciliation —
// Probe's crash-recovery self-heal, or Load's own heal-adopt on reopen), or
// a ResolveAdopt/ResolveAbandon pair (a user-driven merge/discard resolution
// and its exact-restore Esc-abort undo). An observation becomes
// ANCESTOR-ELIGIBLE (origin IN (load, save, resolve), seq NOT NULL, AND
// session_id = the querying session's own) only when the journal
// reconstruction at that seq equals the observation's own blob — i.e. the
// journal can reproduce what the observation claims was agreed at that
// position. A bare sighting (an ordinary probe, a diverged reload, an
// in-window swap capture) records history — it is never treated as
// agreement, and its seq is left NULL so ancestorAt can never select it.
//
// # Three facts, three derivations
//
// For a document at journal position pos, AS SEEN BY ONE SESSION:
//   - Ours = the journal reconstruction at pos, scoped to this session
//     (RecoverDocument).
//   - Theirs = the NEWEST observation for the document, by id, ANY origin,
//     ANY session — the freshest disk knowledge available, regardless of
//     who recorded it. Sync/syncWithTheirs classify against this, never
//     against saved_obs directly: a bare sighting (Probe, a diverged Load)
//     must still be visible to the conflict lifecycle even though it never
//     moved the CAS baseline — reading saved_obs as theirs would blind
//     load-time/idle-probe detection to exactly the external changes (or a
//     different session's saves) those paths exist to catch.
//   - Ancestor = ancestorAt(pos) — position-derived agreement, scoped to
//     THIS session's own history (see "Session scoping" above), never a
//     stored pointer, so undoing past a resolution automatically re-exposes
//     whatever divergence it settled.
//   - saved_obs (session_documents, per-session) is read by exactly ONE
//     consumer: Materialize's CAS expect. It answers "what did I (this
//     session) last write or explicitly adopt" — a stale value REFUSES a
//     clobbering write even when every UI-level guard was bypassed (defense
//     in depth), which is exactly why a bare sighting must never move it.
//
// Dirty ⟺ ours differs from ancestor (SyncBufferAhead or SyncDiverged) —
// never Kind != Clean, which would read a pure external change (DiskAhead,
// disk moved but the buffer didn't) as phantom-dirty.
package docstate

import (
	"crypto/sha256"
	"database/sql"
	"encoding/hex"
	"fmt"
	"time"

	"rune/pkg/vfs"
)

// ObsID identifies a row in observations. The zero value is never a valid id
// (ids are AUTOINCREMENT starting at 1); callers that need to represent
// "no observation" pair an ObsID with a separate bool (§1.7), exactly like
// every other id in this package — never overload 0.
type ObsID int64

// Observation is one recorded sighting of a document's disk state — load,
// save, probe, watch, resolve, or swap (Part III). Two DIFFERENT facts are
// read from this table and must never be conflated: session_documents.
// saved_obs (the CAS expectation for the next write — SavedObs) and the
// 3-way-merge ancestor (derived on the fly — ancestorAt), never a stored
// pointer.
type Observation struct {
	ID    ObsID
	DocID int64
	// SessionID (v10) is WHO recorded this sighting — required so
	// ancestorAt's eligibility filter can be scoped to "my own prior
	// agreement" (B1). newestObservation ("theirs") never filters on this;
	// it stays deliberately session-agnostic — any session's disk fact is
	// everyone's disk fact.
	SessionID int64
	BlobHash  string
	Seq       sql.NullInt64 // journal position this sighting correlates to; NULL = uncorrelated
	Size      int64
	MTime     string
	// Inode/Device/NLink are NULL (Valid=false) when the stat this
	// observation was recorded from failed or exposed no usable identity —
	// never a literal 0 sharing the column with a real value (D13/§1.7,
	// schema v9). A copy-forward adoption (ResolveAdopt/AdoptEqual, which
	// re-persists a PRIOR observation's stat facts under a new origin) must
	// thread these straight through so "identity was unknown" survives
	// instead of being reconstituted as a false real value.
	Inode      sql.NullInt64
	Device     sql.NullInt64
	NLink      sql.NullInt64
	Origin     string        // 'load'|'save'|'watch'|'probe'|'resolve'|'swap'
	Supersedes sql.NullInt64 // the saved_obs THIS row's adoption replaced; NULL if none (§1.7)
	At         string
}

// Version is a comparable fact for the Sync/Probe three-way comparison:
// a content hash, optionally correlated to the Observation it came from.
// Valid is the out-of-band presence bit (§1.7) — Hash/Obs are meaningless
// when Valid is false (e.g. a document never yet materialized to disk has
// no Theirs).
type Version struct {
	Hash  string
	Obs   ObsID
	Valid bool
}

// SyncKind discriminates the outcome of comparing buffer/saved/ancestor
// state for a document (Part III "Conflict lifecycle").
type SyncKind int

const (
	// SyncClean: the buffer matches what we believe is on disk (or there is
	// no disk fact yet — an untitled document).
	SyncClean SyncKind = iota
	// SyncBufferAhead: only the buffer has changed since the ancestor — an
	// ordinary unsaved edit; disk has not moved.
	SyncBufferAhead
	// SyncDiskAhead: only disk has changed since the ancestor — an external
	// edit landed while the buffer stayed untouched; safe to adopt.
	SyncDiskAhead
	// SyncDiverged: both the buffer and disk changed since the ancestor (or
	// there is no ancestor to reason from at all) — a real conflict.
	SyncDiverged
)

// SyncState is the result of comparing three hashes for a document: the
// buffer head (reconstructed from the journal), the disk fact we believe is
// current (saved_obs), and the derived 3-way-merge ancestor (ancestorAt).
type SyncState struct {
	Kind                   SyncKind
	Ancestor, Ours, Theirs Version
}

// MatResult is the outcome of Materialize. Committed=false means the write
// was refused: either a fresher observation was recorded instead (Fresh; the
// caller re-raises the conflict guard on it rather than losing the attempted
// write — I1: the losing side always survives as recoverable content), or the
// target simply does not exist and bindNew was false (Missing; §1.4.4 —
// Materialize never silently (re)creates a file out from under an ordinary
// overwrite-intent save, so the caller can route to an explicit
// recreate/discard confirmation instead of a surprise create).
//
// Raced (F5) is a THIRD, orthogonal shape: Committed=true AND Raced=true
// means a step-4 swap-race happened — a writer raced INSIDE the atomic-swap
// window, so the bytes the swap displaced differ from expect — but by the
// time that's discovered, OUR bytes are already physically the ones sitting
// at the target (the swap is atomic and already happened). Materialize
// therefore commits OUR write for real (Saved — the CAS record matches
// physical reality) rather than discarding it, and ALSO surfaces the raced
// writer's displaced bytes (Fresh, origin='swap', durably captured per I1) so
// the caller can offer keep-mine (already committed, nothing to do) or
// restore-theirs (a fresh write of the displaced content). Never combined
// with Missing (bindNew's create path has no CAS expectation to race
// against in this sense — see materializeCreate). Missing/Fresh-on-refusal/
// Raced are mutually exclusive discriminants, never a shared sentinel
// (§1.7): exactly one of {Missing, Raced, (!Committed && !Missing)} holds
// meaning for Fresh/Saved at a time.
type MatResult struct {
	Committed bool
	Saved     Observation // meaningful when Committed (an ordinary save OR a Raced win)
	Fresh     Observation // meaningful when (!Committed && !Missing) OR Raced (the displaced/conflicting observation)
	Missing   bool        // true when !Committed because the target doesn't exist and bindNew==false
	Raced     bool        // true when Committed==true via a step-4 swap-race (F5) — see doc comment
}

// LoadResult is the outcome of Load: the freshly-read disk bytes, the
// journal-reconstructed content (identical to DiskContent when the document
// has no history yet), and the SyncState that follows from recording this
// sighting.
type LoadResult struct {
	DocID       int64
	RenamedFrom string
	DiskContent string
	Recovered   string
	HasHistory  bool
	Sync        SyncState
	// NLink is the hard-link count Load observed on the target (0 if stat
	// failed or the platform doesn't expose it). NLink > 1 means saving
	// through this path forks the document from its other names on disk —
	// the caller surfaces a warning (WP-R4 item 6).
	NLink int
}

// hashBytes returns the lowercase hex SHA-256 of data — the same hash space
// blobs.hash and observations.blob_hash both live in.
func hashBytes(data []byte) string {
	sum := sha256.Sum256(data)
	return hex.EncodeToString(sum[:])
}

// recordObservation inserts a new observations row and returns its id.
// Pure SQLite — the caller has already done any disk I/O and blob storage
// this observation reports on (no DB tx is ever open across a vfs.FS call).
// at is the caller's own single s.clock() sample (threaded in, never
// resampled here) so a caller that also returns an Observation — e.g.
// observeFromStat below — can reuse the EXACT timestamp that was persisted,
// rather than a second, independently-drifting clock read.
//
// inode/device/nlink are sql.NullInt64 (D13/§1.7, schema v9): the caller
// builds them from vfs.FileID's/vfs.FileNLink's own out-of-band `ok`, so a
// stat failure or an unsupported platform writes a genuine SQL NULL — never
// a literal 0 sharing the column with a real identity. Every row is tagged
// with this Store's own session_id (v10) — see the Observation.SessionID
// doc comment.
func (s *Store) recordObservation(docID int64, blobHash string, seq sql.NullInt64, size int64, mtime string, inode, device, nlink sql.NullInt64, origin, at string) (ObsID, error) {
	res, err := s.perm.Exec(
		`INSERT INTO observations(doc_id, session_id, blob_hash, seq, size, mtime, inode, device, nlink, origin, at)
		 VALUES(?,?,?,?,?,?,?,?,?,?,?)`,
		docID, s.sessionID, blobHash, seq, size, mtime, inode, device, nlink, origin, at,
	)
	if err != nil {
		return 0, fmt.Errorf("record observation doc %d origin %s: %w", docID, origin, err)
	}
	id, err := res.LastInsertId()
	if err != nil {
		return 0, fmt.Errorf("record observation doc %d origin %s: last insert id: %w", docID, origin, err)
	}
	return ObsID(id), nil
}

// statIdentity stats path (through fsys) and returns the (inode, device,
// nlink) facts as sql.NullInt64 — Valid=false when the stat failed or
// exposed no usable identity/link-count, built from vfs.FileID's/
// vfs.FileNLink's own out-of-band `ok` and never reconstituted from the
// value as `inode != 0` (D12/D13/§1.7). Shared by every observation writer
// that stats a path directly (observeFromStat here; commitSave in
// materialize.go stats its own resolved target the same way for BOTH the
// observation it records and the documents rebind it does in the same tx).
func statIdentity(fsys vfs.FS, path string) (size int64, mtime string, inode, device, nlink sql.NullInt64) {
	fi, err := fsys.Stat(path)
	if err != nil {
		return 0, "", sql.NullInt64{}, sql.NullInt64{}, sql.NullInt64{}
	}
	size = fi.Size()
	mtime = fi.ModTime().UTC().Format(time.RFC3339Nano)
	ino, dev, idOK := vfs.FileID(fi)
	inode = sql.NullInt64{Int64: int64(ino), Valid: idOK}
	device = sql.NullInt64{Int64: int64(dev), Valid: idOK}
	n, nlinkOK := vfs.FileNLink(fi)
	nlink = sql.NullInt64{Int64: int64(n), Valid: nlinkOK}
	return size, mtime, inode, device, nlink
}

// observeFromStat stats path (through fsys) and records an observation of
// blobHash at that metadata. Used for the 'probe'/'swap' origins, where the
// caller already has the bytes (and their hash) in hand and only needs the
// surrounding stat facts. A stat failure degrades to a NULL identity rather
// than failing the observation — the content hash is what matters most; a
// transient stat error must not block recording capture-before-discard
// evidence (I1).
func (s *Store) observeFromStat(fsys vfs.FS, docID int64, path, blobHash string, seq sql.NullInt64, origin string) (Observation, error) {
	size, mtime, inode, device, nlink := statIdentity(fsys, path)
	// Single clock sample, reused for both the persisted row and the
	// returned Observation — a second independent sample here could drift
	// from what was actually written (the hygiene fix this function exists
	// for).
	at := s.clock().UTC().Format(time.RFC3339Nano)
	id, err := s.recordObservation(docID, blobHash, seq, size, mtime, inode, device, nlink, origin, at)
	if err != nil {
		return Observation{}, err
	}
	return Observation{
		ID: id, DocID: docID, SessionID: s.sessionID, BlobHash: blobHash, Seq: seq,
		Size: size, MTime: mtime, Inode: inode, Device: device, NLink: nlink,
		Origin: origin, At: at,
	}, nil
}

// scanObservation scans one observations row — the SELECT id, doc_id,
// session_id, blob_hash, seq, size, mtime, inode, device, nlink, origin,
// supersedes, at column list every reader in this file/sync.go uses — into
// an Observation. inode/device/nlink scan straight into their sql.NullInt64
// fields (D13/§1.7): no Go-side unwrap-to-zero — a NULL column stays NULL
// (Valid=false) in the returned Observation, so a copy-forward adoption
// elsewhere never mistakes "identity unknown" for a real 0. found=false with
// err=nil on sql.ErrNoRows — callers that treat "not found" as a genuine
// error (getObservation) wrap it themselves; callers that treat it as a
// legitimate outcome (newestObservation, ancestorAt) just return
// found=false.
func scanObservation(row *sql.Row) (Observation, bool, error) {
	var o Observation
	err := row.Scan(&o.ID, &o.DocID, &o.SessionID, &o.BlobHash, &o.Seq, &o.Size, &o.MTime, &o.Inode, &o.Device, &o.NLink, &o.Origin, &o.Supersedes, &o.At)
	if err == sql.ErrNoRows {
		return Observation{}, false, nil
	}
	if err != nil {
		return Observation{}, false, err
	}
	return o, true, nil
}

// getObservation reads one observations row by id.
func (s *Store) getObservation(id ObsID) (Observation, error) {
	o, found, err := scanObservation(s.perm.QueryRow(
		`SELECT id, doc_id, session_id, blob_hash, seq, size, mtime, inode, device, nlink, origin, supersedes, at
		 FROM observations WHERE id=?`, int64(id),
	))
	if err != nil {
		return Observation{}, fmt.Errorf("get observation %d: %w", id, err)
	}
	if !found {
		return Observation{}, fmt.Errorf("get observation %d: %w", id, sql.ErrNoRows)
	}
	return o, nil
}

// newestObservation returns docID's newest recorded observation, by id, ANY
// origin, ANY session — the "Theirs" derivation of the Three Facts (package
// doc comment): the freshest disk knowledge available, whether or not it
// ever moved saved_obs, and regardless of which session recorded it (B1 —
// this is the ONE query in this package that deliberately stays unscoped by
// session_id: a disk fact recorded by a different rune window is exactly as
// real a disk fact as one this session recorded itself). Pure SQLite.
// found=false means no observation has EVER been recorded for this document
// (an untitled scratch that was never loaded/materialized).
func (s *Store) newestObservation(docID int64) (Observation, bool, error) {
	o, found, err := scanObservation(s.perm.QueryRow(
		`SELECT id, doc_id, session_id, blob_hash, seq, size, mtime, inode, device, nlink, origin, supersedes, at
		 FROM observations WHERE doc_id=? ORDER BY id DESC LIMIT 1`,
		docID,
	))
	if err != nil {
		return Observation{}, false, fmt.Errorf("newest observation doc %d: %w", docID, err)
	}
	return o, found, nil
}

// SavedObs returns docID's current CAS expectation AS SEEN BY THIS SESSION
// (v10 — session_documents, formerly documents.saved_obs) — the observation
// this session believes reflects what is physically on disk right now.
// found=false means either this session has never materialized/loaded this
// document (an untitled scratch, or one whose kind is not yet bound to a
// path) OR — new under session-scoping — this session has simply never
// adopted anything for this docID yet, even if a DIFFERENT session has.
func (s *Store) SavedObs(docID int64) (Observation, bool, error) {
	return s.savedObsFor(s.sessionID, docID)
}

// savedObsFor is SavedObs's engine, parameterized by sessionID — shared by
// SavedObs (always s.sessionID) and findInheritableDraft's cross-session
// baseline check (load.go, v10 review fix): whether disk has moved since a
// DIFFERENT, already-identified session's own last-known agreement, which
// is exactly what decides whether that other session's reconstruction is
// genuine unsaved work or just a stale mirror of a disk state a later,
// unrelated change has since superseded.
func (s *Store) savedObsFor(sessionID, docID int64) (Observation, bool, error) {
	var obsID sql.NullInt64
	err := s.perm.QueryRow(`SELECT saved_obs FROM session_documents WHERE session_id=? AND doc_id=?`, sessionID, docID).Scan(&obsID)
	if err == sql.ErrNoRows {
		return Observation{}, false, nil
	}
	if err != nil {
		return Observation{}, false, fmt.Errorf("saved obs doc %d session %d: %w", docID, sessionID, err)
	}
	if !obsID.Valid {
		return Observation{}, false, nil
	}
	obs, err := s.getObservation(ObsID(obsID.Int64))
	if err != nil {
		return Observation{}, false, fmt.Errorf("saved obs doc %d session %d: %w", docID, sessionID, err)
	}
	return obs, true, nil
}

// setSavedObs updates docID's CAS expectation (for THIS session — v10) to
// obs directly, with no accompanying observation insert and no supersedes
// bookkeeping. Production code never calls this standalone: every adoption
// (commitSave, ResolveAdopt, AdoptEqual, recordAdoption) moves saved_obs
// INSIDE the same tx as the observation row it accompanies, so the
// supersedes chain stays intact and a reader can never observe one moved
// without the other. This bare helper exists for tests that need to plant a
// saved_obs value without a full adoption (e.g. markSavedNow) — a bare
// sighting (a probe/watch observation) must NEVER move the CAS baseline
// through this or any other path; only our own successful write or an
// explicit adoption does (the Adoption Contract, package doc comment).
func (s *Store) setSavedObs(docID int64, obs ObsID) error {
	if _, err := s.perm.Exec(
		`INSERT INTO session_documents(session_id, doc_id, saved_obs) VALUES(?,?,?)
		 ON CONFLICT(session_id, doc_id) DO UPDATE SET saved_obs=excluded.saved_obs`,
		s.sessionID, docID, int64(obs),
	); err != nil {
		return fmt.Errorf("set saved_obs doc %d: %w", docID, err)
	}
	return nil
}

// ancestorAt, Sync, syncWithTheirs, and classifySync — the Conflict lifecycle
// comparison — live in sync.go (§1.6, split out of this file to stay under
// the 500-LoC limit).
//
// ResolveAdopt, ResolveAbandon, recordAdoption, and AdoptEqual — the
// operations that move saved_obs (the Adoption Contract, package doc
// comment) — live in adopt.go.
