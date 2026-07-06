// Package mergemode owns the interactive 3-way merge resolver — the ONLY UI
// package importing rune/pkg/merge (§10). It builds the conflict-marker
// working form in the LIVE main-editor buffer (so [O]/[T] are ordinary
// journaled ReplaceRanges, undoable/redoable for free via the workspace
// journal), and renders a separate read-only diff view (a PlainSync textedit
// instance) coloring ours/theirs/markers via the textedit background-interval
// overlay. See the package-level design note in the plan for the full
// rationale: markdownedit stays merge-free (CGO-free import graph); mergemode
// is workspace-adjacent orchestration glue (imports both a sibling component
// and a domain package), so it lives under pkg/ui/pages/workspace/.
package mergemode

import (
	"charm.land/bubbles/v2/key"
	tea "charm.land/bubbletea/v2"
	"charm.land/lipgloss/v2"

	"rune/pkg/editor/cursor"
	"rune/pkg/merge"
	"rune/pkg/ui/components/markdownedit"
	"rune/pkg/ui/components/textedit"
	"rune/pkg/ui/keymap"
	"rune/pkg/ui/styles"
)

// Colors used ONLY by the merge diff view — ours green bg, theirs red bg
// (today's markdownedit conflictBg), marker lines a dim gray bg.
var (
	oursColor   = lipgloss.Color("22")
	theirsColor = lipgloss.Color("52")
	markerColor = lipgloss.Color("240")
)

// Marker framing bytes — authored ONLY by mergemode. Enter builds the working
// buffer with these exact bytes; Resync's verifying scan searches for this
// exact byte-for-byte sequence (mergemode_resync.go), so the two MUST stay in
// lockstep — always go through frameBlock.
const (
	oursMarkerLine   = "<<<<<<< ours\n"
	sepMarkerLine    = "=======\n"
	theirsMarkerLine = ">>>>>>> theirs\n"
)

// conflictEntry is one immutable conflict from the 3-way merge result
// (captured once in Enter, verbatim per §1.4.5). Never mutated afterward —
// only WHERE it currently lives in the buffer (block) changes.
type conflictEntry struct {
	ours, theirs []byte
}

// block is the current buffer span of one conflict. While unresolved,
// [start,end) holds the exact framed marker bytes (frameBlock(ours,theirs));
// once resolved, [start,end) holds the accepted side's bytes and no markers
// remain in that span.
type block struct {
	start, end int
	resolved   bool
}

// State is the merge resolver state. Zero value = inactive (active=false);
// production always constructs via New so the read-only view instance exists
// before first use.
type State struct {
	active       bool
	conflicts    []conflictEntry
	blocks       []block
	cur          int // index into conflicts/blocks that [O]/[T] act on; -1 = none
	view         textedit.Model
	preMergeOurs string          // ed.Content() just before Enter's marker ReplaceAll; Abort restores this
	keys         keymap.Bindings // §3.1: named bindings for [O]/[T]/[n]/[p] — visible to ValidateNoPhysicalKeyCollisions, unlike a raw msg.Code comparison
}

// New constructs the read-only merge-view instance (call once at workspace
// New, sized later via SetSize). SetReadOnly(true) AND SetFocused(true) are
// both required (critic R4): textedit dims unfocused content and drops
// selection overlays when unfocused (View's `dim := !m.focused`), and the
// merge view never receives real workspace focus — so it must be focused
// internally to render un-dimmed and keep selections live for copy.
func New(keys keymap.Bindings, st styles.Styles) State {
	view := textedit.New(keys, st).SetReadOnly(true).SetFocused(true)
	return State{view: view, cur: -1, keys: keys}
}

// SetSize sizes the merge-view instance to the workspace's center pane.
func SetSize(st State, w, h int) State {
	st.view = st.view.SetRect(textedit.Rect{W: w, H: h})
	return st
}

// View renders the merge diff view.
func (st State) View() string { return st.view.View() }

// IsActive reports whether the merge resolver is currently active.
func IsActive(st State) bool { return st.active }

// HasUnresolvedConflicts reports whether an active merge still has unresolved
// conflict blocks. False when not active.
func HasUnresolvedConflicts(st State) bool { return st.active && len(st.blocks) > 0 }

// ConflictsLeft returns the count of unresolved conflict blocks (for the
// footer hint). 0 when not active or all resolved.
func ConflictsLeft(st State) int {
	n := 0
	for _, b := range st.blocks {
		if !b.resolved {
			n++
		}
	}
	return n
}

