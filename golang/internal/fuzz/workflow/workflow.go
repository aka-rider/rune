// Package workflow provides DecodeWorkflow, a byte-driven cluster grammar
// that maps fuzz corpus bytes to coherent sequences of user-level events.
// Unlike event.Decode (which maps every byte to a single keystroke),
// DecodeWorkflow selects named workflow *clusters* — multi-step sequences
// that mirror real user actions (open search, navigate tree, edit+undo, etc.)
// — and parameterises them from the remaining corpus bytes.
//
// FuzzHumanSession uses this instead of event.Decode so the fuzzer exercises
// coherent flows while retaining Go-native corpus shrinking.
//
// Cluster implementations live in sibling files to keep each under the
// 500-LoC limit (§1.6/§11): workflow_clusters_core.go (0-10, the original
// clusters), workflow_clusters_wp4.go (11-15, persistence), and
// workflow_clusters_wp6.go (16-19, input/dictation/chrome).
package workflow

import (
	"unicode/utf8"

	tea "charm.land/bubbletea/v2"

	"rune/internal/fuzz/event"
)

// DecodeWorkflow decodes a binary fuzzer input to a slice of Events using
// the cluster grammar. Every byte string produces some (possibly empty) list.
// maxWorkflowEvents bounds the events one fuzz input may expand to. Each cluster
// byte expands to several events, so a degenerate input (a long run of one cluster
// id) otherwise generates tens of thousands of events and times out in rendering —
// a fuzzer pathology that wastes the time budget, not a defect under test. Far above
// any realistic session, so it never clips genuine exploration.
const maxWorkflowEvents = 2000

func DecodeWorkflow(data []byte) []event.Event {
	var out []event.Event
	i := 0
	for i < len(data) && len(out) < maxWorkflowEvents {
		clusterID := data[i]
		i++
		var consumed int
		var evs []event.Event
		evs, consumed = decodeCluster(clusterID, data[i:])
		i += consumed
		out = append(out, evs...)
	}
	return out
}

// decodeCluster selects one cluster by clusterID % numClusters and parameterises
// it from the prefix of data, returning the events and bytes consumed.
func decodeCluster(id byte, data []byte) ([]event.Event, int) {
	// numClusters is 20 as of WP4 (clusters 11-15) / WP6 (clusters 16-19) —
	// bumped once at WP4, not incrementally, so corpus artifacts encoded
	// against this grammar keep a stable id%numClusters mapping across both
	// work packages.
	const numClusters = 20
	switch id % numClusters {
	case 0:
		return openSearchAndFind(data)
	case 1:
		return navigateTreeAndOpen(data)
	case 2:
		return editUndoRedoSave(data)
	case 3:
		return tabChurn(data)
	case 4:
		return dirtyCloseGuard(data)
	case 5:
		return externalChange(data)
	case 6:
		return resizeTerminal(data)
	case 7:
		return globalSeqDirtySpec(data)
	case 8:
		return followLink(data)
	case 9:
		return navigateTreeAndTrash(data)
	case 10:
		return navigateTreeAndNewFile(data)
	case 11:
		return mergeResolve(data)
	case 12:
		return externalRename(data)
	case 13:
		return externalDelete(data)
	case 14:
		return evictionPressure(data)
	case 15:
		return quitSaveAll(data)
	case 16:
		return selectionClipboard(data)
	case 17:
		return unicodeTyping(data)
	case 18:
		return dictationCluster(data)
	case 19:
		return workspaceChrome(data)
	}
	return nil, 0
}

// ---- helpers ----

func key(kp tea.KeyPressMsg) event.Event {
	// Find the index in bindingTable. We encode KindKey + 2-byte index.
	// For cluster grammar, we use hard-coded indices matching keytable.go.
	// The indices must match bindingTable in driver/keytable.go.
	return event.Event{Kind: event.KindKey, KeyIndex: keyPressToIndex(kp)}
}

