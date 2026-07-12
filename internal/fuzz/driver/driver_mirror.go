package driver

import (
	"fmt"

	tea "charm.land/bubbletea/v2"

	"rune/internal/fuzz/snapshot"
	"rune/pkg/docstate"
	"rune/pkg/editor/buffer"
	"rune/pkg/ui/components/footer"
	pgworkspace "rune/pkg/ui/pages/workspace"
)

// isLoadResult reports whether msg is an async file-load result — the only
// message class RunReorder defers, since out-of-order delivery of these is the
// race under test.
func isLoadResult(msg tea.Msg) bool {
	switch msg.(type) {
	case pgworkspace.FileLoadedMsg, pgworkspace.FileLoadErrorMsg:
		return true
	}
	return false
}

// isLoadFamilyMsg reports whether msg is a load-driven (or equally
// legitimate, explicit-user-confirmed) restore: the doc's baseline is
// legitimately re-established from disk/journal content or purged to blank,
// not from the buffer's own undo-derived state. %T-name matched (the same
// convention internal/fuzz/ui/* uses for workspace's other unexported
// message types) since lateBindLoadedMsg is unexported and driver.go cannot
// import it across the package boundary. prev is the pre-Update snapshot —
// needed to correlate a DataLossGuardResponseMsg to the specific guard it
// resolved.
func isLoadFamilyMsg(msg tea.Msg, prev snapshot.Snapshot) bool {
	if isLoadResult(msg) {
		return true // FileLoadedMsg / FileLoadErrorMsg
	}
	if _, ok := msg.(pgworkspace.StoreReadyMsg); ok {
		return true
	}
	switch fmt.Sprintf("%T", msg) {
	case "workspace.lateBindLoadedMsg":
		// The store-becomes-ready late bind: the startup untitled tab was
		// created store-less (docID==0, mirrorFor is a no-op for it); THIS
		// message is what first assigns its real docID and carries the
		// actual freshly-read disk content — the doc's true first baseline.
		return true
	}
	if resp, ok := msg.(footer.DataLossGuardResponseMsg); ok &&
		resp.Response == footer.DataLossDiscard && prev.PendingDeletedActive {
		// GuardDeleted's [D]iscard purges the doc's history
		// (handleDeletedDiscard → store.DeleteDoc) AND resets the buffer to
		// blank — the same "explicit, prompt-confirmed baseline reset"
		// class CLOSE-NO-LOSS already carves out (driver_verbatim.go), not
		// a buffer that merely got fully undone. Found via this exact
		// scenario false-positiving SHADOW once UNDO-BASELINE landed (WP7
		// session): mirrorFor otherwise kept comparing the fresh blank
		// buffer against the doc's stale pre-purge baseline.
		return true
	}
	return false
}

// mirrorCacheEntry caches mirrorFor's reconstruction for one docID, up to
// (and including) frozenSeq — never including the current tail row, which
// AppendEdit's keystroke-coalescing UPDATE can still mutate in place.
// frozenSeq == 0 means "nothing confirmed frozen yet" and is NEVER trusted
// by the fast path below: it is the same sentinel CurrentSeq returns for a
// brand-new document, and documents.id (plain INTEGER PRIMARY KEY, no
// AUTOINCREMENT) can be reused after DeleteDoc, so a stale zero-entry from a
// deleted doc must never be mistaken for a fresh doc's real state.
type mirrorCacheEntry struct {
	frozenSeq     int64
	frozenContent string
}

// setMirrorCache lazily allocates rs.mirrorCache and writes entry — a tiny
// helper so both write sites in mirrorFor don't repeat the nil-map check.
func (rs *runState) setMirrorCache(docID, frozenSeq int64, frozenContent string) {
	if rs.mirrorCache == nil {
		rs.mirrorCache = map[int64]mirrorCacheEntry{}
	}
	rs.mirrorCache[docID] = mirrorCacheEntry{frozenSeq: frozenSeq, frozenContent: frozenContent}
}