// frameBlock builds the exact byte sequence for one 2-way conflict marker
// block (diff3 "|||||||" ancestor omitted — a 2-way view, §3). Enter uses this
// to build the working buffer; Resync's verifying scan searches for this exact
// sequence — the two must never diverge.
func frameBlock(ours, theirs []byte) []byte {
	out := make([]byte, 0, len(oursMarkerLine)+len(ours)+1+len(sepMarkerLine)+len(theirs)+1+len(theirsMarkerLine))
	out = append(out, oursMarkerLine...)
	out = append(out, ours...)
	out = append(out, '\n')
	out = append(out, sepMarkerLine...)
	out = append(out, theirs...)
	out = append(out, '\n')
	out = append(out, theirsMarkerLine...)
	return out
}

// buildMarkerBuffer walks hunks and builds the merged conflict-marker working
// form: HunkClean regions pass through verbatim, HunkConflict regions become a
// framed marker block (frameBlock). Returns the merged bytes, the immutable
// per-conflict ours/theirs pairs (in order), and their current block spans in
// the merged bytes. Shared by Enter and Preview so the two stay byte-identical
// (Fix D / BUG2) — Preview must show EXACTLY what Enter would put in the
// buffer, just without writing it there or activating the resolver.
func buildMarkerBuffer(hunks []merge.Hunk) (merged []byte, conflicts []conflictEntry, blocks []block) {
	offset := 0
	for _, h := range hunks {
		switch h.Kind {
		case merge.HunkClean:
			merged = append(merged, h.AutoBytes...)
			offset += len(h.AutoBytes)
		case merge.HunkConflict:
			ours := append([]byte(nil), h.OursBytes...)
			theirs := append([]byte(nil), h.TheirsBytes...)
			framed := frameBlock(ours, theirs)
			start := offset
			merged = append(merged, framed...)
			offset += len(framed)
			conflicts = append(conflicts, conflictEntry{ours: ours, theirs: theirs})
			blocks = append(blocks, block{start: start, end: offset})
		}
	}
	return merged, conflicts, blocks
}

// Enter runs the hunks into the live buffer (one journaled whole-buffer
// ReplaceAll via ed.ReplaceAll), records block spans, builds the merge-view
// content + color intervals, and activates (only when at least one conflict
// exists — a clean-only merge needs no user interaction). Captures the
// pre-merge ours content so Abort can restore it exactly.
//
// Returns a non-nil error when the marker-buffer install fails (§1.3); the
// caller MUST surface it (e.g. via the workspace's errorCmd) — st/ed are
// returned unchanged (the resolver never activates on a failed install).
func Enter(hunks []merge.Hunk, st State, ed markdownedit.Model) (State, markdownedit.Model, tea.Cmd, error) {
	preMergeOurs := ed.Content()
	merged, conflicts, blocks := buildMarkerBuffer(hunks)

	var cmd tea.Cmd
	var err error
	ed, cmd, err = ed.ReplaceAll(string(merged))
	if err != nil {
		return st, ed, nil, err
	}

	st.preMergeOurs = preMergeOurs
	st.conflicts = conflicts
	st.blocks = blocks
	st.active = len(blocks) > 0
	st.cur = firstUnresolved(blocks)
	st = st.refreshView(ed)
	return st, ed, cmd, nil
}

// Preview builds a READ-ONLY ours-vs-theirs diff into the merge-view instance
// for the [S]/[D]/[M] guard (BUG2/Fix D) — the "review step" for a clean
// auto-merge and the immediate diff for a real conflict — WITHOUT touching the
// live editor buffer and WITHOUT activating the resolver: active is left
// exactly as st carried it in (never flipped true here), so the R2 save-gate
// (HasUnresolvedConflicts), the paneCenter merge key-intercept, and
// syncMergeHint all stay inert (critic-verified). The caller renders
// st.View() in place of the main editor while m.pendingConflict.active (see
// workspace_view.go) and clears the preview on every guard response. Uses the
// SAME buildMarkerBuffer as Enter so the preview is byte-identical to what
// Enter would show once the resolver actually activates.
func Preview(hunks []merge.Hunk, st State) State {
	merged, conflicts, blocks := buildMarkerBuffer(hunks)
	st.conflicts = conflicts
	st.blocks = blocks
	st.cur = firstUnresolved(blocks)
	st.view = st.view.SetContent(string(merged)).SetBackgroundIntervals(st.colorIntervals())
	if st.cur >= 0 && st.cur < len(st.blocks) {
		st.view = st.view.SetCursors([]cursor.Cursor{{Position: st.blocks[st.cur].start}})
	}
	return st
}

