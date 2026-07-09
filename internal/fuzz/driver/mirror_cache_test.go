//go:build fuzzing

package driver

import (
	"testing"
	"time"

	tea "charm.land/bubbletea/v2"

	"rune/internal/fuzz/invariant"
	"rune/internal/fuzz/session"
	"rune/pkg/docstate"
	"rune/pkg/editor/buffer"
	"rune/pkg/terminal"
	"rune/pkg/ui/keymap"
	pgworkspace "rune/pkg/ui/pages/workspace"
	"rune/pkg/ui/styles"
	"rune/pkg/vfs"
)

// TestMirrorFor_CoalesceThenTruncate exercises the two correctness hazards
// the incremental mirrorFor cache was specifically designed against: a tail
// row mutated in place by AppendEdit's keystroke-coalescing UPDATE (same
// seq, changed content), and a later undo-then-edit that truncates rows the
// cache may have already folded into its frozen content. After every step,
// mirrorFor's output is cross-checked against store.Content(docID) — an
// independent reconstruction (snapshot-anchored RecoverDocument) that goes
// through completely different code (no baseline/frozen-tail bookkeeping),
// exactly mirroring the real SHADOW invariant's own Content vs MirrorContent
// comparison (internal/fuzz/ui/workspace/workspace.go).
func TestMirrorFor_CoalesceThenTruncate(t *testing.T) {
	now := time.Now()
	clock := func() time.Time { return now }
	store, err := docstate.OpenInMemory(clock)
	if err != nil {
		t.Fatalf("OpenInMemory: %v", err)
	}
	defer store.Close()

	ref, err := store.CreateScratch("")
	if err != nil {
		t.Fatalf("CreateScratch: %v", err)
	}
	docID := ref.ID

	rs := &runState{store: store, baselines: map[int64]string{}}

	check := func(step string) {
		t.Helper()
		want, err := store.Content(docID)
		if err != nil {
			t.Fatalf("%s: Content: %v", step, err)
		}
		got := rs.mirrorFor(docID, want, false)
		if got != want {
			t.Fatalf("%s: mirrorFor = %q, want %q (independent RecoverDocument reconstruction)", step, got, want)
		}
	}

	insert := func(ch string) {
		t.Helper()
		if _, err := store.AppendEdit(docID, []buffer.AppliedEdit{{Start: 0, End: 0, Insert: ch}}, nil, nil); err != nil {
			t.Fatalf("AppendEdit %q: %v", ch, err)
		}
	}

	insert("h")
	check("after h")

	// Coalescing burst: each insert lands within the 300ms window, mutating
	// the SAME journal row in place (round-1 hazard: the cache must never
	// trust a cached copy of this row).
	now = now.Add(50 * time.Millisecond)
	insert("e")
	check("after e (coalesced tail mutation)")

	now = now.Add(50 * time.Millisecond)
	insert("l")
	check("after l (coalesced again)")

	// Break the coalescing window: a new row appears, and the previous tail
	// (now provably non-tail) becomes safe to fold into frozen content.
	now = now.Add(400 * time.Millisecond)
	insert("!")
	check("after ! (new row, old tail frozen)")

	// A second coalescing burst, now with a NONZERO cached frozenSeq (the
	// "h"/"e"/"l" row froze when "!" landed) — this specifically exercises
	// the fast EXTEND path's tail refresh, not just the fallback path the
	// first burst above went through (frozenSeq stayed 0 throughout it,
	// since a single-row doc never takes the fast path at all).
	now = now.Add(50 * time.Millisecond)
	insert("?")
	check("after ? (coalesced into the new tail via the fast extend path)")

	// Undo twice — the cache must evict once the store's resolved position
	// drops below what it has frozen.
	for range 2 {
		step, ok, uerr := store.UndoPeek(docID)
		if uerr != nil {
			t.Fatalf("UndoPeek: %v", uerr)
		}
		if !ok {
			break
		}
		if uerr := store.MoveUndoPos(docID, step.NewPos); uerr != nil {
			t.Fatalf("MoveUndoPos: %v", uerr)
		}
		check("after undo")
	}

	// Edit after undo: AppendEdit truncates the abandoned future (round-2
	// adjacent hazard — a cache entry extending across a truncation boundary
	// would silently resurrect deleted bytes).
	now = now.Add(400 * time.Millisecond)
	insert("x")
	check("after truncating edit")
}

