package workspace

// Spec-validation tests for the IsDirty invariant:
//
//   dirty ⟺ effective_journal_position ≠ clean_journal_position
//
// Each test drives the full Update → finalize → syncDirty cycle and asserts
// m.opentabs.HasDirty() at defined steps. These tests prove the SPECIFICATION,
// not implementation internals — they must fail on the old revision-based
// implementation and pass only when the invariant is correct.
//
// Coalescing note: the journal coalesces adjacent single-char inserts within
// 300ms as long as the last inserted char is not whitespace. Tests use typeSeq
// (char + Enter) to create a coalesce-resistant event boundary — the newline
// makes the event's last char whitespace, so the next char is always a new
// journal event.

import (
	"testing"

	tea "charm.land/bubbletea/v2"
)

// ── helpers ──────────────────────────────────────────────────────────────────

// typeChar sends one printable character to the focused editor.
func typeChar(m Model, ch rune) Model {
	m, _ = m.Update(tea.KeyPressMsg{Code: ch})
	return m
}

// typeSeq types a character then a newline, creating a single coalesced event
// whose last char is whitespace. Because the coalescing rule does not coalesce
// across a whitespace-terminated event, the NEXT call to typeChar (or typeSeq)
// will always produce a separate journal event.
func typeSeq(m Model, ch rune) Model {
	m = typeChar(m, ch)
	m, _ = m.Update(tea.KeyPressMsg{Code: tea.KeyEnter})
	return m
}

// undoOnce sends the undo key (ctrl+z).
func undoOnce(m Model) Model {
	m, _ = m.Update(tea.KeyPressMsg{Code: 'z', Mod: tea.ModCtrl})
	return m
}

// redoOnce sends the redo key (ctrl+y).
func redoOnce(m Model) Model {
	m, _ = m.Update(tea.KeyPressMsg{Code: 'y', Mod: tea.ModCtrl})
	return m
}

// save simulates a complete ⌘S round-trip (startSave → FileSavedMsg).
func save(m Model) Model {
	m, _ = m.startSave()
	reqID := m.activeSave.RequestID
	m, _ = m.Update(FileSavedMsg{Path: m.filePath, RequestID: reqID})
	return m
}

// dirtyWorkspace returns a workspace with a store, focused editor, and a file
// already loaded. All spec tests start from here.
func dirtyWorkspace(t *testing.T) Model {
	t.Helper()
	m := newTestWorkspace(t)
	m = withStore(t, m)
	m = loadFile(m, "spec.md", "")
	m = focusEditor(m)
	return m
}

// ── P1: undo-to-saved-state clears dirty ─────────────────────────────────────

func TestDirtySpec_P1_UndoToSaveStateClearsDirty(t *testing.T) {
	m := dirtyWorkspace(t)

	// Step 1 – freshly loaded: clean.
	if m.opentabs.HasDirty() {
		t.Fatal("P1 step 1: should be clean after load")
	}

	// Step 2 – create event E1 (ends in '\n' so next char won't coalesce): dirty.
	m = typeSeq(m, 'a') // event seq=1 = "a\n"
	if !m.opentabs.HasDirty() {
		t.Fatal("P1 step 2: should be dirty after first edit")
	}

	// Step 3 – save: clean. cleanJournalPos = storeMaxSeq = 1.
	m = save(m)
	if m.opentabs.HasDirty() {
		t.Fatal("P1 step 3: should be clean after save")
	}

	// Step 4 – new event E2 (no coalesce: E1 ends in '\n'): dirty.
	m = typeChar(m, 'b') // event seq=2 = "b"
	if !m.opentabs.HasDirty() {
		t.Fatal("P1 step 4: should be dirty after post-save edit")
	}

	// Step 5 – undo E2: back at E1 (save state): clean.
	m = undoOnce(m)
	if m.opentabs.HasDirty() {
		t.Fatal("P1 step 5: should be clean after undoing back to save state")
	}
}

// ── P2: undo-past-saved-state stays dirty ────────────────────────────────────

func TestDirtySpec_P2_UndoPastSaveStateStaysDirty(t *testing.T) {
	m := dirtyWorkspace(t)

	m = typeSeq(m, 'a') // event seq=1
	m = save(m)          // clean at seq=1
	m = typeChar(m, 'b') // event seq=2, dirty
	m = undoOnce(m)      // back at seq=1: clean

	if m.opentabs.HasDirty() {
		t.Fatal("P2 prerequisite: should be clean at save state")
	}

	// Undo one more (before any event): dirty.
	m = undoOnce(m)
	if !m.opentabs.HasDirty() {
		t.Fatal("P2: should be dirty after undoing past save state")
	}
}

// ── P3: redo-back-to-saved-state clears dirty ────────────────────────────────