// Abort reverts the live buffer to the pre-merge ours content (one journaled
// ReplaceAll on ed) and deactivates — the Esc/cancel escape hatch for the
// modal resolver.
//
// Returns a non-nil error when the revert fails (§1.3); the caller MUST
// surface it — st/ed are returned unchanged (the resolver stays active
// rather than silently discarding the failed-to-apply revert).
func Abort(st State, ed markdownedit.Model) (State, markdownedit.Model, tea.Cmd, error) {
	var cmd tea.Cmd
	var err error
	ed, cmd, err = ed.ReplaceAll(st.preMergeOurs)
	if err != nil {
		return st, ed, nil, err
	}
	st.active = false
	st.conflicts = nil
	st.blocks = nil
	st.cur = -1
	st.preMergeOurs = ""
	st = st.refreshView(ed)
	return st, ed, cmd, nil
}

// Reset clears active/conflict state (keeps the reusable merge-view
// instance). Called defensively after a fully-resolved save.
func Reset(st State) State {
	st.active = false
	st.conflicts = nil
	st.blocks = nil
	st.cur = -1
	st.preMergeOurs = ""
	return st
}

// HandleKey handles one keypress while active. [O]/[T] apply a journaled
// ReplaceRange to ed (caller drains+journals), then rebuild the merge view.
// [n]/[p] move the current block and scroll the merge view. Up/Down/PgUp/
// PgDn/Home/End scroll the merge view. Any other key is consumed (no free
// editing during merge). Returns (State, editor, cmd, consumed, err); err is
// non-nil only when an [O]/[T] accept's ReplaceRange fails (§1.3) — the
// caller MUST surface it (e.g. via the workspace's errorCmd).
func HandleKey(st State, ed markdownedit.Model, msg tea.KeyPressMsg) (State, markdownedit.Model, tea.Cmd, bool, error) {
	if !st.active {
		return st, ed, nil, false, nil
	}

	switch {
	case key.Matches(msg, st.keys.MergeAcceptOurs):
		var cmd tea.Cmd
		var err error
		st, ed, cmd, err = st.accept(ed, true)
		return st, ed, cmd, true, err

	case key.Matches(msg, st.keys.MergeAcceptTheirs):
		var cmd tea.Cmd
		var err error
		st, ed, cmd, err = st.accept(ed, false)
		return st, ed, cmd, true, err

	case key.Matches(msg, st.keys.MergeNext):
		st = st.moveCurrent(1)
		st = st.refreshView(ed)
		return st, ed, nil, true, nil

	case key.Matches(msg, st.keys.MergePrev):
		st = st.moveCurrent(-1)
		st = st.refreshView(ed)
		return st, ed, nil, true, nil

	case isScrollKey(msg):
		st.view = st.scroll(msg)
		return st, ed, nil, true, nil

	default:
		// No free-text editing while merging (§3) — every other key is
		// silently consumed so it never reaches the (hidden) main buffer.
		return st, ed, nil, true, nil
	}
}

// accept collapses the current conflict block to ours (useOurs=true) or
// theirs (useOurs=false) via a journaled ReplaceRange, shifts every later
// block's span by the byte-length delta, and advances to the next unresolved
// block. A no-op if there is no current unresolved block.
//
// Returns a non-nil error when the ReplaceRange fails (§1.3); st/ed are then
// returned unchanged (the block stays unresolved rather than being marked
// resolved against an edit that never applied).
func (st State) accept(ed markdownedit.Model, useOurs bool) (State, markdownedit.Model, tea.Cmd, error) {
	if st.cur < 0 || st.cur >= len(st.blocks) || st.blocks[st.cur].resolved {
		return st, ed, nil, nil
	}
	k := st.cur
	b := st.blocks[k]

	var text string
	if useOurs {
		text = string(st.conflicts[k].ours)
	} else {
		text = string(st.conflicts[k].theirs)
	}

	var cmd tea.Cmd
	var err error
	ed, cmd, err = ed.ReplaceRange(b.start, b.end, text)
	if err != nil {
		return st, ed, nil, err
	}

	// Copy the slice before mutation (value semantics, §1.1).
	newBlocks := make([]block, len(st.blocks))
	copy(newBlocks, st.blocks)

	delta := len(text) - (b.end - b.start)
	newBlocks[k] = block{start: b.start, end: b.start + len(text), resolved: true}
	for j := k + 1; j < len(newBlocks); j++ {
		newBlocks[j].start += delta
		newBlocks[j].end += delta
	}
	st.blocks = newBlocks
	st.active = firstUnresolved(newBlocks) >= 0
	if st.active {
		st.cur = nextUnresolvedFrom(newBlocks, k)
	} else {
		st.cur = -1
	}
	st = st.refreshView(ed)
	return st, ed, cmd, nil
}