// keyPressToIndex maps a well-known KeyPressMsg to its bindingTable index.
// Indices must match keytable.go exactly. Unrecognised keys map to 0 (Up).
func keyPressToIndex(kp tea.KeyPressMsg) uint16 {
	switch {
	case kp.Code == tea.KeyUp && kp.Mod == 0:
		return 0
	case kp.Code == tea.KeyDown && kp.Mod == 0:
		return 1
	case kp.Code == tea.KeyLeft && kp.Mod == 0:
		return 2
	case kp.Code == tea.KeyRight && kp.Mod == 0:
		return 3
	case kp.Code == tea.KeyHome && kp.Mod == 0:
		return 4
	case kp.Code == tea.KeyEnter && kp.Mod == 0:
		return 8
	case kp.Code == tea.KeyBackspace && kp.Mod == 0:
		return 9
	case kp.Code == tea.KeyEscape && kp.Mod == 0:
		return 32
	case kp.Code == 'w' && kp.Mod == tea.ModCtrl: // CloseFile
		return 14
	case kp.Code == 'n' && kp.Mod == tea.ModCtrl: // CreateNewFile
		return 18
	case kp.Code == 'p' && kp.Mod == tea.ModCtrl: // PinTab
		return 19
	case kp.Code == 'x' && kp.Mod == tea.ModCtrl: // FocusExplorer
		return 15
	case kp.Code == 'e' && kp.Mod == tea.ModCtrl: // FocusEditor
		return 16
	case kp.Code == 's' && kp.Mod == tea.ModSuper: // SaveFile
		return 76
	case kp.Code == 'z' && kp.Mod == tea.ModSuper: // Undo
		return 77
	case kp.Code == 'y' && kp.Mod == tea.ModCtrl: // Redo
		return 78
	// Search keys (new — appended at end of bindingTable)
	case kp.Code == 'f' && kp.Mod == tea.ModCtrl: // InFileSearch
		return 79
	case kp.Code == 'g' && kp.Mod == tea.ModSuper: // FindNext
		return 81
	case kp.Code == 'g' && kp.Mod == (tea.ModShift|tea.ModSuper): // FindPrev
		return 82
	// Explorer action
	case kp.Code == tea.KeyBackspace && kp.Mod == tea.ModSuper: // TrashFile
		return 84
	}
	return 0
}

// textEvent builds a KindText event, truncating s to at most 40 bytes on a
// rune boundary (§1.5: text carried on an event is later fed to editor
// insertion paths that assume valid UTF-8 — a byte-slice truncation can
// split a multi-byte rune and hand the buffer half a code point).
func textEvent(s string) event.Event {
	s = truncateRuneSafe(s, 40)
	return event.Event{Kind: event.KindText, Text: s}
}

// truncateRuneSafe returns the longest prefix of s that is both valid UTF-8
// and at most maxBytes long, never splitting a rune.
func truncateRuneSafe(s string, maxBytes int) string {
	if len(s) <= maxBytes {
		return s
	}
	end := maxBytes
	for end > 0 && !utf8.RuneStart(s[end]) {
		end--
	}
	return s[:end]
}

func repeat(ev event.Event, n int) []event.Event {
	out := make([]event.Event, n)
	for i := range out {
		out[i] = ev
	}
	return out
}

func sanitizeQuery(b []byte) string {
	out := make([]byte, 0, len(b))
	for _, c := range b {
		// Keep printable ASCII; remap control chars to letters.
		if c >= 32 && c < 127 {
			out = append(out, c)
		} else {
			out = append(out, 'x')
		}
	}
	if len(out) == 0 {
		return "q"
	}
	return string(out)
}

func clampU8(v, lo, hi uint8) uint8 {
	if v < lo {
		return lo
	}
	if v > hi {
		return hi
	}
	return v
}

// guardResponseIndex maps a 0–2 response to the bindingTable index for s/d/Esc.
// bindingTable: a=33, b=34, ..., d=36, ..., s=51, ..., Escape=32.
func guardResponseIndex(r uint8) uint16 {
	switch r {
	case 0:
		return 51 // 's' = save
	case 1:
		return 36 // 'd' = discard
	default:
		return 32 // Escape = cancel
	}
}

// trashGuardResponseIndex maps a 0–1 response to the bindingTable index for
// y (confirm, 57) or Escape (cancel, 32).
func trashGuardResponseIndex(r uint8) uint16 {
	if r == 0 {
		return 57 // 'y' = confirm trash
	}
	return 32 // Escape = cancel
}