func TestDirtySpec_P3_RedoToSaveStateClearsDirty(t *testing.T) {
	m := dirtyWorkspace(t)

	m = typeSeq(m, 'a') // seq=1
	m = save(m)          // clean at seq=1
	m = typeChar(m, 'b') // seq=2, dirty
	m = undoOnce(m)      // clean at seq=1
	m = undoOnce(m)      // dirty (before seq=1)

	if !m.opentabs.HasDirty() {
		t.Fatal("P3 prerequisite: should be dirty after undoing past save")
	}

	// Redo back to save state (seq=1): clean.
	m = redoOnce(m)
	if m.opentabs.HasDirty() {
		t.Fatal("P3: should be clean after redoing back to save state")
	}
}

// ── P4: redo-past-saved-state re-dirtied ─────────────────────────────────────

func TestDirtySpec_P4_RedoPastSaveStateDirty(t *testing.T) {
	m := dirtyWorkspace(t)

	m = typeSeq(m, 'a') // seq=1
	m = save(m)          // clean at seq=1
	m = typeChar(m, 'b') // seq=2, dirty
	m = undoOnce(m)      // clean at seq=1
	m = undoOnce(m)      // dirty (before seq=1)
	m = redoOnce(m)      // clean at seq=1

	if m.opentabs.HasDirty() {
		t.Fatal("P4 prerequisite: should be clean at save state")
	}

	// Redo past save (to seq=2): dirty.
	m = redoOnce(m)
	if !m.opentabs.HasDirty() {
		t.Fatal("P4: should be dirty after redoing past save state")
	}

	// Undo back to save state: clean.
	m = undoOnce(m)
	if m.opentabs.HasDirty() {
		t.Fatal("P4: should be clean after undoing back to save state")
	}
}

// ── P5: new-edit after undo, then undo-back ───────────────────────────────────
//
// When new text is typed immediately after undoing to the clean state (which
// truncates the abandoned future), undoing that new text must return to clean.
// Robust under the events-between dirty predicate: AppendEdit deletes the
// truncated event, so no live event sits between the saved and current
// positions even though their seq numbers differ.

func TestDirtySpec_P5_NewEditAfterUndoThenUndoBack(t *testing.T) {
	m := dirtyWorkspace(t)

	// Save the empty baseline.
	m = save(m)
	if m.opentabs.HasDirty() {
		t.Fatal("P5 step 0: should be clean after saving empty file")
	}

	// Type A (seq=1): dirty.
	m = typeSeq(m, 'a') // seq=1 = "a\n"
	if !m.opentabs.HasDirty() {
		t.Fatal("P5 step 1: should be dirty after typing A")
	}

	// Undo A: clean (back to the saved empty baseline — no live event remains
	// between the saved and current positions).
	m = undoOnce(m)
	if m.opentabs.HasDirty() {
		t.Fatal("P5 step 2: should be clean after undoing A")
	}

	// Type B (new edit from clean state): AppendEdit truncates the abandoned
	// seq=1 event and inserts a fresh event. Dirty — a live event now sits
	// above the saved baseline.
	m = typeSeq(m, 'b') // seq=2 = "b\n"
	if !m.opentabs.HasDirty() {
		t.Fatal("P5 step 3: should be dirty after typing B")
	}

	// Undo B: clean — the truncated seq=1 no longer exists, so no live event
	// sits between the saved baseline and the post-undo position.
	m = undoOnce(m)
	if m.opentabs.HasDirty() {
		t.Fatal("P5 step 4: should be clean after undoing B")
	}
}

// ── P6: save at mid-undo position ────────────────────────────────────────────

func TestDirtySpec_P6_SaveAtMidUndoPosition(t *testing.T) {
	m := dirtyWorkspace(t)

	// Create two separate events.
	m = typeSeq(m, 'a') // seq=1 = "a\n"
	m = typeChar(m, 'b') // seq=2 = "b"

	// Undo×1: now positioned at seq=1.
	m = undoOnce(m)

	// Save at mid-undo: MarkSaved records the current position (seq=1) as saved.
	m = save(m)
	if m.opentabs.HasDirty() {
		t.Fatal("P6 step 1: should be clean after save at mid-undo position")
	}

	// Undo past save point: dirty — the seq=1 event now sits between the
	// current position and the saved position.
	m = undoOnce(m)
	if !m.opentabs.HasDirty() {
		t.Fatal("P6 step 2: should be dirty after undoing past save point")
	}

	// Redo back to the saved position: clean again.
	m = redoOnce(m)
	if m.opentabs.HasDirty() {
		t.Fatal("P6 step 3: should be clean after redoing to save position")
	}
}

// ── P7: fresh untitled doc is always clean ────────────────────────────────────

func TestDirtySpec_P7_FreshUntitledIsClean(t *testing.T) {
	m := newTestWorkspace(t)
	m = withStore(t, m)
	// No loadFile — the workspace starts with an untitled tab.
	if m.opentabs.HasDirty() {
		t.Fatal("P7: fresh untitled doc must not be dirty")
	}
}