// TestMirrorFor_NeverTrustsZeroFrozenSeq directly exercises the invariant
// behind the frozenSeq>0 gate: a cache entry with frozenSeq==0 must never be
// used to short-circuit a call, because 0 is simultaneously "nothing
// confirmed frozen yet" and the value CurrentSeq resolves to for a doc with
// zero history — and a loaded file with exactly one edit since load (so its
// single row is never folded, leaving frozenSeq==0 with a genuinely
// non-empty frozenContent, unlike an untitled doc's always-empty baseline)
// is exactly the shape that would collide if that doc's id were ever freed
// and handed to a fresh document. This is deliberately a direct cache-state
// test rather than a full end-to-end reproduction: the real actionClose
// reuse trace (TestMirrorFor_DocIDReuseOnDiscardClose) only ever discards
// UNTITLED docs, whose baseline is always "", so a stale frozenSeq==0 entry
// there is harmless by coincidence (stale "" happens to equal the correct
// answer) — it doesn't stress this gate specifically. This test isolates
// the invariant itself so a future refactor that weakens the gate is caught
// even if no currently-known reachable trace depends on it.
func TestMirrorFor_NeverTrustsZeroFrozenSeq(t *testing.T) {
	store, err := docstate.OpenInMemory(time.Now)
	if err != nil {
		t.Fatalf("OpenInMemory: %v", err)
	}
	defer store.Close()

	ref, err := store.CreateScratch("")
	if err != nil {
		t.Fatalf("CreateScratch: %v", err)
	}
	docID := ref.ID

	rs := &runState{store: store, baselines: map[int64]string{}}

	// Establish a loaded-file-style baseline (non-empty, unlike an untitled
	// doc's ""), then journal exactly ONE edit — its single row is never
	// folded (rows[:len-1] is empty), so the resulting cache entry has
	// frozenSeq==0 with non-empty frozenContent.
	if got := rs.mirrorFor(docID, "loaded content", true); got != "loaded content" {
		t.Fatalf("initial load-family call: got %q, want %q", got, "loaded content")
	}
	if _, err := store.AppendEdit(docID, []buffer.AppliedEdit{{Start: 14, End: 14, Insert: " v2"}}, nil, nil); err != nil {
		t.Fatalf("AppendEdit: %v", err)
	}
	if got := rs.mirrorFor(docID, "loaded content v2", false); got != "loaded content v2" {
		t.Fatalf("after one edit: got %q, want %q", got, "loaded content v2")
	}
	cached, ok := rs.mirrorCache[docID]
	if !ok || cached.frozenSeq != 0 || cached.frozenContent != "loaded content" {
		t.Fatalf("setup: expected cache entry {frozenSeq:0, frozenContent:%q}, got %+v (ok=%v)", "loaded content", cached, ok)
	}

	// A second, brand-new document that genuinely has zero history also
	// resolves CurrentSeq()==0 — simulating what a reused docID's first
	// observation looks like, without needing to reproduce the exact
	// deletion/reuse machinery. Point the cache's docID key at this second
	// document's own id is not possible (map is keyed by the real docID), so
	// instead verify directly: a fresh document, correctly cached, is never
	// contaminated by another docID's stale frozenSeq==0 entry sharing the
	// SAME cache. Force the exact collision by reusing docID's cache slot
	// for the second document's traffic — the one thing DeleteDoc+CreateNew
	// would do in production (hand the same numeric id to a new document).
	ref2, err := store.CreateScratch("") // a genuinely different id in the store...
	if err != nil {
		t.Fatalf("CreateScratch (second doc): %v", err)
	}
	docID2 := ref2.ID
	rs.mirrorCache[docID2] = cached // ...but manufacture the exact collision mirrorFor must reject.

	got := rs.mirrorFor(docID2, "", false)
	if got != "" {
		t.Fatalf("mirrorFor returned %q for a doc with zero history, want \"\" — a stale frozenSeq==0 cache entry was trusted", got)
	}
}