// mergeGuardResponseIndex maps a 0-3 response to the bindingTable index for
// the conflict guard's m/d/s/Esc options (guardMergeOptions in workspace.go).
func mergeGuardResponseIndex(r uint8) uint16 {
	switch r % 4 {
	case 0:
		return 45 // 'm' = [M]erge → enter the resolver
	case 1:
		return 36 // 'd' = [D]iscard → load theirs
	case 2:
		return 51 // 's' = [S]ave anyway (CAS force-write)
	default:
		return 32 // Escape = Cancel
	}
}

// activePathSlot is watchTargetPath's "(len(paths)+1)th slot" convention
// (driver/driver_human.go) for "target whatever is currently the active/
// displayed file" — coupled to human_fuzz_test.go's humanPaths having
// exactly 5 entries (WP4: a.md, b.md, notes/c.md, d.md, notes/e.md); update
// together if humanPaths' length ever changes.
const activePathSlot = 5

// paletteText returns one of 16 fixed, compile-time strings — every one
// valid UTF-8 and at most 40 bytes — spanning the text shapes most likely to
// stress the editor's byte/rune handling (§1.5) and load/save round-trip
// (§1.4.5): CJK, emoji+ZWJ, combining accents, RTL+marks, mathematical
// alphanumerics, mixed scripts, CRLF, whitespace+ZWSP, markdown
// metacharacters, and plain ASCII/empty. Invalid UTF-8 is deliberately out
// of scope — the key/paste boundary only ever carries well-formed text; that
// is buffer-fuzzer (FuzzBuffer*) territory, not this harness's.
func paletteText(b byte) string {
	table := [16]string{
		"",                       // empty
		"hello world",            // plain ASCII
		"你好世界，世界你好",              // CJK
		"👨‍👩‍👧‍👦 family",         // emoji + ZWJ sequence
		"é à ô",                  // combining acute/grave/circumflex
		"مرحبا بالعالم",          // RTL (Arabic) + implicit direction marks
		"𝕳𝖊𝖑𝖑𝖔 𝟙𝟚𝟛",              // mathematical alphanumeric symbols
		"aA1! 你好 🙂 mix",          // mixed scripts
		"line1\r\nline2",         // CRLF — §1.4.5 verbatim probe
		" ​\t​ ",                 // whitespace + zero-width space
		"***bold*** _em_ `code`", // markdown metacharacters
		"\n\n\n",                 // blank lines only
		"\"quoted\" 'text'",      // quote characters
		"12345.6789",             // digits
		"a\tb\tc\td",             // tab-separated
		"𝓒𝓾𝓻𝓼𝓲𝓿𝓮",                // mathematical script symbols
	}
	return table[int(b)%len(table)]
}

