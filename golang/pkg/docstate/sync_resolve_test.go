package docstate

import (
	"testing"

	"rune/pkg/editor/buffer"
)

// TestSync_UndoPastResolve_ReDivergesThenRedoCleans is the plan's
// undo-unwind validation: journal a resolve (ResolveAdopt at seq N), undo
// below N -> Sync == Diverged; redo above N -> Clean. This is the
// structural replacement for the old handleMergeUnwindRead's bespoke
// re-detection logic — the ancestor is derived from journal position, never
// a stored pointer, so undoing past a resolution automatically re-exposes
// the divergence it settled.
func TestSync_UndoPastResolve_ReDivergesThenRedoCleans(t *testing.T) {
	s, mem := newMatTestStore(t)
	const path = "/note.md"
	if err := mem.WriteFile(path, []byte("original"), 0o644); err != nil {
		t.Fatalf("seed file: %v", err)
	}

	loaded, err := s.Load(path)
	if err != nil {
		t.Fatalf("Load: %v", err)
	}
	docID := loaded.DocID

	// An external change lands on disk; a fresh probe captures it (this is
	// what a [D]iscard/[M]erge resolution reads as "theirs").
	const theirsContent = "external change"
	if err := mem.WriteFile(path, []byte(theirsContent), 0o644); err != nil {
		t.Fatalf("external change: %v", err)
	}
	preResolveState, err := s.Probe(docID)
	if err != nil {
		t.Fatalf("Probe: %v", err)
	}
	if preResolveState.Kind != SyncDiverged && preResolveState.Kind != SyncDiskAhead {
		t.Fatalf("setup: expected a real divergence before resolving, got %v", preResolveState.Kind)
	}
	freshObs := preResolveState.Theirs.Obs

	// [D]iscard resolution: journal a ReplaceAll turning the buffer into
	// theirs (mirrors applyDiscardConflict), then ResolveAdopt correlates
	// the fresh observation to that edit's seq.
	edits := []buffer.AppliedEdit{{Start: 0, End: len("original"), Deleted: "original", Insert: theirsContent}}
	resolveSeq, err := s.AppendEdit(docID, edits, nil, nil)
	if err != nil {
		t.Fatalf("AppendEdit (resolve): %v", err)
	}
	if err := s.ResolveAdopt(docID, freshObs, resolveSeq); err != nil {
		t.Fatalf("ResolveAdopt: %v", err)
	}

	// Immediately after resolving: Clean.
	clean, err := s.Sync(docID)
	if err != nil {
		t.Fatalf("Sync after resolve: %v", err)
	}
	if clean.Kind != SyncClean {
		t.Fatalf("Sync after resolve: Kind = %v, want SyncClean", clean.Kind)
	}

	// Undo past the resolve (back to before resolveSeq).
	step, ok, err := s.UndoPeek(docID)
	if err != nil || !ok {
		t.Fatalf("UndoPeek: ok=%v err=%v", ok, err)
	}
	if err := s.MoveUndoPos(docID, step.NewPos); err != nil {
		t.Fatalf("MoveUndoPos (undo): %v", err)
	}

	diverged, err := s.Sync(docID)
	if err != nil {
		t.Fatalf("Sync after undo-past-resolve: %v", err)
	}
	if diverged.Kind != SyncDiverged {
		t.Fatalf("Sync after undo past resolve (seq %d, now at %d): Kind = %v, want SyncDiverged; Ours=%+v Theirs=%+v Ancestor=%+v",
			resolveSeq, step.NewPos, diverged.Kind, diverged.Ours, diverged.Theirs, diverged.Ancestor)
	}

	// Redo back up to (and above) the resolve: Clean again.
	redoStep, ok, err := s.RedoPeek(docID)
	if err != nil || !ok {
		t.Fatalf("RedoPeek: ok=%v err=%v", ok, err)
	}
	if err := s.MoveUndoPos(docID, redoStep.NewPos); err != nil {
		t.Fatalf("MoveUndoPos (redo): %v", err)
	}

	cleanAgain, err := s.Sync(docID)
	if err != nil {
		t.Fatalf("Sync after redo: %v", err)
	}
	if cleanAgain.Kind != SyncClean {
		t.Fatalf("Sync after redo past resolve: Kind = %v, want SyncClean", cleanAgain.Kind)
	}
}
