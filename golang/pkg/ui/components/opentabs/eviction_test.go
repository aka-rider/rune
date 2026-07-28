package opentabs

import (
	"fmt"
	"testing"

	"rune/pkg/ui/keymap"
	"rune/pkg/ui/styles"
)

// newModel is a test convenience constructor.
func newModel() Model { return New(keymap.Default(), styles.Default()) }

// th is a shorthand for constructing a TabHandle in tests.
func th(docID int64, path string) TabHandle { return TabHandle{DocID: docID, Path: path} }

// ─────────────────────────────────────────────────────────────────────────────
// SetActive stamping
// ─────────────────────────────────────────────────────────────────────────────

func TestSetActive_StampsOutgoingTab(t *testing.T) {
	m := newModel()
	m = m.OpenFile(1, "a.md")
	m = m.OpenFile(2, "b.md")
	m = m.SetActive(th(1, "a.md")) // A is now active (outgoing: none yet, activeHandle was zero)

	// Switch A→B: A should be stamped.
	m = m.SetActive(th(2, "b.md"))
	if m.tabs[0].lastActiveSeq == 0 {
		t.Error("SetActive should have stamped the outgoing tab A with a non-zero seq")
	}
	// B (now active) must NOT be stamped yet.
	if m.tabs[1].lastActiveSeq != 0 {
		t.Errorf("newly active tab B must not be stamped; got lastActiveSeq=%d", m.tabs[1].lastActiveSeq)
	}
}

func TestSetActive_Idempotent(t *testing.T) {
	m := newModel()
	m = m.OpenFile(1, "a.md")
	m = m.SetActive(th(1, "a.md"))
	seqBefore := m.activitySeq

	m = m.SetActive(th(1, "a.md")) // same handle — must be a no-op
	if m.activitySeq != seqBefore {
		t.Errorf("idempotent SetActive must not bump activitySeq: before=%d after=%d", seqBefore, m.activitySeq)
	}
}