// moveCurrent moves st.cur to the next unresolved block in direction dir
// (+1 = [n]ext, -1 = [p]rev), wrapping around. No-op if there is nothing
// unresolved to navigate to.
func (st State) moveCurrent(dir int) State {
	n := len(st.blocks)
	if n == 0 {
		return st
	}
	cur := st.cur
	if cur < 0 {
		cur = 0
	}
	for i := 1; i <= n; i++ {
		idx := (((cur + dir*i) % n) + n) % n
		if !st.blocks[idx].resolved {
			st.cur = idx
			break
		}
	}
	return st
}

// firstUnresolved returns the index of the first unresolved block, or -1.
func firstUnresolved(blocks []block) int {
	for i, b := range blocks {
		if !b.resolved {
			return i
		}
	}
	return -1
}

// nextUnresolvedFrom returns the index of the next unresolved block strictly
// after from (wrapping), or -1 if none remain.
func nextUnresolvedFrom(blocks []block, from int) int {
	n := len(blocks)
	for i := 1; i <= n; i++ {
		idx := (from + i) % n
		if !blocks[idx].resolved {
			return idx
		}
	}
	return -1
}

// colorIntervals computes the ours/theirs/marker background tints for every
// still-unresolved block, derived purely from each block's start offset and
// the immutable conflict's ours/theirs byte lengths (the framed layout is
// fully deterministic — see frameBlock). Resolved blocks contribute no
// interval: their span no longer holds markers, just plain accepted content.
func (st State) colorIntervals() []textedit.BgInterval {
	var ivs []textedit.BgInterval
	for k, b := range st.blocks {
		if b.resolved {
			continue
		}
		c := st.conflicts[k]
		oursStart := b.start + len(oursMarkerLine)
		oursEnd := oursStart + len(c.ours)
		theirsStart := oursEnd + 1 + len(sepMarkerLine)
		theirsEnd := theirsStart + len(c.theirs)

		ivs = append(ivs,
			textedit.BgInterval{Start: b.start, End: oursStart, Color: markerColor},
			textedit.BgInterval{Start: oursStart, End: oursEnd, Color: oursColor},
			textedit.BgInterval{Start: oursEnd, End: theirsStart, Color: markerColor},
			textedit.BgInterval{Start: theirsStart, End: theirsEnd, Color: theirsColor},
			textedit.BgInterval{Start: theirsEnd, End: b.end, Color: markerColor},
		)
	}
	return ivs
}

// refreshView rebuilds the merge-view content and color overlay from ed's
// current buffer, and scrolls it to the current block (if any). Called after
// every structural change: Enter, accept, moveCurrent, Resync, Abort.
func (st State) refreshView(ed markdownedit.Model) State {
	content := ed.Content()
	st.view = st.view.SetContent(content).SetBackgroundIntervals(st.colorIntervals())
	if st.cur >= 0 && st.cur < len(st.blocks) {
		pos := st.blocks[st.cur].start
		st.view = st.view.SetCursors([]cursor.Cursor{{Position: pos}})
	}
	return st
}

// isScrollKey reports whether msg is a merge-view scroll key.
func isScrollKey(msg tea.KeyPressMsg) bool {
	switch msg.Code {
	case tea.KeyUp, tea.KeyDown, tea.KeyPgUp, tea.KeyPgDown, tea.KeyHome, tea.KeyEnd:
		return true
	}
	return false
}

// scroll applies one scroll key to the merge view. The view is read-only, so
// Up/Down move the viewport (not a caret) — mirrors textedit's own
// read-only-mode scroll behavior (execCursorUp/execCursorDown).
func (st State) scroll(msg tea.KeyPressMsg) textedit.Model {
	v := st.view
	switch msg.Code {
	case tea.KeyUp:
		v = v.SetScrollOffset(v.ScrollOffset() - 1)
	case tea.KeyDown:
		v = v.SetScrollOffset(v.ScrollOffset() + 1)
	case tea.KeyPgUp:
		v = v.SetScrollOffset(v.ScrollOffset() - v.ContentHeight())
	case tea.KeyPgDown:
		v = v.SetScrollOffset(v.ScrollOffset() + v.ContentHeight())
	case tea.KeyHome:
		v = v.SetScrollOffset(0)
	case tea.KeyEnd:
		v = v.GotoBottom()
	}
	return v
}
