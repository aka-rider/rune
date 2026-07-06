//go:build fuzzing

// WP4 persistence clusters (11-15). See workflow.go for the shared helpers
// (activePathSlot, paletteText, mergeGuardResponseIndex, event vars) these
// clusters use.
package workflow

import "rune/internal/fuzz/event"

// MergeResolve [r,u]: edit ours → external write → ⌘S → FileSaveErrorMsg{Conflict}
// → GuardMerge; r%4 picks the response:
//
//	0 [M]erge: enter the resolver, exercise o/t/n resolver keys, then EITHER
//	  undo×u (if u>0 — resyncMergeIfMain may unwind active→inactive and
//	  probeUnwindCmd/handleUnwindProbe re-raises GuardMerge, drained with Esc)
//	  OR ⌘S to persist the resolution.
//	1 [D]iscard (handleDataLossDiscardConflict → applyDiscardConflict).
//	2 [S]ave anyway (handleDataLossSaveAnyway — CAS force-write).
//	3 Esc (Cancel).
func mergeResolve(data []byte) ([]event.Event, int) {
	r := uint8(0)
	u := uint8(0)
	consumed := 0
	if len(data) >= 2 {
		r = data[0] % 4
		u = data[1] % 3
		consumed = 2
	}
	var evs []event.Event
	evs = append(evs, evEdit)
	evs = append(evs, textEvent("ours-edit"))
	// The external write changes ONLY line 1 of a.md, keeping the rest
	// identical to the ancestor — same shape as ours's edit (also line-1
	// only, via prepend at cursor 0) — so both sides genuinely overlap on
	// the SAME line and MergeHunks (pkg/merge) produces a real HunkConflict
	// block (mergemode.Enter's st.active = len(blocks)>0), not a clean
	// auto-merge. A full-file replacement (the plan's literal
	// "external-content\n") was tried first and produced ZERO conflict
	// blocks — MergeHunks' diff3 evidently resolves "theirs replaces
	// everything, ours only touched the prefix" without a marker region,
	// which starves applyMergeConflict's [M]/undo/handleUnwindProbe path of
	// ANY active resolver session to interact with.
	evs = append(evs, event.Event{Kind: event.KindExternalWrite, PathIndex: 0,
		Text: "# File A (external)\n\n[B](b.md)\n[notes](notes/c.md)\n[x](missing.md)\n[web](https://example.com)\n[mail](mailto:a@b.com)\n"})
	evs = append(evs, evSave) // → refused (Conflict) → GuardMerge

	switch r {
	case 0: // [M]erge
		evs = append(evs, event.Event{Kind: event.KindKey, KeyIndex: mergeGuardResponseIndex(0)})
		if u > 0 {
			// Undo IMMEDIATELY, before resolving the (single) conflict block:
			// mergemode.Enter journals the marker-buffer install as ONE edit,
			// so the FIRST undo unwinds exactly that edit, taking the resolver
			// from active→inactive — resyncMergeIfMain's own trigger
			// (workspace_merge_fresh.go) — and launches probeUnwindCmd,
			// landing in handleUnwindProbe. Resolving first (o/n/t) would
			// deactivate the (single-block) resolver on its own before undo
			// ever ran, so wasActive would already be false and
			// resyncMergeIfMain's gate would never fire.
			evs = append(evs, repeat(evUndo, int(u))...)
			evs = append(evs, evEsc) // drain a re-raised GuardMerge if handleUnwindProbe fired one
		} else {
			evs = append(evs, evMergeOurs, evMergeNext, evMergeTheirs)
			evs = append(evs, evSave) // persist the resolution
		}
	case 1: // [D]iscard
		evs = append(evs, event.Event{Kind: event.KindKey, KeyIndex: mergeGuardResponseIndex(1)})
	case 2: // [S]ave anyway
		evs = append(evs, event.Event{Kind: event.KindKey, KeyIndex: mergeGuardResponseIndex(2)})
	default: // Esc
		evs = append(evs, event.Event{Kind: event.KindKey, KeyIndex: mergeGuardResponseIndex(3)})
	}
	return evs, consumed
}