func TestSetActive_LRUOrdering(t *testing.T) {
	// Open A, B, C. Visit them in order A→B→C→A.
	// Each switch stamps the OUTGOING tab with the current counter.
	// A→B stamps A (seq=1), B→C stamps B (seq=2), C→A stamps C (seq=3).
	// So the stamp ordering reflects the order of abandonment: seqA < seqB < seqC.
	m := newModel()
	m = m.OpenFile(1, "a.md")
	m = m.OpenFile(2, "b.md")
	m = m.OpenFile(3, "c.md")

	m = m.SetActive(th(1, "a.md")) // A active (no outgoing stamp — initial activeHandle was zero)
	m = m.SetActive(th(2, "b.md")) // A→B: A stamped with seq=1
	m = m.SetActive(th(3, "c.md")) // B→C: B stamped with seq=2
	m = m.SetActive(th(1, "a.md")) // C→A: C stamped with seq=3

	seqA := m.tabs[0].lastActiveSeq // stamped first (seq=1): oldest abandonment
	seqB := m.tabs[1].lastActiveSeq // stamped second (seq=2)
	seqC := m.tabs[2].lastActiveSeq // stamped third (seq=3): most recent abandonment

	if !(seqA < seqB && seqB < seqC) {
		t.Errorf("expected seqA < seqB < seqC; got seqA=%d seqB=%d seqC=%d", seqA, seqB, seqC)
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// HasTab
// ─────────────────────────────────────────────────────────────────────────────

func TestHasTab_ByDocID(t *testing.T) {
	m := newModel().OpenFile(7, "seven.md")
	if !m.HasTab(7, "seven.md") {
		t.Error("HasTab must return true for an open docID")
	}
	if m.HasTab(99, "seven.md") {
		t.Error("HasTab must return false for an unknown docID even if path matches")
	}
}

func TestHasTab_ByPath_ForVirtualDoc(t *testing.T) {
	m := newModel().OpenFile(0, "/help")
	if !m.HasTab(0, "/help") {
		t.Error("HasTab must return true for a virtual doc matched by path")
	}
	if m.HasTab(0, "/other") {
		t.Error("HasTab must return false for an unknown virtual path")
	}
}

func TestHasTab_False(t *testing.T) {
	m := newModel()
	if m.HasTab(1, "a.md") {
		t.Error("HasTab on empty model must return false")
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// EvictionCandidate
// ─────────────────────────────────────────────────────────────────────────────

func TestEvictionCandidate_PrefersCleanOverDirty(t *testing.T) {
	m := newModel()
	m = m.OpenFile(1, "dirty.md")
	m = m.OpenFile(2, "clean.md")
	m = m.OpenFile(3, "active.md")
	m = m.SetActive(th(1, "dirty.md"))
	m = m.SetActive(th(2, "clean.md"))
	m = m.SetActive(th(3, "active.md")) // 3 is now active
	m = m.SetDirty(TabHandle{DocID: 1}, true)

	victim, dirty, ok := m.EvictionCandidate()
	if !ok {
		t.Fatal("EvictionCandidate must return ok=true when eligible tabs exist")
	}
	if dirty {
		t.Errorf("EvictionCandidate must prefer clean tab; got dirty=%v victim=%v", dirty, victim)
	}
	if victim.DocID != 2 {
		t.Errorf("expected clean tab 2 as victim, got %+v", victim)
	}
}

func TestEvictionCandidate_LRUAmongClean(t *testing.T) {
	// Open A(1), B(2), C(3). Visit A→B→C→A.
	// Active = A. B was visited least recently → B should be evicted.
	m := newModel()
	m = m.OpenFile(1, "a.md")
	m = m.OpenFile(2, "b.md")
	m = m.OpenFile(3, "c.md")
	m = m.OpenFile(4, "d.md") // active tab

	m = m.SetActive(th(1, "a.md"))
	m = m.SetActive(th(2, "b.md"))
	m = m.SetActive(th(3, "c.md"))
	m = m.SetActive(th(4, "d.md")) // 4 is active; 1=oldest stamp, 2=middle, 3=newest

	// B(2) was stamped first (oldest), so it should be evicted.
	// Wait: actually 1 was stamped when we switched to 2.
	// 2 was stamped when we switched to 3.
	// 3 was stamped when we switched to 4.
	// So 1 has the smallest seq (most LRU). Expect 1 to be evicted.
	victim, dirty, ok := m.EvictionCandidate()
	if !ok {
		t.Fatal("expected ok=true")
	}
	if dirty {
		t.Errorf("expected clean victim, got dirty")
	}
	if victim.DocID != 1 {
		t.Errorf("expected LRU tab 1 as victim, got docID=%d", victim.DocID)
	}
}

func TestEvictionCandidate_FallsThroughToDirty(t *testing.T) {
	m := newModel()
	m = m.OpenFile(1, "dirty.md")
	m = m.OpenFile(2, "active.md")
	m = m.SetActive(th(1, "dirty.md"))
	m = m.SetActive(th(2, "active.md")) // 2 is active
	m = m.SetDirty(TabHandle{DocID: 1}, true)

	victim, dirty, ok := m.EvictionCandidate()
	if !ok {
		t.Fatal("expected ok=true when only dirty eligible tab exists")
	}
	if !dirty {
		t.Error("expected dirty=true when only dirty candidates exist")
	}
	if victim.DocID != 1 {
		t.Errorf("expected tab 1 as dirty victim, got %+v", victim)
	}
}

func TestEvictionCandidate_SkipsActiveTab(t *testing.T) {
	m := newModel()
	m = m.OpenFile(1, "only.md")
	m = m.SetActive(th(1, "only.md"))

	_, _, ok := m.EvictionCandidate()
	if ok {
		t.Error("active tab must never be an eviction candidate")
	}
}

func TestEvictionCandidate_SkipsPinned(t *testing.T) {
	m := newModel()
	m = m.OpenFile(1, "pinned.md")
	m = m.OpenFile(2, "active.md")
	m = m.SetActive(th(1, "pinned.md"))
	m = m.SetActive(th(2, "active.md"))
	m = m.PinIndex(0) // pin tab at index 0 (doc 1)

	_, _, ok := m.EvictionCandidate()
	if ok {
		t.Error("pinned tab must not be an eviction candidate")
	}
}

func TestEvictionCandidate_SkipsHelp(t *testing.T) {
	// Help tabs have DocID == 0.
	m := newModel()
	m = m.OpenFile(0, "/help")
	m = m.OpenFile(1, "active.md")
	m = m.SetActive(th(1, "active.md")) // help is not active, but DocID==0 → exempt

	_, _, ok := m.EvictionCandidate()
	if ok {
		t.Error("help tab (DocID==0) must not be an eviction candidate")
	}
}

func TestEvictionCandidate_SkipsUntitledDraft(t *testing.T) {
	// Untitled drafts have Path == "" and DocID != 0.
	m := newModel()
	m = m.OpenFile(5, "") // untitled draft
	m = m.OpenFile(6, "active.md")
	m = m.SetActive(th(5, ""))
	m = m.SetActive(th(6, "active.md"))

	_, _, ok := m.EvictionCandidate()
	if ok {
		t.Error("untitled draft (Path==\"\") must not be an eviction candidate")
	}
}

func TestEvictionCandidate_NoEligible_AllExempt(t *testing.T) {
	m := newModel()
	m = m.OpenFile(1, "pinned.md")
	m = m.OpenFile(0, "/help")
	m = m.OpenFile(2, "active.md")
	m = m.SetActive(th(1, "pinned.md"))
	m = m.SetActive(th(2, "active.md"))
	m = m.PinIndex(0) // pin doc 1

	_, _, ok := m.EvictionCandidate()
	if ok {
		t.Error("EvictionCandidate must return ok=false when all candidates are exempt")
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// Fuzz: opentabs model — open/close/setActive/markDirty/pin operation sequences
// ─────────────────────────────────────────────────────────────────────────────

func FuzzEvictionModel(f *testing.F) {
	// Open 4, cycle active A→B→C→A, mark B dirty → eviction prefers clean C over dirty B.
	f.Add([]byte{0, 0, 0, 0, 0, 0, 0, 0, 2, 0, 2, 1, 2, 2, 2, 0, 3, 1, 0, 0})
	// Open 4, pin first two, mark all dirty → dirty tier (only unpinned eligible).
	f.Add([]byte{0, 0, 0, 0, 0, 0, 0, 0, 4, 0, 4, 1, 3, 0, 3, 1, 3, 2, 3, 3, 2, 3, 0, 0})
	// Open/close cycling.
	f.Add([]byte{0, 0, 0, 0, 2, 0, 1, 0, 0, 0, 2, 0, 1, 0, 0, 0})
	// All dirty → dirty tier; LRU ordering among dirty candidates.
	f.Add([]byte{0, 0, 0, 0, 0, 0, 3, 0, 3, 1, 3, 2, 2, 2, 0, 0})

	f.Fuzz(func(t *testing.T, ops []byte) {
		m := newModel()
		var docCounter int64 = 1

		checkInvariants := func() {
			t.Helper()

			// INV-EVICT-ELIGIBLE: EvictionCandidate only returns tabs that are
			// eligible: not active, not pinned, DocID≠0, Path≠"".
			victim, _, ok := m.EvictionCandidate()
			if ok {
				if victim.Equal(m.activeHandle) {
					t.Error("INV-EVICT-ELIGIBLE: victim is the active tab")
				}
				if victim.DocID == 0 {
					t.Error("INV-EVICT-ELIGIBLE: victim.DocID == 0 (help exempt)")
				}
				if victim.Path == "" {
					t.Error("INV-EVICT-ELIGIBLE: victim.Path empty (untitled exempt)")
				}
				for _, tab := range m.tabs {
					if tab.DocID == victim.DocID && tab.Pinned {
						t.Error("INV-EVICT-ELIGIBLE: victim is pinned")
					}
				}
			}

			// INV-SEQ: every tab's lastActiveSeq must not exceed the global counter.
			for _, tab := range m.tabs {
				if tab.lastActiveSeq > m.activitySeq {
					t.Errorf("INV-SEQ: tab %q lastActiveSeq=%d > activitySeq=%d",
						tab.Path, tab.lastActiveSeq, m.activitySeq)
				}
			}

			// INV-ACTIVE-UNIQUE: at most one tab matches the active handle.
			count := 0
			for _, tab := range m.tabs {
				if (TabHandle{DocID: tab.DocID, Path: tab.Path}).Equal(m.activeHandle) {
					count++
				}
			}
			if count > 1 {
				t.Errorf("INV-ACTIVE-UNIQUE: %d tabs match activeHandle %v", count, m.activeHandle)
			}
		}

		for i := 0; i+1 < len(ops); i += 2 {
			op := int(ops[i]) % 5
			arg := ops[i+1]
			tabs := m.tabs // snapshot before op — stable indices for this iteration

			if len(tabs) == 0 && op != 0 {
				continue
			}

			switch op {
			case 0: // open new tab
				m = m.OpenFile(docCounter, fmt.Sprintf("f%d.md", docCounter))
				docCounter++
			case 1: // close tab at index
				m = m.Close(TabHandle{DocID: tabs[int(arg)%len(tabs)].DocID})
			case 2: // set active to tab at index
				tab := tabs[int(arg)%len(tabs)]
				m = m.SetActive(TabHandle{DocID: tab.DocID, Path: tab.Path})
			case 3: // mark dirty by index
				m = m.SetDirty(TabHandle{DocID: tabs[int(arg)%len(tabs)].DocID}, true)
			case 4: // toggle pin at index
				m = m.PinIndex(int(arg) % len(tabs))
			}

			checkInvariants()
		}
	})
}