// mirrorFor reconstructs the document's content independently of the live editor
// buffer: the loaded baseline plus every journaled "main" edit replayed forward.
// The baseline is captured the first time a doc is seen with zero edits (its buffer
// IS the freshly-loaded content then); later edits replay on top — matching how the
// editor builds the buffer (loaded content + edits). This lets SHADOW validate that
// the journal faithfully tracks the buffer for LOADED files, not just untitled ones
// born empty.
//
// UNDO-BASELINE (WP7, landed last with its own soak per the plan): isLoadFamily
// distinguishes a legitimate fresh baseline (a new/re- load) from a buffer
// that's merely fully undone (active edits empty because the undo head is back
// at the load position, NOT because a new doc was loaded). Silently
// re-adopting whatever the LIVE BUFFER holds as the new baseline in the
// latter case is a soundness gap: a full-undo restoring WRONG bytes (an undo
// bug) would be blessed as "correct" and never caught again. Verified safe
// per the plan: title edits are never journaled; discard/merge resolutions
// journal a ReplaceAll (batches stays non-empty, this branch doesn't apply);
// an untitled doc starts baselined at "" already.
//
// PERFORMANCE: this function runs on EVERY settled message, unsampled (unlike
// checkV4Properties), so it caches its reconstruction incrementally instead of
// replaying the full edit history from scratch every call — a full replay's
// cost is quadratic in journal length per call (buffer.ReplayForward rebuilds
// the whole string per edit), so redoing it on every one of a session's calls
// is cubic overall. The cache only ever trusts rows strictly older than the
// current tail as frozen — never the tail itself, which AppendEdit's
// keystroke-coalescing UPDATE can still mutate in place (coalescing only ever
// targets the doc's current max-seq row, journal.go's AppendEdit) — and is
// gated on frozenSeq > 0 (see mirrorCacheEntry) so a docID-reuse-after-delete
// can never extend from a stale entry: the correctness-critical invariant is
// that on ANY settle where the store's resolved position for a cached docID
// drops to or below cached.frozenSeq (undo, or a reused docID whose fresh
// CurrentSeq()==0 sits below a stale frozenSeq left by the doc that used to
// own this id), the entry is evicted before any edit can be folded onto it —
// this fires unconditionally on every settle, never gated behind isLoadFamily
// or any message-type check, which is what makes it safe: mirrorFor itself is
// never sampled, so no docstate-mutating transition can ever happen between
// two mirrorFor calls for the same docID without this eviction check running
// in between. (Confirmed unreachable in the current tree via two independent
// reviews: undo and append never land in the same Update — MoveUndoPos and
// AppendEdit have disjoint caller sets — and every docID birth is either
// load-family (cache cleared directly) or a synchronous CreateScratch with
// zero events, so a reused id is always first observed at CurrentSeq()==0,
// which evicts any stale entry before an edit can extend it. See e.g. the
// dirty-untitled discard-on-close path, workspace_conflict.go's actionClose,
// which reuses a documents.id in a single, non-load-family Update.)
func (rs *runState) mirrorFor(docID int64, bufferContent string, isLoadFamily bool) string {
	if rs.store == nil || docID == 0 {
		return ""
	}

	if !isLoadFamily {
		if cached, ok := rs.mirrorCache[docID]; ok && cached.frozenSeq > 0 {
			if targetSeq, err := rs.store.CurrentSeq(docID); err == nil {
				switch {
				case targetSeq == cached.frozenSeq:
					return cached.frozenContent
				case targetSeq > cached.frozenSeq:
					if rows, dErr := rs.store.EditsInRange(docID, cached.frozenSeq, targetSeq); dErr == nil && len(rows) > 0 {
						frozenContent, frozenSeq := cached.frozenContent, cached.frozenSeq
						for _, row := range rows[:len(rows)-1] { // strictly-older rows: proven immutable, fold in
							frozenContent = buffer.ReplayForward(frozenContent, [][]buffer.AppliedEdit{row.Edits})
							frozenSeq = row.Seq
						}
						tail := rows[len(rows)-1] // current tail: ALWAYS freshly fetched, NEVER cached
						full := buffer.ReplayForward(frozenContent, [][]buffer.AppliedEdit{tail.Edits})
						rs.setMirrorCache(docID, frozenSeq, frozenContent)
						return full
					}
				}
			}
			// targetSeq < cached.frozenSeq (undo, or a reused docID whose
			// fresh position sits below a stale entry), or an unverifiable
			// read: never extend from a stale entry.
			delete(rs.mirrorCache, docID)
		}
	} else if rs.mirrorCache != nil {
		delete(rs.mirrorCache, docID)
	}

	// ORIGINAL fallback logic, preserved exactly (HasHistory/baseline
	// handling untouched), adapted only to use the seq-tagged fetch so the
	// cache can be (re)populated without ever treating the tail row as
	// frozen.
	targetSeq, seqErr := rs.store.CurrentSeq(docID)
	if seqErr != nil {
		// Skip, don't approximate: a read error here is expected once
		// teardownAndQuit (workspace_quit.go) closes the store mid-run
		// (cluster 15/quitSaveAll) — every subsequent read on the same doc
		// errors, so falling back to the STALE pre-edit baseline would
		// compare it against a live buffer that has since moved on,
		// false-positiving SHADOW on a doc that was never actually
		// corrupted (the store simply can't confirm it anymore). "" is
		// SHADOW's own established skip signal (its gate is
		// `MirrorContent != ""`) — fire-and-forget: degrades coverage for
		// this doc, never loses data, never false-positives.
		return ""
	}
	var rows []docstate.EditRow
	if targetSeq > 0 {
		var rErr error
		if rows, rErr = rs.store.EditsInRange(docID, 0, targetSeq); rErr != nil {
			return ""
		}
	}
	if len(rows) == 0 {
		hasHistory, hhErr := rs.store.HasHistory(docID)
		if baseline, exists := rs.baselines[docID]; exists && !isLoadFamily && hhErr == nil && hasHistory {
			// Fully undone (or never edited) on a doc we've already
			// baselined, and this settle was NOT driven by a fresh load —
			// the buffer must still match that baseline. Return it
			// UNCHANGED rather than silently adopting whatever the buffer
			// now holds: SHADOW's own Content != MirrorContent comparison
			// (internal/fuzz/ui/workspace/workspace.go) then catches a
			// genuine divergence — no separate/duplicated check needed.
			//
			// Gated on the STORE's own HasHistory, not just "a baseline
			// entry exists in rs.baselines": docID is a SQLite rowid, and
			// GuardDeleted's [D]iscard purge (store.DeleteDoc) followed by
			// a LATER CreateNewFile can hand that SAME numeric ID to a
			// brand-new, unrelated document — rs.baselines[docID] is then a
			// stale cache entry for a document that, from the STORE's own
			// point of view, no longer has any history at all. Trusting it
			// anyway false-positived SHADOW comparing a fresh blank
			// untitled buffer against the PURGED doc's old content (found
			// via FuzzHumanSession's dirtyCloseGuard-discard →
			// CreateNewFile sequence, WP7 follow-up session).
			return baseline
		}
		// No baseline yet, a load/reload legitimately establishes a new
		// one, or the store itself has no history for this docID (a fresh
		// or ID-reused document) — (re)record it per-doc so subsequent
		// edits replay on top.
		rs.baselines[docID] = bufferContent
		return bufferContent
	}
	frozenContent, frozenSeq := rs.baselines[docID], int64(0)
	for _, row := range rows[:len(rows)-1] {
		frozenContent = buffer.ReplayForward(frozenContent, [][]buffer.AppliedEdit{row.Edits})
		frozenSeq = row.Seq
	}
	tail := rows[len(rows)-1]
	full := buffer.ReplayForward(frozenContent, [][]buffer.AppliedEdit{tail.Edits})
	// frozenSeq may still be 0 here (a single-row doc) — fine, the next call
	// just won't use the fast path yet until a second row proves the first
	// one immutable.
	rs.setMirrorCache(docID, frozenSeq, frozenContent)
	return full
}