// ExternalRename [src,_,_]: an external process renames a tracked file to
// d.md (dst is fixed — see below) → KindWatch(dir-changed) surfaces it to
// the filetree → Esc-drain any stray banner → tree-navigate (evHome then a
// FIXED Down-count) to "d.md" and open it (its inode now carries src's
// identity, so store.Load's inode match fires the RenamedFrom branch of
// handleFileLoadedMsg) → edit → save. Organically covers path-reuse (§1.4.6).
//
// dst is fixed to d.md (PathIndex 3), not fuzzed, so the Down-count below is
// a known constant against the KNOWN top-level listing order (seedMem's
// fixed set: .. notes a.md b.md d.md — dirs first, then files alphabetical).
// evHome (filetree's GotoTop) resets the cursor to 0 FIRST — filetree cursor
// position persists across focus changes and Watch-triggered DirReloadedMsg
// (only a user-navigation DirLoadedMsg resets it), so an absolute reset
// before counting Downs is required for the count to be correct regardless
// of what any earlier step in the same run left the cursor at. The
// filetree's letter-jump-to-name feature was tried first and rejected: its
// search buffer only resets on a REAL 750ms wall-clock gap, which the
// synchronous fuzz driver never produces between consecutive events, so
// letters typed by an earlier step in the same run silently prefix the next
// step's query. src is fuzzed (a.md/b.md, never d.md — renaming a path onto
// itself is a no-op the driver already treats as safe but adds nothing).
func externalRename(data []byte) ([]event.Event, int) {
	src := uint8(0)
	consumed := 0
	if len(data) >= 1 {
		src = data[0] % 2 // 0=a.md, 1=b.md — both top-level, never d.md
		consumed = 1
	}
	const dst = 3 // d.md
	var evs []event.Event
	evs = append(evs, event.Event{Kind: event.KindExternalRename, PathIndex: src, DestIndex: dst})
	evs = append(evs, event.Event{Kind: event.KindWatch, PathIndex: 0, WatchSub: 0}) // dir-changed
	evs = append(evs, evEsc)
	evs = append(evs, evTree, evHome)
	evs = append(evs, repeat(evDown, 4)...) // .. notes a.md b.md [d.md] — index 4
	evs = append(evs, evEnter)
	evs = append(evs, evEdit)
	evs = append(evs, textEvent("after-rename"))
	evs = append(evs, evSave)
	return evs, consumed
}

// ExternalDelete [p,r,v]:
//
//	v=0 (watch-detect): an external process removes the ACTIVE file (the
//	  activePathSlot convention) → KindWatch(dir-changed) → the idle
//	  probeDocCmd cycle observes it missing → raiseDeletedGuard → s/d/Esc
//	  (handleDeletedSave recreate / handleDeletedDiscard purge / keep editing).
//	v=1 (save-detect): dirty the active file, then remove it externally
//	  WITHOUT a watch event → ⌘S → FileSaveErrorMsg{Missing} → GuardDeleted
//	  → s/d/Esc.
func externalDelete(data []byte) ([]event.Event, int) {
	p := uint8(activePathSlot)
	r := uint8(0)
	v := uint8(0)
	consumed := 0
	if len(data) >= 3 {
		r = data[1] % 3
		v = data[2] % 2
		consumed = 3
	}
	guardKey := event.Event{Kind: event.KindKey, KeyIndex: guardResponseIndex(r)}
	var evs []event.Event
	if v == 0 {
		evs = append(evs, event.Event{Kind: event.KindExternalRemove, PathIndex: p})
		evs = append(evs, event.Event{Kind: event.KindWatch, PathIndex: p, WatchSub: 0})
		evs = append(evs, guardKey)
	} else {
		evs = append(evs, evEdit)
		evs = append(evs, textEvent("about-to-vanish"))
		evs = append(evs, event.Event{Kind: event.KindExternalRemove, PathIndex: p})
		evs = append(evs, evSave) // → FileSaveErrorMsg{Missing} → GuardDeleted
		evs = append(evs, guardKey)
	}
	return evs, consumed
}

