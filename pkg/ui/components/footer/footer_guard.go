package footer

import (
	tea "charm.land/bubbletea/v2"

	"rune/pkg/ui/styles"
)

// GuardKind identifies which type of guard is active.
type GuardKind int

const (
	GuardDirty GuardKind = iota
	GuardMerge
	GuardTrash
	// GuardDeleted gates recovery when the current document's file has gone
	// missing on disk (deleted, or its parent dir removed) — mirrors GuardMerge
	// but with no "theirs" to diff against (§1.4.7).
	GuardDeleted
	// GuardRaced gates a Materialize swap-race outcome (F5): our write
	// committed for real, but a concurrent writer's bytes were displaced and
	// captured (never lost — I1). A DISTINCT guard from GuardMerge: there is
	// no live disk divergence to re-probe (a fresh read would just find OUR
	// already-committed bytes), only a choice between keeping our write or
	// restoring the captured displaced bytes on top of it.
	GuardRaced
	// GuardDegraded confirms an explicit write while the store is degraded
	// (docstate.Store.Degraded(): capture-into-RAM masquerading as
	// durability — the recovery journal will NOT survive a crash for the
	// rest of this session). Raised before an interactive save proceeds.
	GuardDegraded
)

// GuardOption maps a keyboard input to a guard response.
type GuardOption struct {
	Key      rune
	Response DataLossGuardResponse
}

// DataLossGuardResponse enumerates user responses to data-loss guard prompts.
type DataLossGuardResponse int

const (
	DataLossSave DataLossGuardResponse = iota
	DataLossDiscard
	DataLossCancel
	DataLossMergeAccept
	DataLossMergeReject
	DataLossTrash
	// DataLossSaveAnyway is the [S]ave anyway response for the conflict guard
	// (GuardMerge): the user acknowledges the external change and overwrites.
	DataLossSaveAnyway
	// DataLossMerge is the [M]erge response for the conflict guard (GuardMerge):
	// the user requests the interactive merge resolver (Phase 2).
	DataLossMerge
	// DataLossKeepMine is the [K]eep-mine response for the swap-race guard
	// (GuardRaced, F5): our already-committed write stands; the displaced
	// bytes remain recoverable history but are not restored to disk.
	DataLossKeepMine
	// DataLossRestoreTheirs is the [R]estore-theirs response for the
	// swap-race guard (GuardRaced, F5): the captured displaced bytes are
	// written back to disk, on top of our already-committed write.
	DataLossRestoreTheirs
	// DataLossConfirmDegraded is the [Y]es response for the degraded-store
	// guard (GuardDegraded): the user acknowledges the save proceeds without
	// a durable recovery journal for the rest of this session.
	DataLossConfirmDegraded
)

// DataLossGuardResponseMsg is emitted when the user responds to a guard prompt.
type DataLossGuardResponseMsg struct {
	Response DataLossGuardResponse
}

// resolveGuard is the chokepoint all three guard-resolution paths in Update
// funnel through — a typed key matching one of m.guardOptions, Enter mapping
// to the last option (GuardMerge/GuardDeleted only), and Cancel mapping to
// the last option: clear the guard state and emit opt's response. Each
// caller still decides WHICH opt to resolve with; this only centralizes what
// happens once one is chosen.
func (m Model) resolveGuard(opt GuardOption) (Model, tea.Cmd) {
	m.guardKind = 0
	m.guardOptions = nil
	m.guardLabel = ""
	return m, func() tea.Msg { return DataLossGuardResponseMsg{Response: opt.Response} }
}

// guardOptionHint is one keyed option in a guard's rendered hint, e.g.
// "[S]ave": Key is FooterKey-styled, Suffix is FooterHint-styled and
// immediately follows the closing "]" (a word-continuation like "ave" needs
// no leading space; a standalone word like " Cancel" supplies its own).
type guardOptionHint struct {
	Key    string
	Suffix string
}

// guardDescriptor is the render recipe for one GuardKind: a label followed
// by its keyed option hints.
type guardDescriptor struct {
	Label   string
	Options []guardOptionHint
}

// guardDescriptorFor is the single source View's guard-render arm reads
// from — before this chokepoint, each of the 6 guard kinds independently
// built its own "label + [Key]suffix [Key]suffix..." construction inline.
// ok is false for a GuardKind with no descriptor (there is none today; every
// declared GuardKind has one).
func guardDescriptorFor(kind GuardKind) (guardDescriptor, bool) {
	switch kind {
	case GuardDirty:
		// Label defaults to "Unsaved changes." — View overrides it with
		// m.guardLabel when set (e.g. the eviction victim's filename).
		return guardDescriptor{
			Label:   "Unsaved changes.",
			Options: []guardOptionHint{{"S", "ave"}, {"D", "iscard"}, {"Esc", " Cancel"}},
		}, true
	case GuardMerge:
		// R4: [S]ave anyway [D]iscard [M]erge [Esc] — Enter is neutralized to Cancel.
		return guardDescriptor{
			Label:   "File changed on disk.",
			Options: []guardOptionHint{{"S", "ave anyway"}, {"D", "iscard"}, {"M", "erge"}, {"Esc", ""}},
		}, true
	case GuardDeleted:
		// Mirrors GuardMerge's rendering, but there is no "theirs" to diff
		// against — only [S]ave (recreate) / [D]iscard (purge) / Esc.
		return guardDescriptor{
			Label:   "File deleted on disk.",
			Options: []guardOptionHint{{"S", "ave"}, {"D", "iscard"}, {"Esc", ""}},
		}, true
	case GuardRaced:
		return guardDescriptor{
			Label:   "Save raced with a concurrent write.",
			Options: []guardOptionHint{{"K", "eep mine"}, {"R", "estore theirs"}, {"Esc", ""}},
		}, true
	case GuardDegraded:
		return guardDescriptor{
			Label:   "Storage degraded — history will not survive a crash.",
			Options: []guardOptionHint{{"Y", "es, save anyway"}, {"Esc", " Cancel"}},
		}, true
	case GuardTrash:
		return guardDescriptor{
			Label:   "Trash file?",
			Options: []guardOptionHint{{"Y", "es"}, {"Esc", " Cancel"}},
		}, true
	}
	return guardDescriptor{}, false
}

// renderGuardHint renders one guard descriptor's label followed by its
// "[Key]suffix" options, each separated by a space — byte-identical to the
// inline construction it replaces.
func renderGuardHint(st styles.Styles, label string, opts []guardOptionHint) string {
	s := st.FooterKey.Render(label)
	for _, o := range opts {
		s += st.FooterHint.Render(" [") + st.FooterKey.Render(o.Key) + st.FooterHint.Render("]"+o.Suffix)
	}
	return s
}
