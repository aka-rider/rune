package docstate

import (
	"database/sql"
	"io/fs"
	"testing"

	"rune/pkg/vfs"
)

// statFailFS forces every Stat call to fail while ReadFile/WriteFile pass
// through to the wrapped FS — isolates "file exists and is readable, but its
// stat facts are unavailable" (the realistic path to idOK/nlinkOK=false once
// ReadFile has already succeeded), used to pin the NULL-when-absent (§1.7)
// behavior for observations.inode/device/nlink (D13) without depending on a
// platform-specific stat failure.
type statFailFS struct {
	vfs.FS
}

func (statFailFS) Stat(name string) (fs.FileInfo, error) {
	return nil, fs.ErrNotExist
}

var _ vfs.FS = statFailFS{}

// obsIdentity reads back the raw inode/device/nlink NullInt64 columns for an
// observations row — the observations-table mirror of
// store_documents_test.go's identityNull helper.
func obsIdentity(t *testing.T, s *Store, obsID ObsID) (inode, device, nlink sql.NullInt64) {
	t.Helper()
	if err := s.perm.QueryRow(`SELECT inode, device, nlink FROM observations WHERE id=?`, int64(obsID)).
		Scan(&inode, &device, &nlink); err != nil {
		t.Fatalf("obsIdentity: query observation %d: %v", obsID, err)
	}
	return
}

// TestObservationIdentityNull_StatFailureRecordsNull pins the write side of
// D13: when the post-write/post-load stat fails, observations.inode/device/
// nlink are NULL (idOK/nlinkOK's own out-of-band validity), never the
// literal-0 sentinel the pre-fix code reconstituted from a dropped `ok`.
func TestObservationIdentityNull_StatFailureRecordsNull(t *testing.T) {
	s, mem := newMatTestStore(t)
	const path = "/note.md"
	if err := mem.WriteFile(path, []byte("hello"), 0o644); err != nil {
		t.Fatalf("seed file: %v", err)
	}
	s.UseFS(statFailFS{FS: mem})

	result, err := s.Load(path)
	if err != nil {
		t.Fatalf("Load: %v", err)
	}
	obs, ok, err := s.SavedObs(result.DocID)
	if err != nil || !ok {
		t.Fatalf("SavedObs: ok=%v err=%v", ok, err)
	}
	if obs.Inode.Valid || obs.Device.Valid || obs.NLink.Valid {
		t.Fatalf("stat failure: expected NULL inode/device/nlink, got Inode=%+v Device=%+v NLink=%+v", obs.Inode, obs.Device, obs.NLink)
	}
	inode, device, nlink := obsIdentity(t, s, obs.ID)
	if inode.Valid || device.Valid || nlink.Valid {
		t.Fatalf("stat failure: raw observations row has a non-NULL identity column: inode=%+v device=%+v nlink=%+v", inode, device, nlink)
	}
}

// TestObservationIdentityNull_RealStatRecordsIdentity is the control: a
// normal stat records a real, valid inode/device/nlink — confirming the fix
// didn't just make every observation NULL.
func TestObservationIdentityNull_RealStatRecordsIdentity(t *testing.T) {
	s, mem := newMatTestStore(t)
	const path = "/note.md"
	if err := mem.WriteFile(path, []byte("hello"), 0o644); err != nil {
		t.Fatalf("seed file: %v", err)
	}

	result, err := s.Load(path)
	if err != nil {
		t.Fatalf("Load: %v", err)
	}
	obs, ok, err := s.SavedObs(result.DocID)
	if err != nil || !ok {
		t.Fatalf("SavedObs: ok=%v err=%v", ok, err)
	}
	if !obs.Inode.Valid || !obs.Device.Valid || !obs.NLink.Valid {
		t.Fatalf("real stat: expected valid inode/device/nlink, got Inode=%+v Device=%+v NLink=%+v", obs.Inode, obs.Device, obs.NLink)
	}
}

// TestObservationIdentityNull_CopyForwardPreservesAbsence guards the
// copy-forward paths (ResolveAdopt/AdoptEqual, via recordAdoption): an
// adoption that re-persists a PRIOR observation's stat facts under a new
// origin must carry an unknown identity through as NULL, never silently
// promote it to a false real 0 the new row's Valid=true would claim.
func TestObservationIdentityNull_CopyForwardPreservesAbsence(t *testing.T) {
	s, mem := newMatTestStore(t)
	const path = "/note.md"
	if err := mem.WriteFile(path, []byte("hello"), 0o644); err != nil {
		t.Fatalf("seed file: %v", err)
	}
	s.UseFS(statFailFS{FS: mem})

	result, err := s.Load(path)
	if err != nil {
		t.Fatalf("Load: %v", err)
	}
	obs, ok, err := s.SavedObs(result.DocID)
	if err != nil || !ok {
		t.Fatalf("SavedObs: ok=%v err=%v", ok, err)
	}
	if obs.Inode.Valid {
		t.Fatal("setup: expected a NULL-identity observation from the stat-failed Load")
	}

	seq, err := s.CurrentSeq(result.DocID)
	if err != nil {
		t.Fatalf("CurrentSeq: %v", err)
	}
	adopted, err := s.AdoptEqual(result.DocID, obs.ID, seq)
	if err != nil {
		t.Fatalf("AdoptEqual: %v", err)
	}
	if adopted.Inode.Valid || adopted.Device.Valid || adopted.NLink.Valid {
		t.Fatalf("copy-forward adoption promoted a NULL identity to a false real value: Inode=%+v Device=%+v NLink=%+v",
			adopted.Inode, adopted.Device, adopted.NLink)
	}
	inode, device, nlink := obsIdentity(t, s, adopted.ID)
	if inode.Valid || device.Valid || nlink.Valid {
		t.Fatalf("copy-forward adoption's raw observations row is not NULL: inode=%+v device=%+v nlink=%+v", inode, device, nlink)
	}
}