// EvictionPressure [n,v]:
//
//	v=0 (dirty-victim): dirty a.md, open+dirty b.md, fill n%4+8 (8-11)
//	  untitled tabs — pushing the total past tabLimit(10) — then tree-open
//	  notes/c.md. enforceTabLimit's EvictionCandidate excludes untitled tabs
//	  (DocID==0/Path=="") entirely, so the only eligible non-active candidate
//	  is the dirty, unpinned bound tab (a.md) → GuardDirty(evict) →
//	  s/d/Esc (evictSave/evictSaveAck / evictDiscard / refusal — the key is
//	  silently consumed, c.md stays unopened).
//	v=1 (no-eligible-victim refusal): pin a.md first (removing the only
//	  other bound tab from candidacy), then 10 untitled tabs, then tree-open
//	  notes/c.md — EvictionCandidate finds nothing (all candidates pinned or
//	  untitled) → "Tab limit reached" refusal, no guard.
func evictionPressure(data []byte) ([]event.Event, int) {
	n := uint8(0)
	v := uint8(0)
	consumed := 0
	if len(data) >= 2 {
		n = data[0]
		v = data[1] % 2
		consumed = 2
	}
	// Tree navigation uses evHome (filetree's GotoTop, resets cursor to 0)
	// then a FIXED Down-count against the KNOWN listing order — see
	// externalRename's doc comment for why letter-jump search doesn't work
	// reliably under this synchronous driver.
	openNotesC := []event.Event{
		evTree, evHome, evDown, evEnter, // .. [notes] a.md b.md d.md → index 1 → descend
		evHome, evDown, evEnter, // within notes/: .. [c.md] e.md → index 1 → open
	}
	var evs []event.Event
	if v == 0 {
		evs = append(evs, evEdit)
		evs = append(evs, textEvent("dirty-a"))
		evs = append(evs, evTree, evHome)
		evs = append(evs, repeat(evDown, 3)...) // .. notes a.md [b.md] d.md → index 3
		evs = append(evs, evEnter)              // open b.md
		evs = append(evs, evEdit)
		evs = append(evs, textEvent("dirty-b"))
		fill := int(n%4) + 8
		evs = append(evs, newDistinctUntitleds(fill)...)
		evs = append(evs, openNotesC...)
		response := n % 3
		evs = append(evs, event.Event{Kind: event.KindKey, KeyIndex: guardResponseIndex(response)})
	} else {
		evs = append(evs, evPin) // pin a.md — the only other bound tab — out of candidacy
		evs = append(evs, newDistinctUntitleds(10)...)
		evs = append(evs, openNotesC...) // refused: no eligible eviction candidate
	}
	return evs, consumed
}

// newDistinctUntitleds builds events for n DISTINCT untitled tabs. A bare
// repeat(evNew, n) does NOT work: CreateNewFile's own guard
// (workspace_update_keys.go) only calls CreateUntitled when the current view
// isn't untitled OR has non-empty content — reusing the current blank
// untitled (just re-focusing its title field to rename) otherwise, so a run
// of ctrl+n presses with nothing between them creates exactly ONE tab. Each
// step here commits the title (⌘N's own default name via Enter — returning
// focus to the editor) then types a one-character marker so the NEXT ctrl+n
// sees non-empty content and genuinely creates a fresh tab.
func newDistinctUntitleds(n int) []event.Event {
	var evs []event.Event
	for i := 0; i < n; i++ {
		evs = append(evs, evNew, evEnter, textEvent("x"))
	}
	return evs
}

// QuitSaveAll [r]: dirty two tabs (a.md, b.md) → KindQuitRequest (the ^C^C
// chord's completion message injected directly — structurally unreachable
// inline, see event.KindQuitRequest) → GuardDirty(quit) → s/d/Esc:
// saveAllDirtyForQuit batch + saveLeft countdown + teardownAndQuit /
// discard-all quit / cancel.
func quitSaveAll(data []byte) ([]event.Event, int) {
	r := uint8(0)
	consumed := 0
	if len(data) >= 1 {
		r = data[0] % 3
		consumed = 1
	}
	guardKey := event.Event{Kind: event.KindKey, KeyIndex: guardResponseIndex(r)}
	var evs []event.Event
	evs = append(evs, evEdit)
	evs = append(evs, textEvent("dirty-a"))
	evs = append(evs, evTree, evHome)
	evs = append(evs, repeat(evDown, 3)...) // .. notes a.md [b.md] d.md → index 3
	evs = append(evs, evEnter)              // open b.md
	evs = append(evs, evEdit)
	evs = append(evs, textEvent("dirty-b"))
	evs = append(evs, event.Event{Kind: event.KindQuitRequest})
	evs = append(evs, guardKey)
	return evs, consumed
}