// TestMirrorFor_DocIDReuseOnDiscardClose exercises the concrete non-load-
// family docID-reuse trace found during review: closing the sole dirty
// untitled tab with [D]iscard deletes its doc (store.DeleteDoc) and, since
// there is no neighbor tab, synchronously creates a fresh one
// (CreateUntitled -> CreateScratch) in the SAME Update — and because
// documents.id has no AUTOINCREMENT, SQLite can hand the freed id straight
// to the new document. isLoadFamilyMsg returns false for this message (it
// only recognizes FileLoadedMsg-family messages and the load-into-blank
// discard variant, not this one), so the cache's safety here depends
// entirely on the unconditional targetSeq<frozenSeq eviction, not on
// isLoadFamily. Driven through the real drainMsg entry point so the actual
// production SHADOW check (session.Check comparing Content vs MirrorContent)
// is what verifies correctness here, not a hand-rolled assertion.
func TestMirrorFor_DocIDReuseOnDiscardClose(t *testing.T) {
	keys := keymap.Default()
	reg, res, err := BuildFuzzApp(keys)
	if err != nil {
		t.Fatalf("BuildFuzzApp: %v", err)
	}

	store, err := docstate.OpenInMemory(time.Now)
	if err != nil {
		t.Fatalf("OpenInMemory: %v", err)
	}
	defer store.Close()

	mem := vfs.NewMem()
	store.UseFS(mem)

	st := styles.Default()
	caps := terminal.TermCaps{}
	model := pgworkspace.New(keys, st, reg, res, caps, "/fuzz", nil).WithFS(mem)

	rs := &runState{store: store, monitors: session.NewMonitors(), baselines: map[int64]string{}, mem: mem}

	model, v := bootstrap(rs, model, store, 80, 24)
	if v != nil {
		t.Fatalf("bootstrap: invariant %s: %s", v.InvariantID, v.Message)
	}

	step := func(msg tea.Msg) {
		t.Helper()
		var v *invariant.Violation
		model, v = drainMsg(rs, model, msg)
		if v != nil {
			t.Fatalf("drainMsg(%#v): invariant %s: %s", msg, v.InvariantID, v.Message)
		}
	}

	step(tea.KeyPressMsg{Code: 'e', Mod: tea.ModCtrl}) // focus editor
	step(tea.PasteMsg{Content: "dirty"})               // multi-char paste: its own row, never coalesces
	step(tea.PasteMsg{Content: " again"})              // a second row, so the first is provably frozen below
	firstDocID := model.FuzzInspect().DocID
	if firstDocID == 0 {
		t.Fatal("expected a real docID after journaling an edit")
	}

	// Confirm the cache actually holds NON-EMPTY frozen content for the
	// about-to-be-discarded doc — otherwise a stale post-reuse hit would
	// coincidentally read back "" and this test wouldn't discriminate a
	// broken frozenSeq>0 guard from a correct one (verified: this exact test
	// passed even with that guard removed until this second paste was added).
	if cached, ok := rs.mirrorCache[firstDocID]; !ok || cached.frozenSeq == 0 || cached.frozenContent == "" {
		t.Fatalf("setup: expected a non-empty frozen cache entry for docID %d before discard, got %+v (ok=%v)", firstDocID, cached, ok)
	}

	step(tea.KeyPressMsg{Code: 'w', Mod: tea.ModCtrl}) // close tab -> dirty guard (no neighbor tab)

	snapBeforeDiscard := model.FuzzInspect()
	if !snapBeforeDiscard.GuardVisible {
		t.Fatal("expected the dirty-close guard to be visible before discard")
	}

	// The real "[D]iscard" response is a plain 'd' keypress, not a directly
	// injected footer.DataLossGuardResponseMsg: footer.Update intercepts it
	// while the guard is visible, clears its OWN guard state synchronously,
	// and returns a Cmd that drainMsg's own drainCmd then drains to deliver
	// the DataLossGuardResponseMsg workspace acts on — a two-message
	// sequence, matching how the real fuzz clusters (dirtyCloseGuard,
	// internal/fuzz/workflow) drive this exact response. Injecting the
	// response message directly would skip the footer's own state-clearing
	// step and spuriously trip GUARD-STATE-COH.
	step(tea.KeyPressMsg{Code: 'd', Text: "d"})

	snapAfter := model.FuzzInspect()
	if snapAfter.DocID != firstDocID {
		t.Skipf("docID %d was not reused for the fresh untitled tab (got %d) — SQLite did not hand back the freed id in this run; the reuse scenario was not reached", firstDocID, snapAfter.DocID)
	}
	if snapAfter.Content != "" {
		t.Fatalf("expected a fresh blank untitled tab after discard-close, got content %q", snapAfter.Content)
	}
	// The regression check already ran inside the discard step() call above:
	// drainMsg -> mirrorFor computed MirrorContent for the reused docID, and
	// session.Check compared it against the live buffer's Content. A stale
	// cache entry surviving the reuse would have returned the DELETED
	// document's old content here and failed that call loudly (SHADOW).
}