var (
	evDown      = key(tea.KeyPressMsg{Code: tea.KeyDown})
	evHome      = key(tea.KeyPressMsg{Code: tea.KeyHome})
	evEnter     = key(tea.KeyPressMsg{Code: tea.KeyEnter})
	evEsc       = key(tea.KeyPressMsg{Code: tea.KeyEscape})
	evLeft      = key(tea.KeyPressMsg{Code: tea.KeyLeft})
	evBackspace = key(tea.KeyPressMsg{Code: tea.KeyBackspace})
	evSave      = key(tea.KeyPressMsg{Code: 's', Mod: tea.ModSuper})
	evUndo      = key(tea.KeyPressMsg{Code: 'z', Mod: tea.ModSuper})
	evRedo      = key(tea.KeyPressMsg{Code: 'y', Mod: tea.ModCtrl})
	evClose     = key(tea.KeyPressMsg{Code: 'w', Mod: tea.ModCtrl})
	evNew       = key(tea.KeyPressMsg{Code: 'n', Mod: tea.ModCtrl})
	evPin       = key(tea.KeyPressMsg{Code: 'p', Mod: tea.ModCtrl})
	evTree      = key(tea.KeyPressMsg{Code: 'x', Mod: tea.ModCtrl})
	evEdit      = key(tea.KeyPressMsg{Code: 'e', Mod: tea.ModCtrl})
	evSearch    = key(tea.KeyPressMsg{Code: 'f', Mod: tea.ModCtrl})
	evFindNext  = key(tea.KeyPressMsg{Code: 'g', Mod: tea.ModSuper})
	evFindPrev  = key(tea.KeyPressMsg{Code: 'g', Mod: tea.ModShift | tea.ModSuper})
	evTrash     = key(tea.KeyPressMsg{Code: tea.KeyBackspace, Mod: tea.ModSuper})

	// Merge resolver keys (mergemode.HandleKey matches msg.Code only — see
	// keytable.go's WP0 doc comment). Built directly with the bindingTable
	// indices for the unmodified printables 'o'/'t'/'n' (33 + letter offset;
	// keyPressToIndex has no case for these bare letters, so key() would
	// wrongly default to index 0/Up — bypass it, matching
	// guardResponseIndex's own "hard-coded index" convention below).
	evMergeOurs   = event.Event{Kind: event.KindKey, KeyIndex: 47} // 'o' = accept ours
	evMergeTheirs = event.Event{Kind: event.KindKey, KeyIndex: 52} // 't' = accept theirs
	evMergeNext   = event.Event{Kind: event.KindKey, KeyIndex: 46} // 'n' = next conflict block

	// WP3 keytable appends (indices 85-110 — see driver/keytable.go).
	evCopy           = event.Event{Kind: event.KindKey, KeyIndex: 85}  // ⌘C = CopyToClipboard
	evCut            = event.Event{Kind: event.KindKey, KeyIndex: 86}  // ⌘X = CutToClipboard
	evPasteRequest   = event.Event{Kind: event.KindKey, KeyIndex: 87}  // ⌘V = PasteFromClipboard (request phase)
	evAddCursorUp    = event.Event{Kind: event.KindKey, KeyIndex: 88}  // ⌥⌘↑ = AddCursorAbove
	evAddCursorDown  = event.Event{Kind: event.KindKey, KeyIndex: 89}  // ⌥⌘↓ = AddCursorBelow
	evHelp           = event.Event{Kind: event.KindKey, KeyIndex: 90}  // F1 = Help
	evVoiceDictate   = event.Event{Kind: event.KindKey, KeyIndex: 101} // ^V = VoiceDictation
	evShiftWordLeft  = event.Event{Kind: event.KindKey, KeyIndex: 102} // ⌥⇧← = ShiftWordLeft
	evShiftWordRight = event.Event{Kind: event.KindKey, KeyIndex: 103} // ⌥⇧→ = ShiftWordRight
	evShiftBottom    = event.Event{Kind: event.KindKey, KeyIndex: 105} // ⇧end = ShiftGotoBottom
	evMoveLineUp     = event.Event{Kind: event.KindKey, KeyIndex: 30}  // ⌥↑ = MoveLineUp
	evMoveLineDown   = event.Event{Kind: event.KindKey, KeyIndex: 31}  // ⌥↓ = MoveLineDown
	evSelectAll      = event.Event{Kind: event.KindKey, KeyIndex: 22}  // ^A = SelectAll
	evShiftRight     = event.Event{Kind: event.KindKey, KeyIndex: 27}  // ⇧→ = extend selection right
	evFocusChat      = event.Event{Kind: event.KindKey, KeyIndex: 17}  // ^R = FocusChat
	evZenMode        = event.Event{Kind: event.KindKey, KeyIndex: 13}  // ^O = ZenMode
	evConfirmExitC   = event.Event{Kind: event.KindKey, KeyIndex: 23}  // ^C = ConfirmExitC (chord)
	evPgDown         = event.Event{Kind: event.KindKey, KeyIndex: 7}   // PgDown
	evHalfPageUp     = event.Event{Kind: event.KindKey, KeyIndex: 20}  // ^U = HalfPageUp

	// Multi-byte printables (indices 108-110) — CJK/emoji/precomposed accent.
	evCJK      = event.Event{Kind: event.KindKey, KeyIndex: 108}
	evEmoji    = event.Event{Kind: event.KindKey, KeyIndex: 109}
	evAccented = event.Event{Kind: event.KindKey, KeyIndex: 110}
)

// tabSwitchIndex maps a 0-9 digit to the bindingTable index for ^1..^0
// (indices 91-100 — see driver/keytable.go's WP3 appends).
func tabSwitchIndex(digit uint8) uint16 {
	return uint16(91) + uint16(digit%10)
}
